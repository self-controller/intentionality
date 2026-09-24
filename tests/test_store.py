"""Data-layer tests: migration, carry, close idempotency, backlog pulls.

The UI is allowed to be bare-bones; this file is why the data underneath
isn't. Run with:  python3 -m unittest discover tests
"""

import json
import re
import sqlite3
import tempfile
import unittest
from datetime import datetime, timedelta
from pathlib import Path

from gate import config, store
from gate.ui import Details

DB_RS = Path(__file__).parent.parent / "app/src-tauri/src/db.rs"

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


# The v4 schema exactly as shipped, for the v4 -> v5 migration fixture.
V4_SCHEMA = """
PRAGMA journal_mode = WAL;
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
INSERT INTO meta (key, value) VALUES ('schema_version', '4');
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
                 CHECK (source IN ('gate', 'mid-session', 'meeting')),
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
);
CREATE TABLE meeting_segment (
    id         INTEGER PRIMARY KEY,
    meeting_id INTEGER NOT NULL REFERENCES meeting(id) ON DELETE CASCADE,
    seq        INTEGER NOT NULL,
    started_at TEXT NOT NULL,
    text       TEXT NOT NULL
);
CREATE UNIQUE INDEX meeting_segment_seq ON meeting_segment (meeting_id, seq);
CREATE TABLE meeting_action (
    id         INTEGER PRIMARY KEY,
    meeting_id INTEGER NOT NULL REFERENCES meeting(id) ON DELETE CASCADE,
    position   INTEGER NOT NULL,
    text       TEXT NOT NULL,
    task_id    INTEGER REFERENCES task(id) ON DELETE SET NULL
);
"""

# The v5 schema exactly as shipped, for the v5 -> v6 migration fixture.
V5_SCHEMA = """
PRAGMA journal_mode = WAL;
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
INSERT INTO meta (key, value) VALUES ('schema_version', '5');
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
                 CHECK (source IN ('gate', 'mid-session', 'meeting')),
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
CREATE TABLE meeting (
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
                 CHECK (state IN ('recording', 'cleaning', 'summarizing', 'done', 'failed')),
    error        TEXT
);
CREATE TABLE meeting_segment (
    id         INTEGER PRIMARY KEY,
    meeting_id INTEGER NOT NULL REFERENCES meeting(id) ON DELETE CASCADE,
    seq        INTEGER NOT NULL,
    started_at TEXT NOT NULL,
    text       TEXT NOT NULL
);
CREATE UNIQUE INDEX meeting_segment_seq ON meeting_segment (meeting_id, seq);
CREATE TABLE meeting_action (
    id         INTEGER PRIMARY KEY,
    meeting_id INTEGER NOT NULL REFERENCES meeting(id) ON DELETE CASCADE,
    position   INTEGER NOT NULL,
    text       TEXT NOT NULL,
    task_id    INTEGER REFERENCES task(id) ON DELETE SET NULL
);
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
);
CREATE INDEX meeting_file_meeting ON meeting_file (meeting_id, position);
"""

V6_SCHEMA = """
PRAGMA journal_mode = WAL;
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
INSERT INTO meta (key, value) VALUES ('schema_version', '6');
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
                 CHECK (source IN ('gate', 'mid-session', 'meeting')),
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
CREATE TABLE meeting (
    id           INTEGER PRIMARY KEY,
    session_id   INTEGER REFERENCES session(id),
    started_at   TEXT NOT NULL,
    ended_at     TEXT,
    title        TEXT NOT NULL DEFAULT '',
    notes        TEXT NOT NULL DEFAULT '',
    clean_transcript TEXT,
    summary      TEXT,
    state        TEXT NOT NULL DEFAULT 'recording'
                 CHECK (state IN ('recording', 'cleaning', 'summarizing', 'done', 'failed')),
    error        TEXT,
    summary_edited_at TEXT
);
CREATE TABLE meeting_segment (
    id         INTEGER PRIMARY KEY,
    meeting_id INTEGER NOT NULL REFERENCES meeting(id) ON DELETE CASCADE,
    seq        INTEGER NOT NULL,
    started_at TEXT NOT NULL,
    text       TEXT NOT NULL
);
CREATE UNIQUE INDEX meeting_segment_seq ON meeting_segment (meeting_id, seq);
CREATE TABLE meeting_action (
    id         INTEGER PRIMARY KEY,
    meeting_id INTEGER NOT NULL REFERENCES meeting(id) ON DELETE CASCADE,
    position   INTEGER NOT NULL,
    text       TEXT NOT NULL,
    task_id    INTEGER REFERENCES task(id) ON DELETE SET NULL
);
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
);
CREATE INDEX meeting_file_meeting ON meeting_file (meeting_id, position);
"""

# v7 = v6 + task.notes + the two label tables. Derived rather than written
# out: V6_SCHEMA is a frozen snapshot too, and the delta is the part worth
# reading. A replace that missed its target would show up in the parity test.
V7_SCHEMA = V6_SCHEMA.replace(
    "('schema_version', '6')", "('schema_version', '7')"
).replace(
    "    resolved_at  TEXT\n);",
    "    resolved_at  TEXT,\n    notes        TEXT NOT NULL DEFAULT ''\n);",
) + """
CREATE TABLE label (
    id    INTEGER PRIMARY KEY,
    name  TEXT NOT NULL COLLATE NOCASE UNIQUE,
    color TEXT NOT NULL
);
CREATE TABLE task_label (
    task_id  INTEGER NOT NULL REFERENCES task(id) ON DELETE CASCADE,
    label_id INTEGER NOT NULL REFERENCES label(id) ON DELETE CASCADE,
    PRIMARY KEY (task_id, label_id)
);
"""

# v8 = v7 + task.due_date, appended by ALTER TABLE. Same derivation rule as
# V7_SCHEMA above: the delta is the readable part, and a replace that missed
# its target shows up in the parity test.
V8_SCHEMA = V7_SCHEMA.replace(
    "('schema_version', '7')", "('schema_version', '8')"
).replace(
    "    notes        TEXT NOT NULL DEFAULT ''\n);",
    "    notes        TEXT NOT NULL DEFAULT '',\n    due_date     TEXT\n);",
)

# v9 = v8 + meeting.transcript_edited_at, appended by ALTER TABLE.
V9_SCHEMA = V8_SCHEMA.replace(
    "('schema_version', '8')", "('schema_version', '9')"
).replace(
    "    summary_edited_at TEXT\n",
    "    summary_edited_at TEXT,\n    transcript_edited_at TEXT\n",
)


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

    def v4_fixture(self) -> None:
        """Three meetings covering every key_points shape the rebuild must
        survive, plus one still stranded in 'recording' -- which is exactly
        what the live store holds, and what a rebuild is most likely to trip
        over if it assumes every row reached a terminal state."""
        conn = sqlite3.connect(config.STORE_PATH)
        conn.executescript(V4_SCHEMA)
        with conn:
            conn.execute(
                "INSERT INTO session (id, started_at, statement, intended_minutes,"
                " mode) VALUES (1, '2026-09-01T09:00:00+00:00', 'a session', 60,"
                " 'manual')"
            )
            conn.execute(
                "INSERT INTO task (id, session_id, title, position, source,"
                " created_at) VALUES (1, NULL, 'from a meeting', 1, 'meeting',"
                " '2026-09-01T11:00:00+00:00')"
            )
            # Summarized: a flat key_points array, the shape v5 converts.
            conn.execute(
                "INSERT INTO meeting (id, session_id, started_at, ended_at, title,"
                " summary, key_points, state) VALUES (1, 1,"
                " '2026-09-01T10:00:00+00:00', '2026-09-01T11:00:00+00:00',"
                " 'Roadmap', 'We picked Q4.', '[\"Ship in Q4\", \"Drop the widget\"]',"
                " 'done')"
            )
            conn.execute(
                "INSERT INTO meeting_segment (id, meeting_id, seq, started_at, text)"
                " VALUES (1, 1, 0, '2026-09-01T10:00:00+00:00', 'we should ship')"
            )
            conn.execute(
                "INSERT INTO meeting_action (id, meeting_id, position, text, task_id)"
                " VALUES (1, 1, 0, 'Write the plan', 1)"
            )
            # Never summarized: key_points is NULL.
            conn.execute(
                "INSERT INTO meeting (id, started_at, state, error) VALUES"
                " (2, '2026-09-02T10:00:00+00:00', 'failed', 'the model timed out')"
            )
            # Stranded mid-recording, like meeting 2 in the live store.
            conn.execute(
                "INSERT INTO meeting (id, started_at, state) VALUES"
                " (3, '2026-09-03T10:00:00+00:00', 'recording')"
            )
        conn.close()

    def v5_fixture(self) -> None:
        """Every shape the v6 fold has to handle: a fully written-up meeting
        with a subpoint and details, one with only a paragraph, one that
        failed before any notes (all three NULL), one still stranded in
        'recording', and an attached file -- meeting_file has to come through
        untouched."""
        conn = sqlite3.connect(config.STORE_PATH)
        conn.executescript(V5_SCHEMA)
        with conn:
            conn.execute(
                "INSERT INTO session (id, started_at, statement, intended_minutes,"
                " mode) VALUES (1, '2026-09-01T09:00:00+00:00', 'a session', 60,"
                " 'manual')"
            )
            conn.execute(
                "INSERT INTO meeting (id, session_id, started_at, ended_at, title,"
                " notes, summary, key_points, details, state) VALUES (1, 1,"
                " '2026-09-01T10:00:00+00:00', '2026-09-01T11:00:00+00:00',"
                " 'Roadmap', 'Dana = Dana K.', 'We picked Q4.',"
                " '[{\"text\": \"Ship in Q4\", \"subpoints\": [\"the deal closes then\"]},"
                " {\"text\": \"Drop the widget\", \"subpoints\": []}]',"
                " 'The freeze is the week before.', 'done')"
            )
            conn.execute(
                "INSERT INTO meeting_segment (id, meeting_id, seq, started_at, text)"
                " VALUES (1, 1, 0, '2026-09-01T10:00:00+00:00', 'we should ship')"
            )
            conn.execute(
                "INSERT INTO meeting_action (id, meeting_id, position, text)"
                " VALUES (1, 1, 0, 'Write the plan')"
            )
            conn.execute(
                "INSERT INTO meeting_file (id, meeting_id, position, name, path, kind,"
                " bytes, added_at) VALUES (1, 1, 0, 'deck.pptx', 'ab12cd34', 'office',"
                " 4096, '2026-09-01T10:05:00+00:00')"
            )
            # A paragraph alone: no heading may be invented over it.
            conn.execute(
                "INSERT INTO meeting (id, started_at, ended_at, title, summary,"
                " key_points, state) VALUES (2, '2026-09-02T10:00:00+00:00',"
                " '2026-09-02T10:30:00+00:00', 'Quick sync', 'Nothing decided.',"
                " '[]', 'done')"
            )
            # Failed before any notes: every write-up column NULL.
            conn.execute(
                "INSERT INTO meeting (id, started_at, state, error) VALUES"
                " (3, '2026-09-03T10:00:00+00:00', 'failed', 'the model timed out')"
            )
            # Stranded mid-recording, like the live store has had before.
            conn.execute(
                "INSERT INTO meeting (id, started_at, state) VALUES"
                " (4, '2026-09-04T10:00:00+00:00', 'recording')"
            )
        conn.close()


class TestInitAndMigration(StoreCase):
    def test_fresh_init_is_v10(self):
        conn = self.fresh()
        version = conn.execute(
            "SELECT value FROM meta WHERE key = 'schema_version'"
        ).fetchone()[0]
        self.assertEqual(version, "10")

    def test_init_idempotent(self):
        conn = self.fresh()
        store.init(conn)  # second run must be a no-op, not a re-create
        self.assertEqual(store.get_setting(conn, "schema_version"), "10")

    def test_migrates_v1_preserving_rows(self):
        self.v1_fixture()
        conn = self.fresh()
        # A v1 store walks all the way up, not just one step.
        self.assertEqual(store.get_setting(conn, "schema_version"), "10")
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
        self.assertEqual(store.get_setting(conn, "schema_version"), "10")
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
        self.assertEqual(store.get_setting(conn, "schema_version"), "10")
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

    def v6_fixture(self) -> None:
        """A session with tasks in both places -- one still on the board, one
        already in the backlog -- plus a meeting, because v7 leaves the meeting
        tables alone and that has to be visible in the parity check."""
        conn = sqlite3.connect(config.STORE_PATH)
        conn.executescript(V6_SCHEMA)
        with conn:
            conn.execute(
                "INSERT INTO session (id, started_at, statement, intended_minutes,"
                " mode) VALUES (1, '2026-09-01T09:00:00+00:00', 'a session', 60,"
                " 'manual')"
            )
            conn.execute(
                "INSERT INTO task (id, session_id, title, position, created_at)"
                " VALUES (1, 1, 'on the board', 1, '2026-09-01T09:00:00+00:00')"
            )
            conn.execute(
                "INSERT INTO task (id, session_id, title, position, status,"
                " carried_from, created_at) VALUES (2, NULL, 'carried once', 1,"
                " 'planned', 1, '2026-09-01T11:00:00+00:00')"
            )
            conn.execute(
                "INSERT INTO meeting (id, session_id, started_at, title, summary,"
                " state) VALUES (1, 1, '2026-09-01T10:00:00+00:00', 'Roadmap',"
                " 'We picked Q4.', 'done')"
            )
        conn.close()

    def v7_fixture(self) -> None:
        """The shape before due dates: a card with notes and a label on the
        board, and its carried copy in the backlog wearing the same one."""
        conn = sqlite3.connect(config.STORE_PATH)
        conn.executescript(V7_SCHEMA)
        with conn:
            conn.execute(
                "INSERT INTO session (id, started_at, statement, intended_minutes,"
                " mode) VALUES (1, '2026-09-10T09:00:00+00:00', '', 60, 'manual')"
            )
            conn.execute(
                "INSERT INTO task (id, session_id, title, position, created_at, notes)"
                " VALUES (1, 1, 'on the board', 1, '2026-09-10T09:00:00+00:00',"
                " 'half done')"
            )
            conn.execute(
                "INSERT INTO task (id, session_id, title, position, carried_from,"
                " created_at, notes) VALUES (2, NULL, 'on the board', 1, 1,"
                " '2026-09-10T11:00:00+00:00', 'half done')"
            )
            conn.execute("INSERT INTO label (id, name, color) VALUES (1, 'billing', '#7aa2f7')")
            conn.executemany(
                "INSERT INTO task_label (task_id, label_id) VALUES (?, 1)", [(1,), (2,)]
            )
        conn.close()

    def v8_fixture(self) -> None:
        """The shape before the transcript could be edited: a finished meeting
        with its raw segments, a cleaned transcript and a write-up."""
        conn = sqlite3.connect(config.STORE_PATH)
        conn.executescript(V8_SCHEMA)
        with conn:
            conn.execute(
                "INSERT INTO session (id, started_at, statement, intended_minutes,"
                " mode) VALUES (1, '2026-09-11T09:00:00+00:00', '', 60, 'manual')"
            )
            conn.execute(
                "INSERT INTO task (id, session_id, title, position, created_at,"
                " notes, due_date) VALUES (1, 1, 'ship it', 1,"
                " '2026-09-11T09:00:00+00:00', '', '2026-09-12')"
            )
            conn.execute(
                "INSERT INTO meeting (id, session_id, started_at, ended_at, title,"
                " notes, clean_transcript, summary, state) VALUES"
                " (1, 1, '2026-09-11T10:00:00+00:00', '2026-09-11T10:30:00+00:00',"
                " 'Standup', 'jargon', 'the tidied version', '# Standup', 'done')"
            )
            conn.executemany(
                "INSERT INTO meeting_segment (meeting_id, seq, started_at, text)"
                " VALUES (1, ?, ?, ?)",
                [
                    (0, "2026-09-11T10:00:00+00:00", "first two minutes"),
                    (1, "2026-09-11T10:02:00+00:00", "second two minutes"),
                ],
            )
            conn.execute(
                "INSERT INTO meeting_action (meeting_id, position, text)"
                " VALUES (1, 0, 'follow up')"
            )
        conn.close()

    def v9_fixture(self) -> None:
        """The shape before the session indexes: one session, two tasks, one
        analysis."""
        conn = sqlite3.connect(config.STORE_PATH)
        conn.executescript(V9_SCHEMA)
        with conn:
            conn.execute(
                "INSERT INTO session (id, started_at, statement, intended_minutes,"
                " mode) VALUES (1, '2026-09-23T09:00:00+00:00', '', 60, 'manual')"
            )
            conn.executemany(
                "INSERT INTO task (id, session_id, title, position, created_at)"
                " VALUES (?, ?, ?, ?, '2026-09-23T09:00:00+00:00')",
                [(1, 1, "ship it", 1), (2, None, "later", 1)],
            )
            conn.execute(
                "INSERT INTO analysis (session_id, created_at, window_start,"
                " window_end, headline, alignment, body, observed_json) VALUES"
                " (1, '2026-09-23T10:00:00+00:00', '2026-09-23T09:00:00+00:00',"
                " '2026-09-23T10:00:00+00:00', 'On track', 80, 'Fine.', '{}')"
            )
        conn.close()

    def test_migration_end_state_matches_schema_sql(self):
        """schema.sql promises to describe what the migrations produce.

        Compared through PRAGMA rather than the stored DDL text: a migration
        writes its own formatting and drops the comments, so only columns,
        keys and indexes are the actual contract.

        Run from every version that has a fixture, not just the newest: a
        store that walks v3 -> v4 -> ... -> v9 has to land in the same place
        as one that only takes the last step.
        """
        for name in (
            "v3_fixture",
            "v4_fixture",
            "v5_fixture",
            "v6_fixture",
            "v7_fixture",
            "v8_fixture",
            "v9_fixture",
        ):
            with self.subTest(fixture=name):
                self._assert_end_state_matches(getattr(self, name))

    def _assert_end_state_matches(self, fixture):
        for leftover in Path(self._tmp.name).glob("store.db*"):
            leftover.unlink()
        fixture()
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
        migrated.close()
        (Path(self._tmp.name) / "fresh.db").unlink()

    def test_v5_migration_makes_backup(self):
        self.v4_fixture()
        self.fresh()
        backup = Path(str(config.STORE_PATH) + ".pre-v5.bak")
        self.assertTrue(backup.exists())
        bconn = sqlite3.connect(backup)
        self.assertEqual(  # the backup is still v4
            bconn.execute("SELECT value FROM meta WHERE key='schema_version'").fetchone()[0],
            "4",
        )
        bconn.close()

    def test_migrates_v4_preserving_rows(self):
        self.v4_fixture()
        conn = self.fresh()
        self.assertEqual(store.get_setting(conn, "schema_version"), "10")
        # The v5 meeting rebuild is the risky half: every row was copied out
        # of the old table and back into a new one with two more columns.
        meeting = conn.execute("SELECT * FROM meeting WHERE id = 1").fetchone()
        self.assertEqual(meeting["title"], "Roadmap")
        # The paragraph, then the v4 key points folded in on the way through v6.
        self.assertEqual(
            meeting["summary"],
            "We picked Q4.\n\n## Key points\n- Ship in Q4\n- Drop the widget",
        )
        self.assertEqual(meeting["session_id"], 1)
        self.assertEqual(meeting["ended_at"], "2026-09-01T11:00:00+00:00")
        self.assertEqual(meeting["state"], "done")
        # The new columns take their defaults, not NULL for notes.
        self.assertEqual(meeting["notes"], "")
        self.assertIsNone(meeting["clean_transcript"])
        self.assertIsNone(meeting["summary_edited_at"])
        # Segments and actions re-bound to the rebuilt table by name.
        self.assertEqual(
            conn.execute("SELECT text FROM meeting_segment WHERE meeting_id = 1").fetchone()[0],
            "we should ship",
        )
        self.assertEqual(
            conn.execute("SELECT task_id FROM meeting_action WHERE id = 1").fetchone()[0], 1
        )
        self.assertEqual(conn.execute("SELECT COUNT(*) FROM meeting").fetchone()[0], 3)
        self.assertEqual(conn.execute("PRAGMA integrity_check").fetchone()[0], "ok")
        self.assertEqual(conn.execute("PRAGMA foreign_key_check").fetchall(), [])

    def test_v5_keeps_a_meeting_stranded_mid_recording(self):
        """The live store has one. A rebuild that assumed every row had
        reached a terminal state would drop it, and with it the transcript of
        whatever the app was killed in the middle of."""
        self.v4_fixture()
        conn = self.fresh()
        self.assertEqual(
            conn.execute("SELECT state FROM meeting WHERE id = 3").fetchone()[0], "recording"
        )

    def test_v5_state_accepts_cleaning_and_still_rejects_junk(self):
        self.v4_fixture()
        conn = self.fresh()
        with conn:
            conn.execute("UPDATE meeting SET state = 'cleaning' WHERE id = 1")
        self.assertEqual(
            conn.execute("SELECT state FROM meeting WHERE id = 1").fetchone()[0], "cleaning"
        )
        with self.assertRaises(sqlite3.IntegrityError):
            with conn:
                conn.execute("UPDATE meeting SET state = 'thinking' WHERE id = 1")

    def test_v5_key_points_reach_v6_as_bullets(self):
        """v5 turned v4's flat strings into objects; v6 then folded those into
        the document. A v4 store walking both steps has to land on the same
        bullets a v5 store does."""
        self.v4_fixture()
        conn = self.fresh()
        self.assertEqual(
            conn.execute("SELECT summary FROM meeting WHERE id = 1").fetchone()[0],
            "We picked Q4.\n\n## Key points\n- Ship in Q4\n- Drop the widget",
        )
        # Never summarized: NULL key_points fold to nothing, and summary stays
        # NULL rather than becoming an empty document.
        self.assertIsNone(
            conn.execute("SELECT summary FROM meeting WHERE id = 2").fetchone()[0]
        )

    def test_v5_key_points_conversion_survives_junk(self):
        """Unattended, one-shot, over rows an older binary wrote. Refusing to
        migrate is worse than losing a malformed key point."""
        self.assertEqual(store._v5_key_points(None), "[]")
        self.assertEqual(store._v5_key_points("not json at all"), "[]")
        self.assertEqual(store._v5_key_points('{"text": "an object"}'), "[]")
        self.assertEqual(store._v5_key_points('["", "  "]'), "[]")
        self.assertEqual(
            json.loads(store._v5_key_points('["keep", 7, null, "me"]')),
            [{"text": "keep", "subpoints": []}, {"text": "me", "subpoints": []}],
        )
        # Idempotent: a v5-shaped array is not double-wrapped.
        self.assertEqual(
            json.loads(store._v5_key_points('[{"text": "a", "subpoints": ["b"]}]')),
            [{"text": "a", "subpoints": ["b"]}],
        )

    def test_v5_meeting_file_cascades_with_its_meeting(self):
        self.v4_fixture()
        conn = self.fresh()
        with conn:
            conn.execute(
                "INSERT INTO meeting_file (meeting_id, position, name, path, kind,"
                " bytes, added_at) VALUES (1, 0, 'deck.pptx', 'ab12cd34', 'office',"
                " 4096, '2026-09-01T10:05:00+00:00')"
            )
        self.assertEqual(conn.execute("SELECT COUNT(*) FROM meeting_file").fetchone()[0], 1)
        with conn:
            conn.execute("DELETE FROM meeting WHERE id = 1")
        self.assertEqual(conn.execute("SELECT COUNT(*) FROM meeting_file").fetchone()[0], 0)

    def test_v5_meeting_file_kind_is_constrained(self):
        self.v4_fixture()
        conn = self.fresh()
        with self.assertRaises(sqlite3.IntegrityError):
            with conn:
                conn.execute(
                    "INSERT INTO meeting_file (meeting_id, position, name, path,"
                    " kind, bytes, added_at) VALUES (1, 0, 'x', 'y', 'video', 1,"
                    " '2026-09-01T10:05:00+00:00')"
                )

    def test_v6_migration_makes_backup(self):
        self.v5_fixture()
        self.fresh()
        backup = Path(str(config.STORE_PATH) + ".pre-v6.bak")
        self.assertTrue(backup.exists())
        bconn = sqlite3.connect(backup)
        self.assertEqual(  # the backup is still v5
            bconn.execute("SELECT value FROM meta WHERE key='schema_version'").fetchone()[0],
            "5",
        )
        bconn.close()

    def test_migrates_v5_preserving_rows(self):
        self.v5_fixture()
        conn = self.fresh()
        self.assertEqual(store.get_setting(conn, "schema_version"), "10")
        meeting = conn.execute("SELECT * FROM meeting WHERE id = 1").fetchone()
        self.assertEqual(meeting["title"], "Roadmap")
        self.assertEqual(meeting["notes"], "Dana = Dana K.")
        self.assertEqual(meeting["session_id"], 1)
        self.assertEqual(meeting["state"], "done")
        # Nobody has edited anything yet.
        self.assertIsNone(meeting["summary_edited_at"])
        # The two folded columns are gone, not merely emptied.
        columns = {r[1] for r in conn.execute("PRAGMA table_info(meeting)")}
        self.assertNotIn("key_points", columns)
        self.assertNotIn("details", columns)
        self.assertEqual(
            conn.execute("SELECT text FROM meeting_segment WHERE meeting_id = 1").fetchone()[0],
            "we should ship",
        )
        self.assertEqual(
            conn.execute("SELECT name FROM meeting_file WHERE meeting_id = 1").fetchone()[0],
            "deck.pptx",
        )
        self.assertEqual(conn.execute("SELECT COUNT(*) FROM meeting").fetchone()[0], 4)
        self.assertEqual(
            conn.execute("SELECT state FROM meeting WHERE id = 4").fetchone()[0], "recording"
        )
        self.assertEqual(conn.execute("PRAGMA integrity_check").fetchone()[0], "ok")
        self.assertEqual(conn.execute("PRAGMA foreign_key_check").fetchall(), [])

    def test_v7_migration_makes_backup(self):
        self.v6_fixture()
        self.fresh()
        backup = Path(str(config.STORE_PATH) + ".pre-v7.bak")
        self.assertTrue(backup.exists())
        bconn = sqlite3.connect(backup)
        self.assertEqual(  # the backup is still v6
            bconn.execute("SELECT value FROM meta WHERE key='schema_version'").fetchone()[0],
            "6",
        )
        bconn.close()

    def test_migrates_v6_preserving_rows(self):
        self.v6_fixture()
        conn = self.fresh()
        self.assertEqual(store.get_setting(conn, "schema_version"), "10")
        rows = conn.execute("SELECT * FROM task ORDER BY id").fetchall()
        self.assertEqual([r["title"] for r in rows], ["on the board", "carried once"])
        # Every existing task starts with empty notes, never NULL.
        self.assertEqual([r["notes"] for r in rows], ["", ""])
        self.assertEqual(rows[1]["carried_from"], 1)
        self.assertEqual(
            conn.execute("SELECT title FROM meeting WHERE id = 1").fetchone()[0], "Roadmap"
        )
        self.assertEqual(conn.execute("SELECT COUNT(*) FROM label").fetchone()[0], 0)
        self.assertEqual(conn.execute("PRAGMA integrity_check").fetchone()[0], "ok")
        self.assertEqual(conn.execute("PRAGMA foreign_key_check").fetchall(), [])
        # The carry guarantee has to survive every migration that touches task.
        with conn:
            conn.execute(
                "INSERT OR IGNORE INTO task (session_id, title, position,"
                " carried_from, created_at) VALUES (NULL, 'twice?', 9, 1,"
                " '2026-09-02T09:00:00+00:00')"
            )
        self.assertEqual(conn.execute("SELECT COUNT(*) FROM task").fetchone()[0], 2)

    def test_v7_label_name_is_unique_case_insensitively(self):
        conn = self.fresh()
        with conn:
            conn.execute("INSERT INTO label (name, color) VALUES ('billing', '#7aa2f7')")
        with self.assertRaises(sqlite3.IntegrityError):
            with conn:
                conn.execute("INSERT INTO label (name, color) VALUES ('Billing', '#9ece6a')")

    def test_deleting_a_task_takes_its_label_links(self):
        """The join rows go with the task, so a deleted backlog item can never
        leave a label looking as though something still wears it."""
        conn = self.fresh()
        with conn:
            conn.execute(
                "INSERT INTO task (id, session_id, title, position, created_at)"
                " VALUES (1, NULL, 'a backlog item', 1, '2026-09-01T09:00:00+00:00')"
            )
            conn.execute("INSERT INTO label (id, name, color) VALUES (1, 'billing', '#7aa2f7')")
            conn.execute("INSERT INTO task_label (task_id, label_id) VALUES (1, 1)")
        with conn:
            conn.execute("DELETE FROM task WHERE id = 1")
        self.assertEqual(conn.execute("SELECT COUNT(*) FROM task_label").fetchone()[0], 0)

    def test_v6_folds_key_points_and_details_into_the_document(self):
        self.v5_fixture()
        conn = self.fresh()

        def doc(meeting_id):
            return conn.execute(
                "SELECT summary FROM meeting WHERE id = ?", (meeting_id,)
            ).fetchone()[0]

        self.assertEqual(
            doc(1),
            "We picked Q4.\n\n"
            "## Key points\n- Ship in Q4\n  - the deal closes then\n- Drop the widget\n\n"
            "## Additional information\nThe freeze is the week before.",
        )
        # A paragraph alone gets no heading invented over it.
        self.assertEqual(doc(2), "Nothing decided.")
        # Never summarized stays NULL: that is what the app reads as "no notes yet".
        self.assertIsNone(doc(3))
        self.assertIsNone(doc(4))

    def test_v6_document_survives_junk(self):
        """Same contract as _v5_key_points: unattended, one-shot, over rows an
        older binary wrote. Junk drops out; the migration never refuses."""
        self.assertIsNone(store._v6_document(None, None, None))
        self.assertIsNone(store._v6_document("  ", "not json", "  "))
        self.assertEqual(store._v6_document("A.", "not json", None), "A.")
        # v4's flat shape, in case a row somehow skipped v5's conversion.
        self.assertEqual(
            store._v6_document(None, '["flat v4 shape", 7, null]', None),
            "## Key points\n- flat v4 shape",
        )
        self.assertEqual(
            store._v6_document(None, '[{"text": " a ", "subpoints": [" b ", "", 3]}]', None),
            "## Key points\n- a\n  - b",
        )
        self.assertEqual(
            store._v6_document(None, None, "Only context."),
            "## Additional information\nOnly context.",
        )

    def test_v8_migration_makes_backup(self):
        self.v7_fixture()
        self.fresh()
        backup = Path(str(config.STORE_PATH) + ".pre-v8.bak")
        self.assertTrue(backup.exists())
        bconn = sqlite3.connect(backup)
        self.assertEqual(  # the backup is still v7
            bconn.execute("SELECT value FROM meta WHERE key='schema_version'").fetchone()[0],
            "7",
        )
        bconn.close()

    def test_migrates_v7_preserving_rows(self):
        self.v7_fixture()
        conn = self.fresh()
        self.assertEqual(store.get_setting(conn, "schema_version"), "10")
        rows = conn.execute("SELECT * FROM task ORDER BY id").fetchall()
        self.assertEqual([r["notes"] for r in rows], ["half done", "half done"])
        # Every existing task starts with no due date.
        self.assertEqual([r["due_date"] for r in rows], [None, None])
        self.assertEqual(conn.execute("SELECT COUNT(*) FROM task_label").fetchone()[0], 2)
        self.assertEqual(conn.execute("PRAGMA integrity_check").fetchone()[0], "ok")
        self.assertEqual(conn.execute("PRAGMA foreign_key_check").fetchall(), [])
        # The carry guarantee has to survive every migration that touches task.
        with conn:
            conn.execute(
                "INSERT OR IGNORE INTO task (session_id, title, position,"
                " carried_from, created_at) VALUES (NULL, 'twice?', 9, 1,"
                " '2026-09-11T09:00:00+00:00')"
            )
        self.assertEqual(conn.execute("SELECT COUNT(*) FROM task").fetchone()[0], 2)

    def test_v9_migration_makes_backup(self):
        self.v8_fixture()
        self.fresh()
        backup = Path(str(config.STORE_PATH) + ".pre-v9.bak")
        self.assertTrue(backup.exists())
        bconn = sqlite3.connect(backup)
        self.assertEqual(  # the backup is still v8
            bconn.execute("SELECT value FROM meta WHERE key='schema_version'").fetchone()[0],
            "8",
        )
        bconn.close()

    def test_migrates_v8_preserving_rows(self):
        self.v8_fixture()
        conn = self.fresh()
        self.assertEqual(store.get_setting(conn, "schema_version"), "10")
        meeting = conn.execute("SELECT * FROM meeting WHERE id = 1").fetchone()
        # Nothing the meeting already held may be disturbed by the new column.
        self.assertEqual(meeting["title"], "Standup")
        self.assertEqual(meeting["notes"], "jargon")
        self.assertEqual(meeting["clean_transcript"], "the tidied version")
        self.assertEqual(meeting["summary"], "# Standup")
        self.assertEqual(meeting["state"], "done")
        # Nothing was hand-edited before this version existed, so the mark
        # starts clear -- a backfilled stamp would claim every old write-up
        # was out of date with its transcript.
        self.assertIsNone(meeting["transcript_edited_at"])
        self.assertEqual(
            [r["text"] for r in conn.execute(
                "SELECT text FROM meeting_segment WHERE meeting_id = 1 ORDER BY seq"
            )],
            ["first two minutes", "second two minutes"],
        )
        self.assertEqual(conn.execute("SELECT COUNT(*) FROM meeting_action").fetchone()[0], 1)
        self.assertEqual(conn.execute("SELECT COUNT(*) FROM task").fetchone()[0], 1)
        self.assertEqual(conn.execute("PRAGMA integrity_check").fetchone()[0], "ok")
        self.assertEqual(conn.execute("PRAGMA foreign_key_check").fetchall(), [])

    def test_v10_migration_makes_backup(self):
        self.v9_fixture()
        self.fresh()
        backup = Path(str(config.STORE_PATH) + ".pre-v10.bak")
        bconn = sqlite3.connect(backup)
        self.assertEqual(  # the backup is still v9
            bconn.execute("SELECT value FROM meta WHERE key='schema_version'").fetchone()[0],
            "9",
        )
        bconn.close()

    def test_migrates_v9_preserving_rows_and_using_the_index(self):
        self.v9_fixture()
        conn = self.fresh()
        self.assertEqual(store.get_setting(conn, "schema_version"), "10")
        self.assertEqual(conn.execute("SELECT COUNT(*) FROM task").fetchone()[0], 2)
        self.assertEqual(conn.execute("SELECT COUNT(*) FROM analysis").fetchone()[0], 1)
        plan = " ".join(
            r[3] for r in conn.execute(
                "EXPLAIN QUERY PLAN SELECT * FROM task WHERE session_id = 1 ORDER BY position"
            )
        )
        self.assertIn("task_session", plan)
        self.assertEqual(conn.execute("PRAGMA integrity_check").fetchone()[0], "ok")

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

    def test_carry_copies_notes_and_labels(self):
        """What a task collected during the session comes with it. Losing the
        notes on carry would be the quiet kind of loss the 'n means not done'
        bug already cost this store once."""
        conn = self.fresh()
        sid = store.commit_draft(conn, "s", None, "manual", ["a", "b"])
        a, b = store.get_tasks(conn, sid)
        with conn:
            conn.execute("UPDATE task SET notes = ? WHERE id = ?", ("half done", a["id"]))
            conn.execute("INSERT INTO label (id, name, color) VALUES (1, 'billing', '#7aa2f7')")
            conn.execute("INSERT INTO label (id, name, color) VALUES (2, 'urgent', '#f7768e')")
            conn.executemany(
                "INSERT INTO task_label (task_id, label_id) VALUES (?, ?)",
                [(a["id"], 1), (a["id"], 2)],
            )
        store.resolve_task(conn, b["id"], "done")
        store.close_session(conn, sid)
        self.assertEqual(store.carry_unfinished(conn, sid), 1)

        carried = store.get_backlog(conn)[0]
        self.assertEqual(carried["title"], "a")
        self.assertEqual(carried["notes"], "half done")
        self.assertEqual(
            [
                r[0]
                for r in conn.execute(
                    "SELECT l.name FROM task_label tl JOIN label l ON l.id = tl.label_id"
                    " WHERE tl.task_id = ? ORDER BY l.name",
                    (carried["id"],),
                )
            ],
            ["billing", "urgent"],
        )
        # The session row keeps its own copy: it is history, not a move.
        self.assertEqual(
            conn.execute("SELECT COUNT(*) FROM task_label WHERE task_id = ?", (a["id"],))
            .fetchone()[0],
            2,
        )
        # A second carry is still the no-op task_carried_once makes it, and it
        # does not double the label rows either.
        self.assertEqual(store.carry_unfinished(conn, sid), 0)
        self.assertEqual(conn.execute("SELECT COUNT(*) FROM task_label").fetchone()[0], 4)

    def test_carry_copies_due_date(self):
        """A deadline does not move because the session ended."""
        conn = self.fresh()
        sid = store.commit_draft(conn, "s", None, "manual", ["a", "b"])
        a, _ = store.get_tasks(conn, sid)
        with conn:
            conn.execute("UPDATE task SET due_date = '2026-09-18' WHERE id = ?", (a["id"],))
        store.close_session(conn, sid)
        store.carry_unfinished(conn, sid)
        self.assertEqual(
            [(t["title"], t["due_date"]) for t in store.get_backlog(conn)],
            [("a", "2026-09-18"), ("b", None)],
        )

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
        self.assertEqual(store.backlog_carry_counts(conn), {b1["id"]: 1})
        s2 = store.commit_draft(conn, "two", None, "manual", [], backlog_ids=[b1["id"]])
        store.close_session(conn, s2)
        store.carry_unfinished(conn, s2)
        (b2,) = store.get_backlog(conn)
        self.assertEqual(store.backlog_carry_counts(conn), {b2["id"]: 2})


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


class TestCommitPlan(StoreCase):
    """The welcome screen's commit: finish, delete, pull, add, in one go."""

    def carried(self, conn, *titles):
        """A closed session and the backlog copies of its tasks."""
        old = store.commit_draft(conn, "", None, "manual", list(titles))
        store.close_session(conn, old)
        store.carry_unfinished(conn, old)
        return old, [t["id"] for t in store.get_backlog(conn)]

    def test_kept_tasks_first_then_typed(self):
        conn = self.fresh()
        _, (a, b) = self.carried(conn, "a", "b")
        sid = store.commit_plan(conn, 45, [b, a], ["typed"])
        tasks = store.get_tasks(conn, sid)
        self.assertEqual([t["title"] for t in tasks], ["b", "a", "typed"])
        self.assertEqual([t["position"] for t in tasks], [1, 2, 3])
        self.assertEqual(store.get_backlog(conn), [])
        session = store.get_session(conn, sid)
        self.assertEqual((session["statement"], session["mode"]), ("", "manual"))
        self.assertEqual(session["intended_minutes"], 45)
        self.assertEqual(
            session["checkpoint_due_at"], store.checkpoint_due(session["started_at"], 45)
        )

    def test_done_finishes_the_origin_and_drops_the_copy(self):
        conn = self.fresh()
        old, (copy,) = self.carried(conn, "finished last time")
        sid = store.commit_plan(conn, None, [], ["next"], done_ids=[copy])
        (origin,) = store.get_tasks(conn, old)
        self.assertEqual(origin["status"], "done")
        self.assertIsNotNone(origin["resolved_at"])
        # The copy is gone. (Not checked by id: SQLite hands a deleted top
        # rowid straight to the next insert, here the typed task.)
        self.assertEqual(store.get_backlog(conn), [])
        self.assertEqual([t["title"] for t in store.get_tasks(conn, sid)], ["next"])

    def test_done_without_an_origin_writes_nothing(self):
        conn = self.fresh()
        with conn:
            cur = conn.execute(
                "INSERT INTO task (session_id, title, position, created_at)"
                " VALUES (NULL, 'from a meeting', 1, ?)",
                (store.now(),),
            )
        store.commit_plan(conn, None, [], ["next"], done_ids=[cur.lastrowid])
        (row,) = store.get_backlog(conn)
        self.assertEqual((row["title"], row["status"]), ("from a meeting", "planned"))

    def test_delete_is_the_apps_backlog_delete(self):
        conn = self.fresh()
        old, (copy,) = self.carried(conn, "not any more")
        with conn:
            label = conn.execute(
                "INSERT INTO label (name, color) VALUES ('work', '#fff')"
            ).lastrowid
            conn.execute("INSERT INTO task_label VALUES (?, ?)", (copy, label))
            meeting = conn.execute(
                "INSERT INTO meeting (started_at, state) VALUES (?, 'done')", (store.now(),)
            ).lastrowid
            conn.execute(
                "INSERT INTO meeting_action (meeting_id, position, text, task_id)"
                " VALUES (?, 0, 'not any more', ?)",
                (meeting, copy),
            )

        store.commit_plan(conn, None, [], ["next"], delete_ids=[copy])

        self.assertEqual(store.get_backlog(conn), [])
        self.assertEqual(conn.execute("SELECT COUNT(*) FROM task_label").fetchone()[0], 0)
        self.assertIsNone(conn.execute("SELECT task_id FROM meeting_action").fetchone()[0])
        # The session row it was carried from is history, and is untouched.
        self.assertEqual(store.get_tasks(conn, old)[0]["status"], "planned")

    def test_delete_refuses_session_rows(self):
        conn = self.fresh()
        old = store.commit_draft(conn, "", None, "manual", ["history"])
        (row,) = store.get_tasks(conn, old)
        store.commit_plan(conn, None, [], ["next"], delete_ids=[row["id"]])
        self.assertEqual([t["title"] for t in store.get_tasks(conn, old)], ["history"])

    def test_an_empty_session_rolls_everything_back(self):
        conn = self.fresh()
        old, (done, gone, kept) = self.carried(conn, "done", "gone", "kept")
        # The desktop app took the kept task while the gate was up.
        store.pull_from_backlog(conn, old, [kept])
        sessions = conn.execute("SELECT COUNT(*) FROM session").fetchone()[0]

        with self.assertRaises(store.EmptyPlan):
            store.commit_plan(conn, 30, [kept], [], done_ids=[done], delete_ids=[gone])

        self.assertEqual(conn.execute("SELECT COUNT(*) FROM session").fetchone()[0], sessions)
        self.assertEqual([t["id"] for t in store.get_backlog(conn)], [done, gone])
        self.assertEqual(store.get_tasks(conn, old)[0]["status"], "planned")


class TestTaskDetails(StoreCase):
    """_set_details, the gate's twin of the app's db::write_details."""

    def backlog_task(self, conn, title="t"):
        with conn:
            return conn.execute(
                "INSERT INTO task (session_id, title, position, created_at)"
                " VALUES (NULL, ?, 1, ?)",
                (title, store.now()),
            ).lastrowid

    def set(self, conn, task_id, details, session_id=None):
        with conn:
            return store._set_details(conn, task_id, session_id, details)

    def test_palette_matches_the_app(self):
        # Parsed out of the Rust source, so the two lists cannot drift.
        block = re.search(r"const LABEL_COLORS[^=]*=\s*\[(.*?)\];", DB_RS.read_text(), re.S)
        self.assertEqual(tuple(re.findall(r'"(#[0-9a-f]{6})"', block[1])), store.LABEL_COLORS)

    def test_round_trip_and_labels_by_task(self):
        conn = self.fresh()
        a, b = self.backlog_task(conn, "a"), self.backlog_task(conn, "b")
        self.assertTrue(self.set(conn, a, Details("  some notes  ", "2026-10-01", ("zeta", "Alpha"))))
        self.set(conn, b, Details(labels=("alpha", " ", "")))
        row = conn.execute("SELECT notes, due_date FROM task WHERE id = ?", (a,)).fetchone()
        self.assertEqual((row["notes"], row["due_date"]), ("some notes", "2026-10-01"))
        # "alpha" is the label "Alpha" already made, spelling and all.
        self.assertEqual(store.labels_by_task(conn), {a: ["Alpha", "zeta"], b: ["Alpha"]})
        labels = store.list_labels(conn)
        self.assertEqual([r["name"] for r in labels], ["Alpha", "zeta"])
        self.assertEqual([r["color"] for r in labels], list(store.LABEL_COLORS[1::-1]))

    def test_a_label_outlives_its_last_task(self):
        conn = self.fresh()
        a = self.backlog_task(conn)
        self.set(conn, a, Details(labels=("preset",)))
        self.set(conn, a, Details())
        self.assertEqual(store.labels_by_task(conn), {})
        self.assertEqual([r["name"] for r in store.list_labels(conn)], ["preset"])

    def test_guarded_on_where_the_task_is(self):
        conn = self.fresh()
        a = self.backlog_task(conn)
        sid = store.commit_draft(conn, "", None, "manual", ["x"])
        self.assertFalse(self.set(conn, a, Details("n", labels=("l",)), session_id=sid))
        self.assertEqual(conn.execute("SELECT COUNT(*) FROM label").fetchone()[0], 0)

    def test_a_malformed_date_is_refused(self):
        conn = self.fresh()
        a = self.backlog_task(conn)
        for bad in ("2026-9-1", "20261001", "2026-02-30"):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                self.set(conn, a, Details(due_date=bad))
        self.assertIsNone(conn.execute("SELECT due_date FROM task").fetchone()[0])

    def test_commit_plan_writes_details_and_rolls_them_back(self):
        conn = self.fresh()
        old = store.commit_draft(conn, "", None, "manual", ["kept"])
        store.close_session(conn, old)
        store.carry_unfinished(conn, old)
        (kept,) = [t["id"] for t in store.get_backlog(conn)]

        sid = store.commit_plan(
            conn, None, [kept], ["typed", "plain"],
            new_details=[Details("typed notes", labels=("new",))],
            edits={kept: Details(due_date="2026-10-01")},
        )
        kept_row, typed, plain = store.get_tasks(conn, sid)
        self.assertEqual(kept_row["due_date"], "2026-10-01")
        self.assertEqual(typed["notes"], "typed notes")
        self.assertEqual((plain["notes"], plain["due_date"]), ("", None))
        self.assertEqual(store.labels_by_task(conn), {typed["id"]: ["new"]})

        # Nothing survives an EmptyPlan, the labels it would have made included.
        second = store.commit_draft(conn, "", None, "manual", ["again"])
        store.close_session(conn, second)
        store.carry_unfinished(conn, second)
        (copy,) = [t["id"] for t in store.get_backlog(conn)]
        store.pull_from_backlog(conn, second, [copy])
        with self.assertRaises(store.EmptyPlan):
            store.commit_plan(conn, None, [copy], [], edits={copy: Details(labels=("ghost",))})
        self.assertEqual([r["name"] for r in store.list_labels(conn)], ["new"])


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


class TestSettings(StoreCase):
    def test_settings_roundtrip(self):
        conn = self.fresh()
        self.assertEqual(store.get_setting(conn, "analysis_mean_minutes", "60"), "60")
        store.set_setting(conn, "analysis_mean_minutes", "45")
        store.set_setting(conn, "analysis_mean_minutes", "30")  # upsert
        self.assertEqual(store.get_setting(conn, "analysis_mean_minutes"), "30")


if __name__ == "__main__":
    unittest.main()
