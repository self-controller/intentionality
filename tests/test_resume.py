"""Does waking the machine re-open the gate?

The decision only — the gate it then runs is the same one the login path
runs, covered by test_store.py. Run with:  python3 -m unittest discover tests
"""

import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path

from gate import config, resume, store


def ago(minutes: float) -> str:
    ts = datetime.now(timezone.utc) - timedelta(minutes=minutes)
    return ts.isoformat(timespec="seconds")


class ResumeCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self._saved_store_path = config.STORE_PATH
        self._saved_skip_path = config.SKIP_PATH
        config.STORE_PATH = Path(self._tmp.name) / "store.db"
        # Point the escape hatch somewhere the test owns: the real one lives
        # in $HOME and may genuinely exist on the machine running this.
        config.SKIP_PATH = Path(self._tmp.name) / "skip"
        self.conn = store.connect()
        store.init(self.conn)

    def tearDown(self):
        self.conn.close()
        config.STORE_PATH = self._saved_store_path
        config.SKIP_PATH = self._saved_skip_path
        self._tmp.cleanup()

    def open_session(self, last_heartbeat: str | None) -> int:
        session_id = store.commit_draft(
            self.conn, "write it up", 90, "manual", ["a task"]
        )
        if last_heartbeat is not None:
            with self.conn:
                self.conn.execute(
                    "UPDATE session SET last_heartbeat = ? WHERE id = ?",
                    (last_heartbeat, session_id),
                )
        return session_id


class TestAwayMinutes(ResumeCase):
    def test_no_open_session_is_unmeasurable(self):
        self.assertIsNone(resume.away_minutes(self.conn))

    def test_open_session_without_heartbeat_is_unmeasurable(self):
        self.open_session(None)
        self.assertIsNone(resume.away_minutes(self.conn))

    def test_measured_from_the_last_heartbeat(self):
        self.open_session(ago(45))
        self.assertAlmostEqual(resume.away_minutes(self.conn), 45, delta=1)

    def test_a_closed_session_does_not_count(self):
        session_id = self.open_session(ago(600))
        store.close_session(self.conn, session_id)
        self.assertIsNone(resume.away_minutes(self.conn))


class TestShouldGate(ResumeCase):
    def test_fires_when_away_past_the_threshold(self):
        self.open_session(ago(31))
        self.assertTrue(resume.should_gate(self.conn))

    def test_quiet_when_barely_away(self):
        self.open_session(ago(3))
        self.assertFalse(resume.should_gate(self.conn))

    def test_exactly_the_threshold_fires(self):
        self.open_session(ago(resume.DEFAULT_MIN_AWAY_MINUTES + 0.01))
        self.assertTrue(resume.should_gate(self.conn))

    def test_nothing_to_measure_fires(self):
        # No stated intention is the case the gate exists for.
        self.assertTrue(resume.should_gate(self.conn))

    def test_the_setting_overrides_the_default(self):
        self.open_session(ago(5))
        self.assertFalse(resume.should_gate(self.conn))
        store.set_setting(self.conn, resume.SETTING, "1")
        self.assertTrue(resume.should_gate(self.conn))

    def test_a_junk_setting_falls_back_to_the_default(self):
        store.set_setting(self.conn, resume.SETTING, "soon")
        self.assertEqual(
            resume.threshold_minutes(self.conn), resume.DEFAULT_MIN_AWAY_MINUTES
        )

    def test_the_skip_file_wins_over_everything(self):
        self.open_session(ago(600))
        config.SKIP_PATH.touch()
        self.assertFalse(resume.should_gate(self.conn))


if __name__ == "__main__":
    unittest.main()
