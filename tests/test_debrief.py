"""What the debrief does to a task, and what the gate does with what's left.

The bug this file exists for: "not done" used to mark a task 'dropped', which
is not in store.UNFINISHED, so the one answer meaning "I didn't finish this"
was the only one that stopped it carrying into the next session.

store.session_label is tested here too — it is what the debrief and the
dashboard print where the statement used to go.

Run with:  python3 -m unittest discover tests
"""

import contextlib
import io
import tempfile
import unittest
from pathlib import Path

from gate import __main__ as gate_main
from gate import config, debrief, store, ui
from gate.ui import GateAborted


class Answers:
    """Stands in for ui.confirm_choice. Scripted, one answer per call; an
    'abort' entry raises GateAborted where a Ctrl-C would."""

    def __init__(self, *answers: str):
        self.queue = list(answers)
        self.prompts: list[str] = []

    def __call__(self, prompt: str, choices: str) -> str:
        self.prompts.append(prompt)
        answer = self.queue.pop(0)
        if answer == "abort":
            raise GateAborted
        assert answer in choices, f"{answer!r} is not one of {choices!r}"
        return answer


class DebriefCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self._saved_store_path = config.STORE_PATH
        config.STORE_PATH = Path(self._tmp.name) / "store.db"
        self.conn = store.connect()
        store.init(self.conn)
        self._saved_choice = ui.confirm_choice
        # The gate talks to a console; keep the suite's output clean and keep
        # what it said available to assert on.
        self.out = io.StringIO()
        printed = contextlib.redirect_stdout(self.out)
        printed.__enter__()
        self.addCleanup(printed.__exit__, None, None, None)

    def tearDown(self):
        ui.confirm_choice = self._saved_choice
        self.conn.close()
        config.STORE_PATH = self._saved_store_path
        self._tmp.cleanup()

    def answer(self, *answers: str) -> Answers:
        # debrief reaches it through the ui module.
        scripted = Answers(*answers)
        ui.confirm_choice = scripted
        return scripted

    def session(self, *titles: str) -> int:
        return store.commit_draft(self.conn, "", 60, "manual", list(titles))

    def statuses(self, session_id: int) -> list[str]:
        return [t["status"] for t in store.get_tasks(self.conn, session_id)]


class TestResolveTasks(DebriefCase):
    def test_not_done_carries(self):
        """The regression test: 'n' must leave the task unfinished so the
        carry copies it into the backlog."""
        sid = self.session("carry me", "finished")
        self.answer("n", "d")
        debrief.resolve_tasks(self.conn, sid)

        self.assertEqual(self.statuses(sid), ["planned", "done"])
        # The carried count is the caller's line, not the debrief's.
        self.assertIn("1/2 done.", self.out.getvalue())
        store.close_session(self.conn, sid)
        self.assertEqual(store.carry_unfinished(self.conn, sid), 1)
        (carried,) = store.get_backlog(self.conn)
        self.assertEqual(carried["title"], "carry me")
        self.assertEqual(carried["status"], "planned")
        self.assertEqual(carried["carried_from"], store.get_tasks(self.conn, sid)[0]["id"])

    def test_drop_for_good_does_not_carry(self):
        sid = self.session("abandon me")
        self.answer("x")
        debrief.resolve_tasks(self.conn, sid)

        (task,) = store.get_tasks(self.conn, sid)
        self.assertEqual(task["status"], "dropped")
        self.assertIsNotNone(task["resolved_at"])
        self.assertIn("0/1 done, 1 dropped.", self.out.getvalue())
        store.close_session(self.conn, sid)
        self.assertEqual(store.carry_unfinished(self.conn, sid), 0)
        self.assertEqual(store.get_backlog(self.conn), [])

    def test_done_does_not_carry(self):
        sid = self.session("finished")
        self.answer("d")
        debrief.resolve_tasks(self.conn, sid)

        (task,) = store.get_tasks(self.conn, sid)
        self.assertEqual(task["status"], "done")
        self.assertIsNotNone(task["resolved_at"])
        store.close_session(self.conn, sid)
        self.assertEqual(store.carry_unfinished(self.conn, sid), 0)

    def test_doing_asked_about_and_carried(self):
        sid = self.session("half done")
        (task,) = store.get_tasks(self.conn, sid)
        store.resolve_task(self.conn, task["id"], "doing")
        scripted = self.answer("n")
        debrief.resolve_tasks(self.conn, sid)

        self.assertIn("(doing)", scripted.prompts[0])
        store.close_session(self.conn, sid)
        self.assertEqual(store.carry_unfinished(self.conn, sid), 1)


class TestRecoverySweep(DebriefCase):
    """The sweep asks nothing: what it carries is the first thing the welcome
    screen shows, and resolving happens there."""

    def test_asks_nothing_and_carries_everything_unfinished(self):
        sid = self.session("stranded otherwise", "also stranded", "finished")
        store.resolve_task(self.conn, store.get_tasks(self.conn, sid)[2]["id"], "done")
        self.answer()  # an empty script: any question would fail the test

        notes = gate_main.recover_open_sessions(self.conn)

        self.assertEqual(store.get_session(self.conn, sid)["close_reason"], "recovered")
        self.assertEqual(store.get_open_sessions(self.conn), [])
        self.assertEqual(
            [t["title"] for t in store.get_backlog(self.conn)],
            ["stranded otherwise", "also stranded"],
        )
        self.assertEqual(
            notes,
            [
                f"Session {sid} was never closed — no heartbeat, end time unknown.",
                "2 unfinished tasks carried over.",
            ],
        )
        # The notes are what was printed, so the terminal needs no second copy.
        self.assertEqual(self.out.getvalue().splitlines(), notes)

    def test_end_time_from_the_heartbeat_in_local_time(self):
        sid = self.session("a task")
        self.conn.execute(
            "UPDATE session SET last_heartbeat = '2026-09-16T23:37:05+00:00' WHERE id = ?",
            (sid,),
        )
        self.conn.commit()
        self.answer()

        notes = gate_main.recover_open_sessions(self.conn)

        stamp = gate_main.local_time("2026-09-16T23:37:05+00:00")
        self.assertEqual(notes[0], f"Session {sid} ended around {stamp}.")
        self.assertRegex(stamp, r"^\d{1,2}:37 [AP]M, Sep 1[67]$")

    def test_nothing_open_says_nothing(self):
        self.assertEqual(gate_main.recover_open_sessions(self.conn), [])

    def test_a_failing_note_still_carries(self):
        """mark_recovered has already run by then, so the session is never
        offered again — anything that skipped the carry would strand it."""
        sid = self.session("stranded otherwise")
        real = store.get_session
        self.addCleanup(setattr, store, "get_session", real)

        def broken(conn, session_id):
            raise RuntimeError("boom")

        store.get_session = broken

        with self.assertRaises(RuntimeError):
            gate_main.recover_open_sessions(self.conn)

        store.get_session = real
        self.assertEqual(store.get_session(self.conn, sid)["close_reason"], "recovered")
        self.assertEqual(len(store.get_backlog(self.conn)), 1)


class TestSessionLabel(DebriefCase):
    def test_statement_wins_when_there_is_one(self):
        sid = store.commit_draft(self.conn, "old style", 60, "manual", ["a task"])
        self.assertEqual(
            store.session_label(self.conn, store.get_session(self.conn, sid)), "old style"
        )

    def test_falls_back_to_the_tasks(self):
        sid = self.session("first thing", "second", "third")
        self.assertEqual(
            store.session_label(self.conn, store.get_session(self.conn, sid)),
            "first thing +2",
        )

    def test_single_task_has_no_suffix(self):
        sid = self.session("only thing")
        self.assertEqual(
            store.session_label(self.conn, store.get_session(self.conn, sid)), "only thing"
        )

    def test_empty_session(self):
        sid = store.commit_draft(self.conn, "", 60, "manual", [])
        self.assertEqual(
            store.session_label(self.conn, store.get_session(self.conn, sid)), "(no tasks)"
        )


if __name__ == "__main__":
    unittest.main()
