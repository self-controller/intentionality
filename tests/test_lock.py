"""One gate on screen at a time: the lock, and where the gates hold it.

The bug this pins: the lid closed on the login gate's welcome screen, waking
fired the resume gate on VT9 as well, and answering both split the tasks over
two sessions. resume.should_gate's side is in test_resume.py.

Run with:  python3 -m unittest discover tests
"""

import contextlib
import io
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from gate import __main__ as gate_main
from gate import config, flow, handoff, lock, store
from gate.ui import GateAborted

REPO = Path(__file__).resolve().parents[1]


class LockCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.addCleanup(setattr, config, "STORE_PATH", config.STORE_PATH)
        config.STORE_PATH = Path(self._tmp.name) / "store.db"


class TestLock(LockCase):
    def test_lives_beside_the_store(self):
        self.assertEqual(lock.path(), Path(self._tmp.name) / "gate.lock")

    def test_nothing_held_before_any_gate_ran(self):
        self.assertFalse(lock.path().exists())
        self.assertFalse(lock.held())

    def test_held_for_the_block_and_no_longer(self):
        with lock.hold() as mine:
            self.assertTrue(mine)
            self.assertTrue(lock.held())
        self.assertFalse(lock.held())

    def test_a_second_gate_runs_anyway_without_it(self):
        ran = False
        with lock.hold():
            with lock.hold() as mine:
                ran = True
                self.assertFalse(mine)
            # The loser closing its own descriptor must not free the winner's.
            self.assertTrue(lock.held())
        self.assertTrue(ran)
        self.assertFalse(lock.held())

    def test_an_unusable_path_never_blocks_a_gate(self):
        config.STORE_PATH = Path(self._tmp.name) / "missing" / "store.db"
        ran = False
        with lock.hold() as mine:
            ran = True
            self.assertFalse(mine)
            self.assertFalse(lock.held())
        self.assertTrue(ran)

    def test_a_killed_holder_leaves_nothing_behind(self):
        # The reason it is an flock and not a row in the store: however the
        # holder dies, the kernel lets go, so the wake gate can't be
        # silenced for good by a crash.
        holder = subprocess.Popen(
            [
                sys.executable, "-c",
                "import sys\n"
                "from gate import lock\n"
                "with lock.hold() as mine:\n"
                "    print(mine, flush=True)\n"
                "    sys.stdin.read()\n",
            ],
            cwd=REPO,
            env={**os.environ, "INTENTIONALITY_STORE": str(config.STORE_PATH)},
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
        )
        try:
            self.assertEqual(holder.stdout.readline().strip(), "True")
            self.assertTrue(lock.held())
        finally:
            holder.kill()
            holder.wait()
            holder.stdin.close()
            holder.stdout.close()
        self.assertFalse(lock.held())


class TestGatesHoldIt(LockCase):
    """Held while the welcome screen is up, and not a moment longer."""

    def setUp(self):
        super().setUp()
        self.conn = store.connect()
        self.addCleanup(self.conn.close)
        store.init(self.conn)
        printed = contextlib.redirect_stdout(io.StringIO())
        printed.__enter__()
        self.addCleanup(printed.__exit__, None, None, None)

        self.seen: dict[str, bool] = {}
        self.addCleanup(setattr, flow, "run", flow.run)
        self.addCleanup(setattr, handoff, "launch_and_wait", handoff.launch_and_wait)
        self.addCleanup(setattr, gate_main, "_close_debrief_carry", gate_main._close_debrief_carry)
        self.addCleanup(setattr, config, "DESKTOP_CMD", config.DESKTOP_CMD)

        def run(conn, notes=()):
            self.seen["welcome"] = lock.held()
            return 1

        def launch(session_id):
            self.seen["desktop"] = lock.held()
            return 0

        flow.run = run
        handoff.launch_and_wait = launch
        gate_main._close_debrief_carry = lambda conn, session_id: None
        config.DESKTOP_CMD = ["fake-desktop"]

    def test_login_gate_lets_go_before_the_desktop(self):
        # The text-gate fallback runs GNOME as its child for hours: a lock
        # still held then would keep every wake gate from firing.
        self.assertEqual(gate_main.gate(self.conn), 0)
        self.assertEqual(self.seen, {"welcome": True, "desktop": False})
        self.assertFalse(lock.held())

    def test_resume_gate_holds_it_too(self):
        self.assertEqual(gate_main.resume_cmd(self.conn), 0)
        self.assertEqual(self.seen, {"welcome": True})
        self.assertFalse(lock.held())

    def test_an_aborted_gate_lets_go(self):
        def abort(conn, notes=()):
            raise GateAborted

        flow.run = abort
        self.assertEqual(gate_main.gate(self.conn), 0)
        self.assertFalse(lock.held())


if __name__ == "__main__":
    unittest.main()
