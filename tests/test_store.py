"""Data-layer tests: migration, carry, close idempotency, backlog pulls.

The UI is allowed to be bare-bones; this file is why the data underneath
isn't. Run with:  python3 -m unittest discover tests
"""

import sqlite3
import tempfile
import unittest
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


class TestInitAndMigration(StoreCase):
    def test_fresh_init_is_v2(self):
        conn = self.fresh()
        version = conn.execute(
            "SELECT value FROM meta WHERE key = 'schema_version'"
        ).fetchone()[0]
        self.assertEqual(version, "2")

    def test_init_idempotent(self):
        conn = self.fresh()
        store.init(conn)  # second run must be a no-op, not a re-create
        self.assertEqual(store.get_setting(conn, "schema_version"), "2")

    def test_migrates_v1_preserving_rows(self):
        self.v1_fixture()
        conn = self.fresh()
        self.assertEqual(store.get_setting(conn, "schema_version"), "2")
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
