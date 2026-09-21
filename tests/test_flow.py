"""The gate's flow: the welcome screen in, one committed session out.

A scripted backend stands in for the screen. What is pinned: the tasks the
screen is shown, that the plan it returns lands as it said -- details
included, and only the details that were changed -- and that there is no way
out of run() with an empty session.

Run with:  python3 -m unittest discover tests
"""

import contextlib
import io
import tempfile
import unittest
from datetime import date
from pathlib import Path

from gate import config, flow, store, ui
from gate.ui import Details, NewTask


class Screen:
    """A welcome screen that answers from a script. Each answer is a function
    of the tasks shown, so a script can refer to them by title."""

    def __init__(self, *answers):
        self.answers = list(answers)
        self.calls: list[tuple] = []

    def welcome(self, tasks, notes, error, labels):
        self.calls.append((tasks, notes, error, labels))
        return self.answers.pop(0)({t.title: t for t in tasks})


def plan(keep=(), done=(), delete=(), new=(), minutes=None, edits=None):
    new = [n if isinstance(n, NewTask) else NewTask(n) for n in new]
    return ui.Plan(list(keep), list(done), list(delete), new, minutes, edits or {})


class TestRun(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.addCleanup(setattr, config, "STORE_PATH", config.STORE_PATH)
        config.STORE_PATH = Path(tmp.name) / "store.db"
        self.conn = store.connect()
        self.addCleanup(self.conn.close)
        store.init(self.conn)
        self.addCleanup(setattr, ui, "backend", None)
        self.out = io.StringIO()
        printed = contextlib.redirect_stdout(self.out)
        printed.__enter__()
        self.addCleanup(printed.__exit__, None, None, None)

    def carry(self, *titles):
        old = store.commit_draft(self.conn, "", None, "manual", list(titles))
        store.close_session(self.conn, old)
        store.carry_unfinished(self.conn, old)
        return old

    def titles(self, session_id):
        return [t["title"] for t in store.get_tasks(self.conn, session_id)]

    def test_the_screen_sees_the_backlog_and_the_notes(self):
        self.carry("write report")
        with self.conn:
            self.conn.execute("UPDATE task SET due_date = ? WHERE session_id IS NULL",
                              (date.today().isoformat(),))
            self.conn.execute(
                "INSERT INTO task (session_id, title, position, created_at)"
                " VALUES (NULL, 'from a meeting', 9, ?)",
                (store.now(),),
            )
        ui.backend = screen = Screen(lambda t: plan(keep=[x.id for x in t.values()]))

        flow.run(self.conn, ["Session 1 ended around 9:00 AM, Sep 17."])

        ((tasks, notes, error, labels),) = screen.calls
        today = date.today()
        self.assertEqual(
            [(t.title, ui.row_detail(ui.Row(t.title, t), today), t.can_finish) for t in tasks],
            [("write report", "carried 1× · due today", True), ("from a meeting", "", False)],
        )
        self.assertEqual(labels, [])
        self.assertEqual(notes, ["Session 1 ended around 9:00 AM, Sep 17."])
        self.assertEqual(error, "")

    def test_keep_all_puts_the_backlog_on_the_board_first(self):
        self.carry("a", "b")
        ui.backend = Screen(lambda t: plan(keep=[t["a"].id, t["b"].id], new=["c"], minutes=25))

        sid = flow.run(self.conn)

        self.assertEqual(self.titles(sid), ["a", "b", "c"])
        self.assertEqual(store.get_backlog(self.conn), [])
        session = store.get_session(self.conn, sid)
        self.assertEqual(session["intended_minutes"], 25)
        self.assertIsNotNone(session["checkpoint_due_at"])
        self.assertIn(f"Session {sid} started — 3 task(s).", self.out.getvalue())

    def test_done_and_delete_land(self):
        old = self.carry("finished", "abandoned", "still on")
        ui.backend = Screen(lambda t: plan(
            keep=[t["still on"].id], done=[t["finished"].id], delete=[t["abandoned"].id]
        ))

        sid = flow.run(self.conn)

        self.assertEqual(self.titles(sid), ["still on"])
        self.assertEqual(store.get_backlog(self.conn), [])
        self.assertEqual(
            [t["status"] for t in store.get_tasks(self.conn, old)], ["done", "planned", "planned"]
        )

    def test_finished_and_dropped_backlog_rows_are_not_shown(self):
        with self.conn:
            for title, status in (("live", "planned"), ("stale", "dropped")):
                self.conn.execute(
                    "INSERT INTO task (session_id, title, position, status, created_at)"
                    " VALUES (NULL, ?, 1, ?, ?)",
                    (title, status, store.now()),
                )
        ui.backend = screen = Screen(lambda t: plan(keep=[t["live"].id]))
        flow.run(self.conn)
        self.assertEqual([t.title for t in screen.calls[0][0]], ["live"])

    def test_an_empty_plan_is_asked_again(self):
        self.carry("tempting to skip")
        ui.backend = screen = Screen(
            lambda t: plan(delete=[t["tempting to skip"].id]),
            lambda t: plan(new=["one real task"]),
        )

        sid = flow.run(self.conn)

        self.assertEqual(screen.calls[1][2], flow.EMPTY)
        # The refused answer wrote nothing: the task is still there to see.
        self.assertEqual([t.title for t in screen.calls[1][0]], ["tempting to skip"])
        self.assertEqual(self.titles(sid), ["one real task"])
        self.assertEqual([t["title"] for t in store.get_backlog(self.conn)], ["tempting to skip"])

    def test_a_kept_task_that_vanished_is_asked_again(self):
        old = self.carry("pulled elsewhere", "marked done")

        def answer_after_the_app_moved(t):
            # The desktop app pulls the task while the screen is up.
            store.pull_from_backlog(self.conn, old, [t["pulled elsewhere"].id])
            return plan(keep=[t["pulled elsewhere"].id], done=[t["marked done"].id])

        ui.backend = screen = Screen(answer_after_the_app_moved, lambda t: plan(new=["fresh"]))

        sid = flow.run(self.conn)

        self.assertEqual(screen.calls[1][2], flow.VANISHED)
        # Rolled back: the done mark did not land, and the task is shown again.
        self.assertEqual([t.title for t in screen.calls[1][0]], ["marked done"])
        self.assertEqual(self.titles(sid), ["fresh"])
        self.assertEqual(
            [t["status"] for t in store.get_tasks(self.conn, old)], ["planned", "planned", "planned"]
        )


class TestDetails(TestRun):
    def details(self, task_id):
        row = self.conn.execute(
            "SELECT notes, due_date FROM task WHERE id = ?", (task_id,)
        ).fetchone()
        labels = store.labels_by_task(self.conn).get(task_id, [])
        return Details(row["notes"], row["due_date"], tuple(labels))

    def set_details(self, task_id, details):
        """What the app's editor does to a backlog row."""
        with self.conn:
            self.assertTrue(store._set_details(self.conn, task_id, None, details))

    def test_the_screen_sees_details_and_every_label(self):
        self.carry("tagged")
        (task,) = store.get_backlog(self.conn)
        with self.conn:
            # A preset nobody wears yet is offered all the same.
            self.conn.execute("INSERT INTO label (name, color) VALUES ('preset', '#123456')")
        self.set_details(task["id"], Details("n", "2026-10-01", ("Zeta", "alpha")))
        ui.backend = screen = Screen(lambda t: plan(keep=[t["tagged"].id]))

        flow.run(self.conn)

        ((tasks, _, _, labels),) = screen.calls
        self.assertEqual(tasks[0].details, Details("n", "2026-10-01", ("alpha", "Zeta")))
        self.assertEqual([label.name for label in labels], ["alpha", "preset", "Zeta"])
        self.assertEqual(labels[1], ui.Label("preset", "#123456"))

    def test_edits_and_new_details_land(self):
        self.carry("edited", "untouched")
        with self.conn:
            self.conn.execute("INSERT INTO label (name, color) VALUES ('school', '#123456')")
        ui.backend = Screen(lambda t: plan(
            keep=[t["edited"].id, t["untouched"].id],
            new=[NewTask("typed", Details("typed notes", "2026-10-02", ("School", "brand new")))],
            edits={t["edited"].id: Details("edited notes", "2026-10-01", ("school",))},
        ))

        sid = flow.run(self.conn)

        edited, untouched, typed = store.get_tasks(self.conn, sid)
        self.assertEqual(self.details(edited["id"]), Details("edited notes", "2026-10-01", ("school",)))
        self.assertEqual(self.details(untouched["id"]), Details())
        # "School" found the existing label, whatever its case; "brand new"
        # was created with a colour from the app's palette.
        self.assertEqual(self.details(typed["id"]), Details("typed notes", "2026-10-02", ("brand new", "school")))
        colors = dict(self.conn.execute("SELECT name, color FROM label").fetchall())
        self.assertEqual(colors["school"], "#123456")
        self.assertIn(colors["brand new"], store.LABEL_COLORS)
        self.assertEqual(len(colors), 2)

    def test_an_unedited_task_keeps_what_the_app_wrote_meanwhile(self):
        self.carry("shared")

        def answer_after_the_app_edited(t):
            with self.conn:
                self.conn.execute(
                    "UPDATE task SET notes = 'from the app' WHERE id = ?", (t["shared"].id,)
                )
            return plan(keep=[t["shared"].id])

        ui.backend = Screen(answer_after_the_app_edited)
        sid = flow.run(self.conn)
        (task,) = store.get_tasks(self.conn, sid)
        self.assertEqual(task["notes"], "from the app")

    def test_an_edit_to_a_task_the_app_took_is_dropped(self):
        old = self.carry("taken", "stays")
        taken = []

        def answer_after_the_app_pulled(t):
            taken.append(t["taken"].id)
            store.pull_from_backlog(self.conn, old, taken)
            return plan(
                keep=[t["taken"].id, t["stays"].id],
                edits={t["taken"].id: Details("gate notes", labels=("gate",))},
            )

        ui.backend = Screen(answer_after_the_app_pulled)
        sid = flow.run(self.conn)
        self.assertEqual(self.titles(sid), ["stays"])
        self.assertEqual(self.details(taken[0]), Details())
        self.assertEqual(self.conn.execute("SELECT COUNT(*) FROM label").fetchone()[0], 0)


if __name__ == "__main__":
    unittest.main()
