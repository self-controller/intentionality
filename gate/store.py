"""The session/task store. Only module that imports sqlite3."""

import json
import sqlite3
from datetime import date, datetime, timedelta, timezone
from pathlib import Path

from . import config

SCHEMA_PATH = Path(__file__).parent / "schema.sql"
SCHEMA_VERSION = 8

# Statuses that mean "still open" — what the debrief asks about and the
# close paths carry into the backlog.
UNFINISHED = ("planned", "doing")

# The desktop app's palette (LABEL_COLORS in app/src-tauri/src/db.rs), picked
# the same way, so a label is the same colour whichever program made it. A
# test keeps the two lists equal.
LABEL_COLORS = (
    "#7aa2f7", "#9ece6a", "#e0af68", "#f7768e", "#bb9af7", "#2ac3de", "#ff9e64", "#41a6b5",
)


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
    if version > SCHEMA_VERSION:
        raise RuntimeError(
            f"store is schema v{version}, this code understands up to v{SCHEMA_VERSION}"
        )
    # Sequential, not exclusive: a v1 store has to walk all the way up.
    if version < 2:
        _backup(conn, suffix=f".v{version}.bak")
        _migrate_v1_to_v2(conn)
    if version < 3:
        _backup(conn, suffix=".pre-v3.bak")
        _migrate_v2_to_v3(conn)
    if version < 4:
        _backup(conn, suffix=".pre-v4.bak")
        _migrate_v3_to_v4(conn)
    if version < 5:
        _backup(conn, suffix=".pre-v5.bak")
        _migrate_v4_to_v5(conn)
    if version < 6:
        _backup(conn, suffix=".pre-v6.bak")
        _migrate_v5_to_v6(conn)
    if version < 7:
        _backup(conn, suffix=".pre-v7.bak")
        _migrate_v6_to_v7(conn)
    if version < 8:
        _backup(conn, suffix=".pre-v8.bak")
        _migrate_v7_to_v8(conn)


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


def _migrate_v2_to_v3(conn: sqlite3.Connection) -> None:
    """v2 -> v3: session.checkpoint_due_at, and analysis gains kind plus the
    two recommendation columns.

    session takes a plain ADD COLUMN. analysis has to be rebuilt: `kind`
    carries a CHECK constraint and SQLite cannot ALTER one in. Same shape as
    _migrate_v1_to_v2 -- foreign_keys OFF outside the transaction (it cannot
    change mid-transaction), one IMMEDIATE transaction under manual control.
    """
    saved_isolation = conn.isolation_level
    conn.isolation_level = None  # autocommit: we manage the transaction
    conn.execute("PRAGMA foreign_keys = OFF")
    try:
        conn.execute("BEGIN IMMEDIATE")
        conn.execute("ALTER TABLE session ADD COLUMN checkpoint_due_at TEXT")
        # Sessions still running when the migration lands get the checkpoint
        # they would have been given at commit. strftime, not datetime():
        # datetime() returns "YYYY-MM-DD HH:MM:SS" with no offset, and every
        # other timestamp in this store is now()'s isoformat.
        conn.execute(
            "UPDATE session"
            " SET checkpoint_due_at = strftime('%Y-%m-%dT%H:%M:%S+00:00',"
            "     started_at, '+' || intended_minutes || ' minutes')"
            " WHERE ended_at IS NULL AND intended_minutes IS NOT NULL"
        )
        conn.execute(
            """
            CREATE TABLE analysis_v3 (
                id                  INTEGER PRIMARY KEY,
                session_id          INTEGER NOT NULL REFERENCES session(id),
                created_at          TEXT NOT NULL,
                window_start        TEXT NOT NULL,
                window_end          TEXT NOT NULL,
                headline            TEXT NOT NULL,
                alignment           INTEGER,
                body                TEXT NOT NULL,
                observed_json       TEXT NOT NULL,
                seen_at             TEXT,
                kind                TEXT NOT NULL DEFAULT 'check'
                                    CHECK (kind IN ('check', 'checkpoint')),
                recommendation_id   TEXT,
                recommendation_note TEXT
            )
            """
        )
        conn.execute(
            "INSERT INTO analysis_v3 (id, session_id, created_at, window_start,"
            " window_end, headline, alignment, body, observed_json, seen_at)"
            " SELECT id, session_id, created_at, window_start, window_end,"
            " headline, alignment, body, observed_json, seen_at FROM analysis"
        )
        conn.execute("DROP TABLE analysis")
        conn.execute("ALTER TABLE analysis_v3 RENAME TO analysis")
        violations = conn.execute("PRAGMA foreign_key_check").fetchall()
        if violations:
            raise RuntimeError(f"migration broke foreign keys: {violations[:3]}")
        conn.execute("UPDATE meta SET value = '3' WHERE key = 'schema_version'")
        conn.execute("COMMIT")
    except BaseException:
        conn.execute("ROLLBACK")
        raise
    finally:
        conn.execute("PRAGMA foreign_keys = ON")
        conn.isolation_level = saved_isolation


def _migrate_v3_to_v4(conn: sqlite3.Connection) -> None:
    """v3 -> v4: the meeting note taker's three tables, and task.source gains
    'meeting' so an approved action item can become a backlog task.

    task has to be rebuilt rather than altered: `source` carries a CHECK and
    SQLite cannot ALTER one in -- the same wall _migrate_v2_to_v3 hit on
    analysis.kind. Dropping the table drops its indexes too, so
    task_carried_once is recreated by hand; without it the double-carry
    guarantee would silently disappear.

    The rebuild runs first so meeting_action's task_id references the final
    table. Same shape as the migrations above: foreign_keys OFF outside the
    transaction (it cannot change mid-transaction), one IMMEDIATE transaction
    under manual control.
    """
    saved_isolation = conn.isolation_level
    conn.isolation_level = None  # autocommit: we manage the transaction
    conn.execute("PRAGMA foreign_keys = OFF")
    try:
        conn.execute("BEGIN IMMEDIATE")
        conn.execute(
            """
            CREATE TABLE task_v4 (
                id           INTEGER PRIMARY KEY,
                session_id   INTEGER REFERENCES session(id),
                title        TEXT NOT NULL,
                position     INTEGER NOT NULL,
                status       TEXT NOT NULL DEFAULT 'planned'
                             CHECK (status IN ('planned', 'doing', 'done', 'dropped')),
                source       TEXT NOT NULL DEFAULT 'gate'
                             CHECK (source IN ('gate', 'mid-session', 'meeting')),
                carried_from INTEGER REFERENCES task(id) ON DELETE SET NULL,
                created_at   TEXT NOT NULL,
                started_at   TEXT,
                resolved_at  TEXT
            )
            """
        )
        conn.execute(
            "INSERT INTO task_v4 (id, session_id, title, position, status, source,"
            " carried_from, created_at, started_at, resolved_at)"
            " SELECT id, session_id, title, position, status, source,"
            " carried_from, created_at, started_at, resolved_at FROM task"
        )
        conn.execute("DROP TABLE task")
        conn.execute("ALTER TABLE task_v4 RENAME TO task")
        conn.execute(
            "CREATE UNIQUE INDEX task_carried_once"
            " ON task (carried_from) WHERE carried_from IS NOT NULL"
        )
        conn.execute(
            """
            CREATE TABLE meeting (
                id           INTEGER PRIMARY KEY,
                session_id   INTEGER REFERENCES session(id),
                started_at   TEXT NOT NULL,
                ended_at     TEXT,
                title        TEXT NOT NULL DEFAULT '',
                summary      TEXT,
                key_points   TEXT,
                state        TEXT NOT NULL DEFAULT 'recording'
                             CHECK (state IN ('recording', 'summarizing', 'done', 'failed')),
                error        TEXT
            )
            """
        )
        conn.execute(
            """
            CREATE TABLE meeting_segment (
                id         INTEGER PRIMARY KEY,
                meeting_id INTEGER NOT NULL REFERENCES meeting(id) ON DELETE CASCADE,
                seq        INTEGER NOT NULL,
                started_at TEXT NOT NULL,
                text       TEXT NOT NULL
            )
            """
        )
        conn.execute(
            "CREATE UNIQUE INDEX meeting_segment_seq ON meeting_segment (meeting_id, seq)"
        )
        conn.execute(
            """
            CREATE TABLE meeting_action (
                id         INTEGER PRIMARY KEY,
                meeting_id INTEGER NOT NULL REFERENCES meeting(id) ON DELETE CASCADE,
                position   INTEGER NOT NULL,
                text       TEXT NOT NULL,
                task_id    INTEGER REFERENCES task(id) ON DELETE SET NULL
            )
            """
        )
        violations = conn.execute("PRAGMA foreign_key_check").fetchall()
        if violations:
            raise RuntimeError(f"migration broke foreign keys: {violations[:3]}")
        conn.execute("UPDATE meta SET value = '4' WHERE key = 'schema_version'")
        conn.execute("COMMIT")
    except BaseException:
        conn.execute("ROLLBACK")
        raise
    finally:
        conn.execute("PRAGMA foreign_keys = ON")
        conn.isolation_level = saved_isolation


def _v5_key_points(raw: str | None) -> str:
    """v4's flat ["a", "b"] -> v5's [{"text": "a", "subpoints": []}, ...].

    Deliberately forgiving. This runs once, unattended, over rows written by
    an older binary, and the only thing worse than losing a key point is
    refusing to migrate the store at all: anything that is not recognisably a
    list of strings becomes [], which is exactly what the UI already renders
    for a meeting that was never summarized.
    """
    if raw is None:
        return "[]"
    try:
        points = json.loads(raw)
    except (ValueError, TypeError):
        return "[]"
    if not isinstance(points, list):
        return "[]"
    converted = []
    for point in points:
        if isinstance(point, str) and point.strip():
            converted.append({"text": point.strip(), "subpoints": []})
        elif isinstance(point, dict) and isinstance(point.get("text"), str):
            # Already v5-shaped: idempotent, so a half-applied migration that
            # was rolled back and retried cannot double-wrap.
            subpoints = point.get("subpoints")
            converted.append({
                "text": point["text"],
                "subpoints": [s for s in subpoints if isinstance(s, str)]
                if isinstance(subpoints, list)
                else [],
            })
    return json.dumps(converted)


def _migrate_v4_to_v5(conn: sqlite3.Connection) -> None:
    """v4 -> v5: the two-pass meeting pipeline, plus attached context files.

    meeting gains notes (what the user typed), clean_transcript (the repair
    pass's output) and details, and `state` gains 'cleaning' -- which is a
    CHECK change, so the table is rebuilt rather than altered, the same wall
    _migrate_v3_to_v4 hit on task.source. meeting has no indexes of its own so
    there is nothing to recreate by hand this time; meeting_file's index is new.

    key_points is converted in the same transaction, from a flat array of
    strings to objects carrying subpoints. Row conversion belongs here and
    nowhere else: the desktop app must never find a shape it cannot read.

    meeting_file is created now even though only the desktop app's attachment
    feature writes it, so the store crosses this version boundary once.
    """
    saved_isolation = conn.isolation_level
    conn.isolation_level = None  # autocommit: we manage the transaction
    conn.execute("PRAGMA foreign_keys = OFF")
    try:
        conn.execute("BEGIN IMMEDIATE")
        conn.execute(
            """
            CREATE TABLE meeting_v5 (
                id           INTEGER PRIMARY KEY,
                session_id   INTEGER REFERENCES session(id),
                started_at   TEXT NOT NULL,
                ended_at     TEXT,
                title        TEXT NOT NULL DEFAULT '',
                notes        TEXT NOT NULL DEFAULT '',
                clean_transcript TEXT,
                summary      TEXT,
                key_points   TEXT,
                details      TEXT,
                state        TEXT NOT NULL DEFAULT 'recording'
                             CHECK (state IN ('recording', 'cleaning', 'summarizing',
                                              'done', 'failed')),
                error        TEXT
            )
            """
        )
        # Read before the drop, convert in Python, write back. The row count
        # here is meetings-ever-recorded, so materialising them is fine.
        rows = conn.execute(
            "SELECT id, session_id, started_at, ended_at, title, summary,"
            " key_points, state, error FROM meeting"
        ).fetchall()
        conn.executemany(
            "INSERT INTO meeting_v5 (id, session_id, started_at, ended_at, title,"
            " notes, clean_transcript, summary, key_points, details, state, error)"
            " VALUES (?, ?, ?, ?, ?, '', NULL, ?, ?, NULL, ?, ?)",
            # Positional, not by name: this must not depend on the caller
            # having set a row_factory.
            [
                (
                    row[0],  # id
                    row[1],  # session_id
                    row[2],  # started_at
                    row[3],  # ended_at
                    row[4],  # title
                    row[5],  # summary
                    _v5_key_points(row[6]),  # key_points
                    row[7],  # state
                    row[8],  # error
                )
                for row in rows
            ],
        )
        conn.execute("DROP TABLE meeting")
        conn.execute("ALTER TABLE meeting_v5 RENAME TO meeting")
        conn.execute(
            """
            CREATE TABLE meeting_file (
                id         INTEGER PRIMARY KEY,
                meeting_id INTEGER NOT NULL REFERENCES meeting(id) ON DELETE CASCADE,
                position   INTEGER NOT NULL,
                name       TEXT NOT NULL,
                path       TEXT NOT NULL,
                kind       TEXT NOT NULL CHECK (kind IN ('text', 'pdf', 'image', 'office')),
                bytes      INTEGER NOT NULL,
                extracted  TEXT,
                added_at   TEXT NOT NULL
            )
            """
        )
        conn.execute(
            "CREATE INDEX meeting_file_meeting ON meeting_file (meeting_id, position)"
        )
        violations = conn.execute("PRAGMA foreign_key_check").fetchall()
        if violations:
            raise RuntimeError(f"migration broke foreign keys: {violations[:3]}")
        conn.execute("UPDATE meta SET value = '5' WHERE key = 'schema_version'")
        conn.execute("COMMIT")
    except BaseException:
        conn.execute("ROLLBACK")
        raise
    finally:
        conn.execute("PRAGMA foreign_keys = ON")
        conn.isolation_level = saved_isolation


def _v6_document(
    summary: str | None, key_points: str | None, details: str | None
) -> str | None:
    """v5's three write-up columns -> v6's one markdown document.

    The paragraph first with no heading, then the key points as a bullet list
    with their subpoints nested under them, then the details under their own
    heading -- the same shape the model is now asked to write directly, so a
    migrated meeting and a new one read the same on screen.

    As forgiving as _v5_key_points, and for the same reason: this runs once,
    unattended, over rows an older binary wrote, and junk in key_points is
    dropped rather than refusing the migration. None when all three are
    empty, so a meeting that was never summarized stays `summary IS NULL`,
    which is what the app reads as "no notes yet".
    """
    parts = []
    if summary and summary.strip():
        parts.append(summary.strip())
    lines = []
    for point in json.loads(_v5_key_points(key_points)):  # tolerant, v5-shaped
        text = point["text"].strip()
        if not text:
            continue
        lines.append(f"- {text}")
        lines.extend(f"  - {s.strip()}" for s in point["subpoints"] if s.strip())
    if lines:
        parts.append("## Key points\n" + "\n".join(lines))
    if details and details.strip():
        parts.append("## Additional information\n" + details.strip())
    return "\n\n".join(parts) if parts else None


def _migrate_v5_to_v6(conn: sqlite3.Connection) -> None:
    """v5 -> v6: the write-up becomes one editable markdown document.

    summary absorbs key_points and details (folded by _v6_document, in the
    same transaction), those two columns go, and summary_edited_at arrives to
    record that the user has changed the document since the model wrote it.

    No CHECK changes this time, so no table rebuild: ALTER TABLE does all of
    it. DROP COLUMN needs SQLite 3.35 (2021). ADD COLUMN appends, which is why
    schema.sql lists summary_edited_at last -- the parity test compares column
    order.
    """
    saved_isolation = conn.isolation_level
    conn.isolation_level = None  # autocommit: we manage the transaction
    try:
        conn.execute("BEGIN IMMEDIATE")
        rows = conn.execute(
            "SELECT id, summary, key_points, details FROM meeting"
        ).fetchall()
        conn.executemany(
            "UPDATE meeting SET summary = ? WHERE id = ?",
            # Positional: this must not depend on the caller's row_factory.
            [(_v6_document(row[1], row[2], row[3]), row[0]) for row in rows],
        )
        conn.execute("ALTER TABLE meeting DROP COLUMN key_points")
        conn.execute("ALTER TABLE meeting DROP COLUMN details")
        conn.execute("ALTER TABLE meeting ADD COLUMN summary_edited_at TEXT")
        conn.execute("UPDATE meta SET value = '6' WHERE key = 'schema_version'")
        conn.execute("COMMIT")
    except BaseException:
        conn.execute("ROLLBACK")
        raise
    finally:
        conn.isolation_level = saved_isolation


def _migrate_v6_to_v7(conn: sqlite3.Connection) -> None:
    """v6 -> v7: a task becomes an editable object -- notes and labels.

    task.notes is plain text the user writes on a card. Labels are free text
    shared across tasks, so they are their own table with a join rather than a
    column: reusing "billing" on a second card has to mean the same label.

    No CHECK changes, so no rebuild -- ALTER TABLE ADD COLUMN does it, and it
    appends, which is why schema.sql lists notes last (the parity test compares
    column order). The two new tables are created inside the same transaction.
    """
    saved_isolation = conn.isolation_level
    conn.isolation_level = None  # autocommit: we manage the transaction
    try:
        conn.execute("BEGIN IMMEDIATE")
        conn.execute("ALTER TABLE task ADD COLUMN notes TEXT NOT NULL DEFAULT ''")
        conn.execute(
            "CREATE TABLE label ("
            " id    INTEGER PRIMARY KEY,"
            " name  TEXT NOT NULL COLLATE NOCASE UNIQUE,"
            " color TEXT NOT NULL)"
        )
        conn.execute(
            "CREATE TABLE task_label ("
            " task_id  INTEGER NOT NULL REFERENCES task(id) ON DELETE CASCADE,"
            " label_id INTEGER NOT NULL REFERENCES label(id) ON DELETE CASCADE,"
            " PRIMARY KEY (task_id, label_id))"
        )
        conn.execute("UPDATE meta SET value = '7' WHERE key = 'schema_version'")
        conn.execute("COMMIT")
    except BaseException:
        conn.execute("ROLLBACK")
        raise
    finally:
        conn.isolation_level = saved_isolation


def _migrate_v7_to_v8(conn: sqlite3.Connection) -> None:
    """v7 -> v8: task.due_date, the day a task is due. NULL = none.

    One nullable column, no CHECK, so ALTER TABLE ADD COLUMN is the whole
    migration -- the _migrate_v6_to_v7 idiom. It appends, which is why
    schema.sql lists due_date after notes. Both writers (the app's
    db::update_task, the gate's _set_details) accept the canonical form only;
    every existing task starts with none.
    """
    saved_isolation = conn.isolation_level
    conn.isolation_level = None  # autocommit: we manage the transaction
    try:
        conn.execute("BEGIN IMMEDIATE")
        conn.execute("ALTER TABLE task ADD COLUMN due_date TEXT")
        conn.execute("UPDATE meta SET value = '8' WHERE key = 'schema_version'")
        conn.execute("COMMIT")
    except BaseException:
        conn.execute("ROLLBACK")
        raise
    finally:
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


class EmptyPlan(ValueError):
    """A session would have started with no tasks. The gate never allows it."""


def commit_plan(
    conn: sqlite3.Connection,
    intended_minutes: int | None,
    keep_ids: list[int],
    titles: list[str],
    done_ids: list[int] = (),
    delete_ids: list[int] = (),
    new_details: list = (),
    edits: dict | None = None,
) -> int:
    """Start a session from the welcome screen: finish, delete, pull, add.

    One transaction, so the screen's answer lands whole or not at all. The
    session starts with the kept backlog tasks, in order, then the typed
    ones. If that comes to nothing -- the desktop app pulled or deleted every
    kept task while the gate was up, and nothing was typed -- EmptyPlan
    rolls the lot back, done and deletes included.

    `new_details` runs alongside `titles`; `edits` maps a kept task's id to
    its changed details. A details value is anything with notes, due_date
    and labels (ui.Details). An edit to a task that did not end up in this
    session -- the app took it meanwhile -- is dropped with it.
    """
    with conn:
        for task_id in done_ids:
            _finish_backlog_task(conn, task_id)
        for task_id in delete_ids:
            _delete_backlog_task(conn, task_id)
        session_id = create_session(conn, "", intended_minutes, "manual")
        _pull_from_backlog(conn, session_id, keep_ids)
        for task_id, details in (edits or {}).items():
            _set_details(conn, task_id, session_id, details)
        new_ids = add_tasks(conn, session_id, titles)
        for task_id, details in zip(new_ids, new_details):
            _set_details(conn, task_id, session_id, details)
        if conn.execute(
            "SELECT 1 FROM task WHERE session_id = ?", (session_id,)
        ).fetchone() is None:
            raise EmptyPlan("a session needs at least one task")
    return session_id


def _delete_backlog_task(conn, task_id: int) -> None:
    # The same statement as the app's backlog delete (db.rs
    # delete_backlog_task): labels go with it by cascade, an approved meeting
    # action reads as unapproved again. Session rows are history: the guard
    # makes deleting one impossible.
    conn.execute("DELETE FROM task WHERE id = ? AND session_id IS NULL", (task_id,))


def _finish_backlog_task(conn, task_id: int) -> None:
    """Done, for a backlog copy: the session row it was carried from is
    marked done -- that is where the work was supposed to happen -- and the
    copy goes. A row carried from nowhere has no session to record it in, so
    it is left alone; the welcome screen offers Done only on carried rows."""
    row = conn.execute(
        "SELECT carried_from FROM task WHERE id = ? AND session_id IS NULL",
        (task_id,),
    ).fetchone()
    if row is None or row["carried_from"] is None:
        return
    conn.execute(
        f"UPDATE task SET status = 'done', resolved_at = ?"
        f" WHERE id = ? AND session_id IS NOT NULL AND status IN {UNFINISHED}",
        (now(), row["carried_from"]),
    )
    _delete_backlog_task(conn, task_id)


def _set_details(conn, task_id: int, session_id: int | None, details) -> bool:
    """A task's notes, due date and exact set of labels, as the app's
    db::write_details writes them. Labels are found by name, case-blind
    (label.name is NOCASE), and a name that isn't there yet is created. Like
    the app, it never deletes a label: one with no task left is a preset.

    Guarded on session_id (None = the backlog), so only a row where the
    caller expects it is touched.
    """
    due = details.due_date
    if due is not None and not _is_day(due):
        raise ValueError(f"due date must be YYYY-MM-DD, not {due!r}")
    cur = conn.execute(
        "UPDATE task SET notes = ?, due_date = ? WHERE id = ? AND session_id IS ?",
        (details.notes.strip(), due, task_id, session_id),
    )
    if not cur.rowcount:
        return False
    conn.execute("DELETE FROM task_label WHERE task_id = ?", (task_id,))
    for name in details.labels:
        name = name.strip()
        if not name:
            continue
        count = conn.execute("SELECT COUNT(*) FROM label").fetchone()[0]
        conn.execute(
            "INSERT OR IGNORE INTO label (name, color) VALUES (?, ?)",
            (name, LABEL_COLORS[count % len(LABEL_COLORS)]),
        )
        conn.execute(
            "INSERT OR IGNORE INTO task_label (task_id, label_id)"
            " SELECT ?, id FROM label WHERE name = ?",
            (task_id, name),
        )
    return True


def _is_day(text: str) -> bool:
    # The fixed width is what makes the column's string order date order.
    try:
        return date.fromisoformat(text).isoformat() == text
    except ValueError:
        return False


def list_labels(conn: sqlite3.Connection) -> list[sqlite3.Row]:
    """Every label, worn or not, by name."""
    return conn.execute("SELECT name, color FROM label ORDER BY name").fetchall()


def labels_by_task(conn: sqlite3.Connection) -> dict[int, list[str]]:
    """Label names per task id, each list by name. One query for the whole
    store, which is small by design (the app's attach_labels does the same)."""
    out: dict[int, list[str]] = {}
    for row in conn.execute(
        "SELECT tl.task_id, l.name FROM task_label tl"
        " JOIN label l ON l.id = tl.label_id ORDER BY l.name"
    ):
        out.setdefault(row["task_id"], []).append(row["name"])
    return out


def create_session(
    conn: sqlite3.Connection, statement: str, intended_minutes: int | None, mode: str
) -> int:
    started_at = now()
    cur = conn.execute(
        "INSERT INTO session (started_at, statement, intended_minutes, mode,"
        " checkpoint_due_at) VALUES (?, ?, ?, ?, ?)",
        (started_at, statement, intended_minutes, mode,
         checkpoint_due(started_at, intended_minutes)),
    )
    return cur.lastrowid


def checkpoint_due(started_at: str, intended_minutes: int | None) -> str | None:
    """When the time-up checkpoint should fire, or None for an open-ended
    session. A NULL intended_minutes is the user saying "don't hold me to a
    clock", so it earns no checkpoint."""
    if intended_minutes is None:
        return None
    start = datetime.fromisoformat(started_at)
    return (start + timedelta(minutes=intended_minutes)).isoformat(timespec="seconds")


def add_tasks(conn: sqlite3.Connection, session_id: int, titles: list[str]) -> list[int]:
    """Insert the titles into the session, returning their ids in order."""
    # Numbered after whatever the session already holds, so tasks added
    # after a pull land below the pulled ones.
    start = conn.execute(
        "SELECT COALESCE(MAX(position), 0) FROM task WHERE session_id = ?",
        (session_id,),
    ).fetchone()[0]
    created = now()
    return [
        conn.execute(
            "INSERT INTO task (session_id, title, position, source, created_at)"
            " VALUES (?, ?, ?, 'gate', ?)",
            (session_id, title, pos, created),
        ).lastrowid
        for pos, title in enumerate(titles, start=start + 1)
    ]


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
            f"SELECT id, title, notes, due_date FROM task WHERE session_id = ?"
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
                " (session_id, title, position, status, source, carried_from,"
                "  created_at, notes, due_date)"
                " VALUES (NULL, ?, ?, 'planned', 'gate', ?, ?, ?, ?)",
                (row["title"], position, row["id"], created, row["notes"],
                 row["due_date"]),
            )
            carried += cur.rowcount
            if cur.rowcount:
                # The labels come along with the copy. Guarded on rowcount so a
                # second carry stays the no-op task_carried_once makes it.
                conn.execute(
                    "INSERT OR IGNORE INTO task_label (task_id, label_id)"
                    " SELECT ?, label_id FROM task_label WHERE task_id = ?",
                    (cur.lastrowid, row["id"]),
                )
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


def session_label(conn: sqlite3.Connection, session: sqlite3.Row) -> str:
    """What identifies a session in a listing.

    Its statement if it has one — sessions from before the gate stopped
    asking for one do — and otherwise its tasks, which are the intention now.
    """
    if session["statement"]:
        return session["statement"]
    titles = conn.execute(
        "SELECT title FROM task WHERE session_id = ? ORDER BY position",
        (session["id"],),
    ).fetchall()
    if not titles:
        return "(no tasks)"
    more = f" +{len(titles) - 1}" if len(titles) > 1 else ""
    return f"{titles[0]['title']}{more}"


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
