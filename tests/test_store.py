"""Data-layer tests: migration, carry, close idempotency, backlog pulls.

The UI is allowed to be bare-bones; this file is why the data underneath
isn't. Run with:  python3 -m unittest discover tests
"""

import sqlite3
import tempfile
import unittest
from datetime import datetime, timedelta
from pathlib import Path

from gate import config, store

# The v1 schema exactly as shipped, for migration fixtures.
V1_SCHEMA = """
PRAGMA journal_mode = WAL;
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
INSERT INTO meta (key, value) VALUES ('schema_version', '1');
CREATE TABLE session (
    id               INTEGER PRIMARY KEY,
    started_at       TEXT NOT NULL,
    ended_at         TEXT,
    close_reason     TEXT CHECK (close_reason IN ('clean', 'recovered')),
    statement        TEXT NOT NULL DEFAULT '',
    intended_minutes INTEGER,
    mode             TEXT NOT NULL CHECK (mode IN ('ai', 'manual'))
);
CREATE TABLE task (
    id          INTEGER PRIMARY KEY,
    session_id  INTEGER NOT NULL REFERENCES session(id),
    title       TEXT NOT NULL,
    position    INTEGER NOT NULL,
    status      TEXT NOT NULL DEFAULT 'planned'
                CHECK (status IN ('planned', 'done', 'dropped')),
    source      TEXT NOT NULL DEFAULT 'gate'
                CHECK (source IN ('gate', 'mid-session')),
    created_at  TEXT NOT NULL,
    resolved_at TEXT
);
"""


# The v2 schema exactly as shipped, for the v2 -> v3 migration fixture.
V2_SCHEMA = """
PRAGMA journal_mode = WAL;
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
INSERT INTO meta (key, value) VALUES ('schema_version', '2');
CREATE TABLE session (
    id               INTEGER PRIMARY KEY,
    started_at       TEXT NOT NULL,
    ended_at         TEXT,
    close_reason     TEXT CHECK (close_reason IN ('clean', 'recovered')),
    statement        TEXT NOT NULL DEFAULT '',
    intended_minutes INTEGER,
    mode             TEXT NOT NULL CHECK (mode IN ('ai', 'manual')),
    last_heartbeat   TEXT
);
CREATE TABLE task (
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
);
CREATE UNIQUE INDEX task_carried_once
    ON task (carried_from) WHERE carried_from IS NOT NULL;
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
);
"""

# The v3 schema exactly as shipped, for the v3 -> v4 migration fixture.
V3_SCHEMA = """
PRAGMA journal_mode = WAL;
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
INSERT INTO meta (key, value) VALUES ('schema_version', '3');
CREATE TABLE session (
    id                INTEGER PRIMARY KEY,
    started_at        TEXT NOT NULL,
    ended_at          TEXT,
    close_reason      TEXT CHECK (close_reason IN ('clean', 'recovered')),
    statement         TEXT NOT NULL DEFAULT '',
    intended_minutes  INTEGER,
    mode              TEXT NOT NULL CHECK (mode IN ('ai', 'manual')),
    last_heartbeat    TEXT,
    checkpoint_due_at TEXT
);
CREATE TABLE task (
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
);
CREATE UNIQUE INDEX task_carried_once
    ON task (carried_from) WHERE carried_from IS NOT NULL;
CREATE TABLE analysis (
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
);
"""


class StoreCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self._saved_store_path = config.STORE_PATH
        config.STORE_PATH = Path(self._tmp.name) / "store.db"

    def tearDown(self):
        config.STORE_PATH = self._saved_store_path
        self._tmp.cleanup()

    def fresh(self) -> sqlite3.Connection:
        conn = store.connect()
        store.init(conn)
        return conn

    def v1_fixture(self) -> None:
        conn = sqlite3.connect(config.STORE_PATH)
        conn.executescript(V1_SCHEMA)
        with conn:
            conn.execute(
                "INSERT INTO session (id, started_at, ended_at, close_reason,"
                " statement, mode) VALUES (1, '2026-08-20T09:00:00+00:00',"
                " NULL, 'recovered', 'old session', 'ai')"
            )
            conn.execute(
                "INSERT INTO task (id, session_id, title, position, status,"
                " created_at) VALUES (1, 1, 'old task', 1, 'planned',"
                " '2026-08-20T09:00:00+00:00')"
            )
        conn.close()

    def v2_fixture(self) -> None:
        """One closed session, one still-open timed session, one analysis."""
        conn = sqlite3.connect(config.STORE_PATH)
        conn.executescript(V2_SCHEMA)
        with conn:
            conn.execute(
                "INSERT INTO session (id, started_at, ended_at, close_reason,"
                " statement, intended_minutes, mode)"
                " VALUES (1, '2026-08-26T09:00:00+00:00',"
                " '2026-08-26T11:00:00+00:00', 'clean', 'closed one', 60, 'manual')"
            )
            conn.execute(
                "INSERT INTO session (id, started_at, statement,"
                " intended_minutes, mode) VALUES (2, '2026-08-27T09:00:00+00:00',"
                " 'still running', 90, 'manual')"
            )
            conn.execute(
                "INSERT INTO session (id, started_at, statement, mode)"
                " VALUES (3, '2026-08-27T09:00:00+00:00', 'open-ended', 'manual')"
            )
            conn.execute(
                "INSERT INTO task (id, session_id, title, position, created_at)"
                " VALUES (1, 2, 'a task', 1, '2026-08-27T09:00:00+00:00')"
            )
            conn.execute(
                "INSERT INTO analysis (id, session_id, created_at, window_start,"
                " window_end, headline, alignment, body, observed_json)"
                " VALUES (1, 1, '2026-08-26T10:00:00+00:00',"
                " '2026-08-26T09:00:00+00:00', '2026-08-26T10:00:00+00:00',"
                " 'Steady', 80, 'Editor most of the hour.', '{}')"
            )
        conn.close()


    def v3_fixture(self) -> None:
        """A carried task and an analysis: the rows the task rebuild must not lose."""
        conn = sqlite3.connect(config.STORE_PATH)
        conn.executescript(V3_SCHEMA)
        with conn:
            conn.execute(
                "INSERT INTO session (id, started_at, statement, intended_minutes,"
                " mode) VALUES (1, '2026-09-01T09:00:00+00:00', 'a session', 60,"
                " 'manual')"
            )
            conn.execute(
                "INSERT INTO task (id, session_id, title, position, status,"
                " source, created_at) VALUES (1, 1, 'the original', 1, 'planned',"
                " 'mid-session', '2026-09-01T09:00:00+00:00')"
            )
            # A backlog copy carried off task 1 -- the chain and the partial
            # unique index both have to survive the rebuild.
            conn.execute(
                "INSERT INTO task (id, session_id, title, position, carried_from,"
                " created_at) VALUES (2, NULL, 'the original', 1, 1,"
                " '2026-09-01T11:00:00+00:00')"
            )
            conn.execute(
                "INSERT INTO analysis (id, session_id, created_at, window_start,"
                " window_end, headline, alignment, body, observed_json, kind)"
                " VALUES (1, 1, '2026-09-01T10:00:00+00:00',"
                " '2026-09-01T09:00:00+00:00', '2026-09-01T10:00:00+00:00',"
                " 'Steady', 80, 'Editor most of the hour.', '{}', 'checkpoint')"
            )
        conn.close()


class TestInitAndMigration(StoreCase):
    def test_fresh_init_is_v4(self):
        conn = self.fresh()
        version = conn.execute(
            "SELECT value FROM meta WHERE key = 'schema_version'"
        ).fetchone()[0]
        self.assertEqual(version, "4")

    def test_init_idempotent(self):
        conn = self.fresh()
        store.init(conn)  # second run must be a no-op, not a re-create
        self.assertEqual(store.get_setting(conn, "schema_version"), "4")

    def test_migrates_v1_preserving_rows(self):
        self.v1_fixture()
        conn = self.fresh()
        # A v1 store walks all the way up, not just one step.
        self.assertEqual(store.get_setting(conn, "schema_version"), "4")
        session = store.get_session(conn, 1)
        self.assertEqual(session["statement"], "old session")
        self.assertIsNone(session["last_heartbeat"])
        (task,) = store.get_tasks(conn, 1)
        self.assertEqual(task["title"], "old task")
        self.assertIsNone(task["carried_from"])
        self.assertEqual(conn.execute("PRAGMA integrity_check").fetchone()[0], "ok")
        self.assertEqual(conn.execute("PRAGMA foreign_key_check").fetchall(), [])
        # v2 features work on the migrated file
        store.resolve_task(conn, task["id"], "doing")
        self.assertEqual(store.get_tasks(conn, 1)[0]["status"], "doing")

    def test_migration_makes_backup(self):
        self.v1_fixture()
        self.fresh()
        backup = Path(str(config.STORE_PATH) + ".v1.bak")
        self.assertTrue(backup.exists())
        bconn = sqlite3.connect(backup)
        self.assertEqual(  # the backup is still v1
            bconn.execute("SELECT value FROM meta WHERE key='schema_version'").fetchone()[0],
            "1",
        )
        bconn.close()

    def test_migrates_v2_preserving_rows(self):
        self.v2_fixture()
        conn = self.fresh()
        self.assertEqual(store.get_setting(conn, "schema_version"), "4")
        (row,) = store.get_analyses(conn, 1)
        self.assertEqual(row["headline"], "Steady")
        self.assertEqual(row["kind"], "check")  # the default the rebuild gives
        self.assertIsNone(row["recommendation_id"])
        (task,) = store.get_tasks(conn, 2)
        self.assertEqual(task["title"], "a task")
        self.assertEqual(conn.execute("PRAGMA integrity_check").fetchone()[0], "ok")
        self.assertEqual(conn.execute("PRAGMA foreign_key_check").fetchall(), [])

    def test_v3_backfills_open_timed_sessions_only(self):
        self.v2_fixture()
        conn = self.fresh()
        # Closed: history, left alone. Open + timed: gets the checkpoint it
        # would have been given at commit. Open + open-ended: none, ever.
        self.assertIsNone(store.get_session(conn, 1)["checkpoint_due_at"])
        self.assertEqual(
            store.get_session(conn, 2)["checkpoint_due_at"],
            "2026-08-27T10:30:00+00:00",
        )
        self.assertIsNone(store.get_session(conn, 3)["checkpoint_due_at"])

    def test_v3_migration_makes_backup(self):
        self.v2_fixture()
        self.fresh()
        backup = Path(str(config.STORE_PATH) + ".pre-v3.bak")
        self.assertTrue(backup.exists())
        bconn = sqlite3.connect(backup)
        self.assertEqual(  # the backup is still v2
            bconn.execute("SELECT value FROM meta WHERE key='schema_version'").fetchone()[0],
            "2",
        )
        bconn.close()

    def test_migrates_v3_preserving_rows(self):
        self.v3_fixture()
        conn = self.fresh()
        self.assertEqual(store.get_setting(conn, "schema_version"), "4")
        # The task rebuild is the risky half of this migration: everything
        # below was copied out of the old table and back into a new one.
        (task,) = store.get_tasks(conn, 1)
        self.assertEqual(task["title"], "the original")
        self.assertEqual(task["source"], "mid-session")
        carried = conn.execute("SELECT * FROM task WHERE id = 2").fetchone()
        self.assertEqual(carried["carried_from"], 1)
        self.assertIsNone(carried["session_id"])
        (row,) = store.get_analyses(conn, 1)
        self.assertEqual(row["headline"], "Steady")
        self.assertEqual(row["kind"], "checkpoint")
        self.assertEqual(
            store.get_session(conn, 1)["checkpoint_due_at"], None
        )  # v3 already ran; the v4 step must not touch it
        self.assertEqual(conn.execute("PRAGMA integrity_check").fetchone()[0], "ok")
        self.assertEqual(conn.execute("PRAGMA foreign_key_check").fetchall(), [])

    def test_v4_migration_makes_backup(self):
        self.v3_fixture()
        self.fresh()
        backup = Path(str(config.STORE_PATH) + ".pre-v4.bak")
        self.assertTrue(backup.exists())
        bconn = sqlite3.connect(backup)
        self.assertEqual(  # the backup is still v3
            bconn.execute("SELECT value FROM meta WHERE key='schema_version'").fetchone()[0],
            "3",
        )
        bconn.close()

    def test_v4_keeps_the_double_carry_guarantee(self):
        # DROP TABLE takes the table's indexes with it. If the rebuild forgot
        # to recreate task_carried_once, this insert would quietly succeed and
        # a task could be carried into the backlog twice.
        self.v3_fixture()
        conn = self.fresh()
        with self.assertRaises(sqlite3.IntegrityError):
            with conn:
                conn.execute(
                    "INSERT INTO task (session_id, title, position, carried_from,"
                    " created_at) VALUES (NULL, 'second copy', 1, 1, '2026-09-01T12:00:00+00:00')"
                )

    def test_v4_task_source_accepts_meeting_and_still_rejects_junk(self):
        self.v3_fixture()
        conn = self.fresh()
        with conn:
            conn.execute(
                "INSERT INTO task (session_id, title, position, source, created_at)"
                " VALUES (NULL, 'from a meeting', 1, 'meeting', '2026-09-01T12:00:00+00:00')"
            )
        self.assertEqual(
            conn.execute("SELECT source FROM task WHERE title = 'from a meeting'").fetchone()[0],
            "meeting",
        )
        with self.assertRaises(sqlite3.IntegrityError):
            with conn:
                conn.execute(
                    "INSERT INTO task (session_id, title, position, source, created_at)"
                    " VALUES (NULL, 'nope', 1, 'invented', '2026-09-01T12:00:00+00:00')"
                )

    def test_v4_migration_end_state_matches_schema_sql(self):
        """schema.sql promises to describe what the migrations produce.

        Compared through PRAGMA rather than the stored DDL text: a migration
        writes its own formatting and drops the comments, so only columns,
        keys and indexes are the actual contract.
        """
        self.v3_fixture()
        migrated = self.fresh()
        fresh_path = Path(self._tmp.name) / "fresh.db"
        fresh = sqlite3.connect(fresh_path)
        fresh.executescript(store.SCHEMA_PATH.read_text())

        def checks(sql):
            """Every CHECK (...) clause in a CREATE TABLE, paren-matched.

            PRAGMA exposes columns, keys and indexes but not CHECK
            constraints, and a CHECK is exactly what this migration had to
            rebuild the task table to change.
            """
            import re

            sql = re.sub(r"--[^\n]*", " ", sql)
            found = []
            for m in re.finditer(r"\bCHECK\s*\(", sql, re.IGNORECASE):
                depth, i = 0, m.end() - 1
                while i < len(sql):
                    if sql[i] == "(":
                        depth += 1
                    elif sql[i] == ")":
                        depth -= 1
                        if depth == 0:
                            break
                    i += 1
                found.append(re.sub(r"\s+", " ", sql[m.end() : i]).strip())
            return sorted(found)

        def shape(conn):
            out = {}
            tables = [
                r[0]
                for r in conn.execute(
                    "SELECT name FROM sqlite_master WHERE type = 'table'"
                    " AND name NOT LIKE 'sqlite_%' ORDER BY name"
                )
            ]
            ddl = dict(
                conn.execute(
                    "SELECT name, sql FROM sqlite_master WHERE type = 'table'"
                    " AND name NOT LIKE 'sqlite_%'"
                )
            )
            for t in tables:
                out[t] = {
                    "checks": checks(ddl[t] or ""),
                    # (name, type, notnull, default, pk) -- no column ids, so a
                    # reordering that keeps the shape is still a difference.
                    "columns": [tuple(r)[1:] for r in conn.execute(f"PRAGMA table_info({t})")],
                    "foreign_keys": sorted(
                        tuple(r)[2:] for r in conn.execute(f"PRAGMA foreign_key_list({t})")
                    ),
                    "indexes": sorted(
                        (r[1], r[2], r[4])  # name, unique, partial
                        for r in conn.execute(f"PRAGMA index_list({t})")
                        if not r[1].startswith("sqlite_")
                    ),
                }
            return out

        self.assertEqual(shape(fresh), shape(migrated))
        fresh.close()

    def test_future_schema_refused(self):
        conn = self.fresh()
        store.set_setting(conn, "schema_version", "99")
        with self.assertRaises(RuntimeError):
            store.init(conn)


class TestTasks(StoreCase):
    def test_doing_sets_started_at_once(self):
        conn = self.fresh()
        sid = store.commit_draft(conn, "s", None, "manual", ["a"])
        (task,) = store.get_tasks(conn, sid)
        store.resolve_task(conn, task["id"], "doing")
        first = store.get_tasks(conn, sid)[0]["started_at"]
        self.assertIsNotNone(first)
        store.resolve_task(conn, task["id"], "done")
        store.resolve_task(conn, task["id"], "doing")  # dragged back
        self.assertEqual(store.get_tasks(conn, sid)[0]["started_at"], first)
        self.assertIsNone(store.get_tasks(conn, sid)[0]["resolved_at"])

    def test_planned_clears_resolved_at(self):
        conn = self.fresh()
        sid = store.commit_draft(conn, "s", None, "manual", ["a"])
        (task,) = store.get_tasks(conn, sid)
        store.resolve_task(conn, task["id"], "done")
        store.resolve_task(conn, task["id"], "planned")
        row = store.get_tasks(conn, sid)[0]
        self.assertEqual(row["status"], "planned")
        self.assertIsNone(row["resolved_at"])


class TestCloseAndCarry(StoreCase):
    def test_close_is_idempotent(self):
        conn = self.fresh()
        sid = store.commit_draft(conn, "s", 60, "manual", ["a"])
        self.assertTrue(store.close_session(conn, sid))
        ended = store.get_session(conn, sid)["ended_at"]
        self.assertFalse(store.close_session(conn, sid))  # second close: no-op
        self.assertEqual(store.get_session(conn, sid)["ended_at"], ended)

    def test_carry_copies_unfinished_as_planned(self):
        conn = self.fresh()
        sid = store.commit_draft(conn, "s", None, "manual", ["a", "b", "c"])
        a, b, c = store.get_tasks(conn, sid)
        store.resolve_task(conn, a["id"], "done")
        store.resolve_task(conn, b["id"], "doing")
        store.close_session(conn, sid)
        self.assertEqual(store.carry_unfinished(conn, sid), 2)  # b and c
        backlog = store.get_backlog(conn)
        self.assertEqual([t["title"] for t in backlog], ["b", "c"])
        for t in backlog:
            self.assertEqual(t["status"], "planned")  # 'doing' reset
            self.assertIsNone(t["started_at"])
            self.assertIsNone(t["resolved_at"])
        # session rows untouched — immutable history
        self.assertEqual(
            [t["status"] for t in store.get_tasks(conn, sid)],
            ["done", "doing", "planned"],
        )

    def test_double_carry_is_ignored(self):
        conn = self.fresh()
        sid = store.commit_draft(conn, "s", None, "manual", ["a"])
        store.close_session(conn, sid)
        self.assertEqual(store.carry_unfinished(conn, sid), 1)
        self.assertEqual(store.carry_unfinished(conn, sid), 0)  # index blocks it
        self.assertEqual(len(store.get_backlog(conn)), 1)

    def test_carry_count_follows_chain(self):
        conn = self.fresh()
        s1 = store.commit_draft(conn, "one", None, "manual", ["stubborn"])
        store.close_session(conn, s1)
        store.carry_unfinished(conn, s1)
        (b1,) = store.get_backlog(conn)
        self.assertEqual(store.carry_count(conn, b1["id"]), 1)
        s2 = store.commit_draft(conn, "two", None, "manual", [], backlog_ids=[b1["id"]])
        store.close_session(conn, s2)
        store.carry_unfinished(conn, s2)
        (b2,) = store.get_backlog(conn)
        self.assertEqual(store.carry_count(conn, b2["id"]), 2)


class TestBacklogPull(StoreCase):
    def test_pull_appends_positions(self):
        conn = self.fresh()
        s1 = store.commit_draft(conn, "one", None, "manual", ["left behind"])
        store.close_session(conn, s1)
        store.carry_unfinished(conn, s1)
        (b,) = store.get_backlog(conn)
        s2 = store.commit_draft(conn, "two", None, "manual", ["fresh"], backlog_ids=[b["id"]])
        tasks = store.get_tasks(conn, s2)
        self.assertEqual([t["title"] for t in tasks], ["fresh", "left behind"])
        self.assertEqual([t["position"] for t in tasks], [1, 2])
        self.assertEqual(store.get_backlog(conn), [])

    def test_pull_refuses_session_rows(self):
        conn = self.fresh()
        s1 = store.commit_draft(conn, "one", None, "manual", ["mine"])
        s2 = store.commit_draft(conn, "two", None, "manual", [])
        (t,) = store.get_tasks(conn, s1)
        store.pull_from_backlog(conn, s2, [t["id"]])  # not a backlog row
        self.assertEqual(store.get_tasks(conn, s1)[0]["session_id"], s1)
        self.assertEqual(store.get_tasks(conn, s2), [])


class TestHeartbeat(StoreCase):
    def test_heartbeat_only_while_open(self):
        conn = self.fresh()
        sid = store.commit_draft(conn, "s", None, "manual", ["a"])
        self.assertTrue(store.heartbeat(conn, sid))
        self.assertIsNotNone(store.get_session(conn, sid)["last_heartbeat"])
        store.close_session(conn, sid)
        self.assertFalse(store.heartbeat(conn, sid))  # can't resurrect

    def test_recovery_uses_last_heartbeat(self):
        conn = self.fresh()
        sid = store.commit_draft(conn, "s", None, "manual", ["a"])
        store.heartbeat(conn, sid)
        beat = store.get_session(conn, sid)["last_heartbeat"]
        store.mark_recovered(conn, sid)
        session = store.get_session(conn, sid)
        self.assertEqual(session["ended_at"], beat)
        self.assertEqual(session["close_reason"], "recovered")


class TestCheckpointDue(StoreCase):
    def test_timed_session_gets_a_checkpoint(self):
        conn = self.fresh()
        sid = store.commit_draft(conn, "ship it", 45, "manual", ["a"])
        session = store.get_session(conn, sid)
        self.assertEqual(
            session["checkpoint_due_at"],
            store.checkpoint_due(session["started_at"], 45),
        )
        elapsed = datetime.fromisoformat(session["checkpoint_due_at"]) - datetime.fromisoformat(
            session["started_at"]
        )
        self.assertEqual(elapsed, timedelta(minutes=45))

    def test_open_ended_session_gets_none(self):
        conn = self.fresh()
        sid = store.commit_draft(conn, "potter about", None, "manual", ["a"])
        self.assertIsNone(store.get_session(conn, sid)["checkpoint_due_at"])

    def test_checkpoint_due_keeps_the_store_timestamp_format(self):
        # The desktop app compares these as strings against db::now(); an
        # offset-less or space-separated timestamp would silently never fire.
        self.assertEqual(
            store.checkpoint_due("2026-08-29T09:00:00+00:00", 90),
            "2026-08-29T10:30:00+00:00",
        )


class TestAnalysisAndSettings(StoreCase):
    def test_analysis_roundtrip(self):
        conn = self.fresh()
        sid = store.commit_draft(conn, "s", None, "manual", ["a"])
        aid = store.add_analysis(
            conn, sid, "2026-08-27T10:00:00+00:00", "2026-08-27T11:00:00+00:00",
            "Mostly on track", 80, "You spent the hour in the editor.", "{}",
        )
        (row,) = store.get_analyses(conn, sid)
        self.assertEqual(row["id"], aid)
        self.assertIsNone(row["seen_at"])
        store.mark_analysis_seen(conn, aid)
        self.assertIsNotNone(store.get_analyses(conn, sid)[0]["seen_at"])

    def test_settings_roundtrip(self):
        conn = self.fresh()
        self.assertEqual(store.get_setting(conn, "analysis_mean_minutes", "60"), "60")
        store.set_setting(conn, "analysis_mean_minutes", "45")
        store.set_setting(conn, "analysis_mean_minutes", "30")  # upsert
        self.assertEqual(store.get_setting(conn, "analysis_mean_minutes"), "30")


if __name__ == "__main__":
    unittest.main()
