"""The session/task store. Only module that imports sqlite3."""

import sqlite3
from datetime import datetime, timezone
from pathlib import Path

from . import config

SCHEMA_PATH = Path(__file__).parent / "schema.sql"
SCHEMA_VERSION = 2

# Statuses that mean "still open" — what the debrief asks about and the
# close paths carry into the backlog.
UNFINISHED = ("planned", "doing")


def now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def connect() -> sqlite3.Connection:
    config.STORE_PATH.parent.mkdir(parents=True, exist_ok=True)
    # IMMEDIATE: `with conn:` takes the write lock up front. A deferred
    # transaction that reads before writing (carry does) can die with
    # SQLITE_BUSY_SNAPSHOT when the desktop app commits in between, and
    # busy_timeout does not retry that.
    conn = sqlite3.connect(config.STORE_PATH, isolation_level="IMMEDIATE")
    conn.row_factory = sqlite3.Row
    # Both pragmas are per-connection in SQLite, so they can't live in schema.sql
    conn.execute("PRAGMA foreign_keys = ON")
    conn.execute("PRAGMA busy_timeout = 5000")
    return conn


def init(conn: sqlite3.Connection) -> None:
    exists = conn.execute(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta'"
    ).fetchone()
    if not exists:
        conn.executescript(SCHEMA_PATH.read_text())
        return
    version = int(
        conn.execute("SELECT value FROM meta WHERE key = 'schema_version'").fetchone()[0]
    )
    if version < 2:
        _backup(conn, suffix=f".v{version}.bak")
        _migrate_v1_to_v2(conn)
    elif version > SCHEMA_VERSION:
        raise RuntimeError(
            f"store is schema v{version}, this code understands up to v{SCHEMA_VERSION}"
        )


def _backup(conn: sqlite3.Connection, suffix: str) -> None:
    # SQLite's online backup API, not cp: copying a live WAL database can
    # capture a torn state.
    target_path = Path(str(config.STORE_PATH) + suffix)
    target = sqlite3.connect(target_path)
    try:
        conn.backup(target)
    finally:
        target.close()


def _migrate_v1_to_v2(conn: sqlite3.Connection) -> None:
    """v1 -> v2: nullable session_id (backlog), 'doing', carry chain,
    heartbeats, analysis table.

    Table rebuild is required — CHECK constraints and NOT NULL can't be
    ALTERed in place. foreign_keys must be OFF during the rebuild and can't
    change mid-transaction, so it's toggled outside; the rebuild itself is
    one IMMEDIATE transaction under manual control.
    """
    saved_isolation = conn.isolation_level
    conn.isolation_level = None  # autocommit: we manage the transaction
    conn.execute("PRAGMA foreign_keys = OFF")
    try:
        conn.execute("BEGIN IMMEDIATE")
        conn.execute("ALTER TABLE session ADD COLUMN last_heartbeat TEXT")
        conn.execute(
            """
            CREATE TABLE task_v2 (
                id           INTEGER PRIMARY KEY,
                session_id   INTEGER REFERENCES session(id),
                title        TEXT NOT NULL,
                position     INTEGER NOT NULL,
                status       TEXT NOT NULL DEFAULT 'planned'
                             CHECK (status IN ('planned', 'doing', 'done', 'dropped')),
                source       TEXT NOT NULL DEFAULT 'gate'
                             CHECK (source IN ('gate', 'mid-session')),
                carried_from INTEGER REFERENCES task(id) ON DELETE SET NULL,
                created_at   TEXT NOT NULL,
                started_at   TEXT,
                resolved_at  TEXT
            )
            """
        )
        conn.execute(
            "INSERT INTO task_v2 (id, session_id, title, position, status,"
            " source, created_at, resolved_at)"
            " SELECT id, session_id, title, position, status,"
            " source, created_at, resolved_at FROM task"
        )
        conn.execute("DROP TABLE task")
        conn.execute("ALTER TABLE task_v2 RENAME TO task")
        conn.execute(
            "CREATE UNIQUE INDEX task_carried_once"
            " ON task (carried_from) WHERE carried_from IS NOT NULL"
        )
        conn.execute(
            """
            CREATE TABLE analysis (
                id            INTEGER PRIMARY KEY,
                session_id    INTEGER NOT NULL REFERENCES session(id),
                created_at    TEXT NOT NULL,
                window_start  TEXT NOT NULL,
                window_end    TEXT NOT NULL,
                headline      TEXT NOT NULL,
                alignment     INTEGER,
                body          TEXT NOT NULL,
                observed_json TEXT NOT NULL,
                seen_at       TEXT
            )
            """
        )
        violations = conn.execute("PRAGMA foreign_key_check").fetchall()
        if violations:
            raise RuntimeError(f"migration broke foreign keys: {violations[:3]}")
        conn.execute("UPDATE meta SET value = '2' WHERE key = 'schema_version'")
        conn.execute("COMMIT")
    except BaseException:
        conn.execute("ROLLBACK")
        raise
    finally:
        conn.execute("PRAGMA foreign_keys = ON")
        conn.isolation_level = saved_isolation


def commit_draft(
    conn: sqlite3.Connection,
    statement: str,
    intended_minutes: int | None,
    mode: str,
    titles: list[str],
    backlog_ids: list[int] | None = None,
) -> int:
    # One transaction: the session, its tasks, and any backlog pulls land
    # together or not at all.
    with conn:
        session_id = create_session(conn, statement, intended_minutes, mode)
        add_tasks(conn, session_id, titles)
        if backlog_ids:
            _pull_from_backlog(conn, session_id, backlog_ids)
    return session_id


def create_session(
    conn: sqlite3.Connection, statement: str, intended_minutes: int | None, mode: str
) -> int:
    cur = conn.execute(
        "INSERT INTO session (started_at, statement, intended_minutes, mode)"
        " VALUES (?, ?, ?, ?)",
        (now(), statement, intended_minutes, mode),
    )
    return cur.lastrowid


def add_tasks(conn: sqlite3.Connection, session_id: int, titles: list[str]) -> None:
    created = now()
    conn.executemany(
        "INSERT INTO task (session_id, title, position, source, created_at)"
        " VALUES (?, ?, ?, 'gate', ?)",
        [(session_id, title, pos, created) for pos, title in enumerate(titles, start=1)],
    )


def get_open_sessions(conn: sqlite3.Connection) -> list[sqlite3.Row]:
    return conn.execute(
        "SELECT * FROM session WHERE ended_at IS NULL AND close_reason IS NULL"
        " ORDER BY id"
    ).fetchall()


def mark_recovered(conn: sqlite3.Connection, session_id: int) -> None:
    # The desktop app's heartbeat is the best estimate of when the session
    # really ended; without one the end time stays honestly unknown.
    with conn:
        conn.execute(
            "UPDATE session SET close_reason = 'recovered',"
            " ended_at = COALESCE(ended_at, last_heartbeat) WHERE id = ?",
            (session_id,),
        )


def close_session(conn: sqlite3.Connection, session_id: int) -> bool:
    """Stamp the end of a session. Idempotent: a second close (e.g. `gate
    close` racing a still-waiting handoff gate) is a no-op.
    """
    with conn:
        cur = conn.execute(
            "UPDATE session SET ended_at = ?, close_reason = 'clean'"
            " WHERE id = ? AND ended_at IS NULL",
            (now(), session_id),
        )
    return cur.rowcount > 0


def carry_unfinished(conn: sqlite3.Connection, session_id: int) -> int:
    """Copy the session's still-unfinished tasks into the backlog.

    New rows, not moves: a session row is immutable history. Runs after the
    debrief so what the user just resolved doesn't carry. Safe to repeat —
    the task_carried_once index makes a second carry an ignored no-op.
    """
    with conn:
        rows = conn.execute(
            f"SELECT id, title FROM task WHERE session_id = ?"
            f" AND status IN {UNFINISHED} ORDER BY position",
            (session_id,),
        ).fetchall()
        position = conn.execute(
            "SELECT COALESCE(MAX(position), 0) FROM task WHERE session_id IS NULL"
        ).fetchone()[0]
        created = now()
        carried = 0
        for row in rows:
            position += 1
            cur = conn.execute(
                "INSERT OR IGNORE INTO task"
                " (session_id, title, position, status, source, carried_from, created_at)"
                " VALUES (NULL, ?, ?, 'planned', 'gate', ?, ?)",
                (row["title"], position, row["id"], created),
            )
            carried += cur.rowcount
    return carried


def get_backlog(conn: sqlite3.Connection) -> list[sqlite3.Row]:
    return conn.execute(
        "SELECT * FROM task WHERE session_id IS NULL ORDER BY position"
    ).fetchall()


def carry_count(conn: sqlite3.Connection, task_id: int) -> int:
    """How many sessions this task has been carried through (its ancestry)."""
    return conn.execute(
        """
        WITH RECURSIVE chain(id) AS (
            SELECT carried_from FROM task WHERE id = ? AND carried_from IS NOT NULL
            UNION ALL
            SELECT t.carried_from FROM task t
            JOIN chain c ON t.id = c.id WHERE t.carried_from IS NOT NULL
        )
        SELECT COUNT(*) FROM chain
        """,
        (task_id,),
    ).fetchone()[0]


def pull_from_backlog(
    conn: sqlite3.Connection, session_id: int, task_ids: list[int]
) -> None:
    with conn:
        _pull_from_backlog(conn, session_id, task_ids)


def _pull_from_backlog(conn, session_id: int, task_ids: list[int]) -> None:
    # A plain move: backlog rows have no session history to protect. The
    # session_id IS NULL guard makes pulling a non-backlog row impossible.
    position = conn.execute(
        "SELECT COALESCE(MAX(position), 0) FROM task WHERE session_id = ?",
        (session_id,),
    ).fetchone()[0]
    for task_id in task_ids:
        position += 1
        conn.execute(
            "UPDATE task SET session_id = ?, position = ?"
            " WHERE id = ? AND session_id IS NULL",
            (session_id, position, task_id),
        )


def heartbeat(conn: sqlite3.Connection, session_id: int) -> bool:
    """Record that the session is still alive. Refuses closed sessions, so a
    desktop app that missed the close can't resurrect one. Returns whether
    the session is still open.
    """
    with conn:
        cur = conn.execute(
            "UPDATE session SET last_heartbeat = ? WHERE id = ? AND ended_at IS NULL",
            (now(), session_id),
        )
    return cur.rowcount > 0


def get_session(conn: sqlite3.Connection, session_id: int) -> sqlite3.Row:
    return conn.execute(
        "SELECT * FROM session WHERE id = ?", (session_id,)
    ).fetchone()


def get_tasks(conn: sqlite3.Connection, session_id: int) -> list[sqlite3.Row]:
    return conn.execute(
        "SELECT * FROM task WHERE session_id = ? ORDER BY position", (session_id,)
    ).fetchall()


def resolve_task(conn: sqlite3.Connection, task_id: int, status: str) -> None:
    ts = now()
    with conn:
        if status == "doing":
            # started_at records the FIRST entry into doing; re-entering keeps it.
            conn.execute(
                "UPDATE task SET status = 'doing',"
                " started_at = COALESCE(started_at, ?), resolved_at = NULL"
                " WHERE id = ?",
                (ts, task_id),
            )
        elif status == "planned":
            conn.execute(
                "UPDATE task SET status = 'planned', resolved_at = NULL WHERE id = ?",
                (task_id,),
            )
        else:  # done / dropped
            conn.execute(
                "UPDATE task SET status = ?, resolved_at = ? WHERE id = ?",
                (status, ts, task_id),
            )


def latest_open_session(conn: sqlite3.Connection) -> sqlite3.Row | None:
    rows = get_open_sessions(conn)
    return rows[-1] if rows else None


def add_analysis(
    conn: sqlite3.Connection,
    session_id: int,
    window_start: str,
    window_end: str,
    headline: str,
    alignment: int | None,
    body: str,
    observed_json: str,
) -> int:
    with conn:
        cur = conn.execute(
            "INSERT INTO analysis (session_id, created_at, window_start,"
            " window_end, headline, alignment, body, observed_json)"
            " VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            (session_id, now(), window_start, window_end, headline, alignment,
             body, observed_json),
        )
    return cur.lastrowid


def get_analyses(conn: sqlite3.Connection, session_id: int) -> list[sqlite3.Row]:
    return conn.execute(
        "SELECT * FROM analysis WHERE session_id = ? ORDER BY id", (session_id,)
    ).fetchall()


def mark_analysis_seen(conn: sqlite3.Connection, analysis_id: int) -> None:
    with conn:
        conn.execute(
            "UPDATE analysis SET seen_at = ? WHERE id = ? AND seen_at IS NULL",
            (now(), analysis_id),
        )


def get_setting(conn: sqlite3.Connection, key: str, default: str | None = None) -> str | None:
    # Settings share the meta table so both writers (gate, desktop app) see
    # them without a second config mechanism.
    row = conn.execute("SELECT value FROM meta WHERE key = ?", (key,)).fetchone()
    return row[0] if row else default


def set_setting(conn: sqlite3.Connection, key: str, value: str) -> None:
    with conn:
        conn.execute(
            "INSERT INTO meta (key, value) VALUES (?, ?)"
            " ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            (key, value),
        )
