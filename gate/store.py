"""The session/task store. Only module that imports sqlite3."""

import sqlite3
from datetime import datetime, timezone
from pathlib import Path

from . import config

SCHEMA_PATH = Path(__file__).parent / "schema.sql"


def now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def connect() -> sqlite3.Connection:
    config.STORE_PATH.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(config.STORE_PATH)
    conn.row_factory = sqlite3.Row
    # foreign_keys is per-connection in SQLite, so it can't live in schema.sql
    conn.execute("PRAGMA foreign_keys = ON")
    return conn


def init(conn: sqlite3.Connection) -> None:
    exists = conn.execute(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'meta'"
    ).fetchone()
    if not exists:
        conn.executescript(SCHEMA_PATH.read_text())


def commit_draft(
    conn: sqlite3.Connection,
    statement: str,
    intended_minutes: int | None,
    mode: str,
    titles: list[str],
) -> int:
    # One transaction: the session and its tasks land together or not at all.
    with conn:
        session_id = create_session(conn, statement, intended_minutes, mode)
        add_tasks(conn, session_id, titles)
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
    # ended_at stays NULL: the real end time is unknown until heartbeats exist.
    with conn:
        conn.execute(
            "UPDATE session SET close_reason = 'recovered' WHERE id = ?", (session_id,)
        )


def close_session(conn: sqlite3.Connection, session_id: int) -> None:
    with conn:
        conn.execute(
            "UPDATE session SET ended_at = ?, close_reason = 'clean' WHERE id = ?",
            (now(), session_id),
        )


def get_session(conn: sqlite3.Connection, session_id: int) -> sqlite3.Row:
    return conn.execute(
        "SELECT * FROM session WHERE id = ?", (session_id,)
    ).fetchone()


def get_tasks(conn: sqlite3.Connection, session_id: int) -> list[sqlite3.Row]:
    return conn.execute(
        "SELECT * FROM task WHERE session_id = ? ORDER BY position", (session_id,)
    ).fetchall()


def resolve_task(conn: sqlite3.Connection, task_id: int, status: str) -> None:
    with conn:
        conn.execute(
            "UPDATE task SET status = ?, resolved_at = ? WHERE id = ?",
            (status, now(), task_id),
        )


def latest_open_session(conn: sqlite3.Connection) -> sqlite3.Row | None:
    rows = get_open_sessions(conn)
    return rows[-1] if rows else None
