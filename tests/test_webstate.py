"""The welcome screen's state, asserted without a display.

The GTK gate's state *was* its widget tree, so none of this could be checked
without a compositor. webstate.py is pure, so all of it can.
"""

import sys
import unittest
from datetime import datetime
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from gate import ui, webstate  # noqa: E402

NOW = datetime(2026, 9, 19, 19, 30)  # a Saturday evening


def task(id, title, carry=0, finish=True, details=None):
    return ui.ActiveTask(id, title, carry, finish, details or ui.Details())


def screen(tasks=None, notes=(), error="", labels=()):
    return webstate.WelcomeScreen(
        list(tasks or []), list(notes), error, list(labels), now=NOW
    )


class TestState(unittest.TestCase):
    def test_greeting_and_date_come_from_ui(self):
        s = screen().state()
        self.assertEqual(s["greeting"], "Good evening")
        self.assertEqual(s["date"], "Saturday, September 19")

    def test_row_detail_is_uis_string(self):
        t = task(1, "HW2", carry=6, details=ui.Details("", "2026-09-19", ("school",)))
        row = screen([t]).state()["rows"][0]
        self.assertEqual(row["detail"], "carried 6× · due today · school")
        self.assertEqual(row["index"], 0)
        self.assertFalse(row["typed"])
        self.assertTrue(row["can_finish"])

    def test_typed_rows_say_new(self):
        s = screen()
        s.handle({"op": "add", "title": "  write it  "})
        row = s.state()["rows"][0]
        self.assertEqual(row["title"], "write it")
        self.assertEqual(row["detail"], "new")
        self.assertTrue(row["typed"])
        self.assertFalse(row["can_finish"])

    def test_empty_add_is_ignored(self):
        s = screen()
        s.handle({"op": "add", "title": "   "})
        self.assertEqual(s.state()["rows"], [])

    def test_status_counts_the_add_box(self):
        s = screen([task(1, "a")])
        self.assertEqual(s.state()["status"], "1 task this session.")
        s.handle({"op": "add_text", "text": "half typed"})
        self.assertEqual(s.state()["status"], "2 tasks this session.")

    def test_start_needs_a_task_but_the_box_counts(self):
        s = screen()
        self.assertFalse(s.state()["start_enabled"])
        self.assertEqual(s.state()["status"], webstate.EMPTY_HINT)
        s.handle({"op": "add_text", "text": "x"})
        self.assertTrue(s.state()["start_enabled"])


class TestMarks(unittest.TestCase):
    def test_delete_strikes_a_carried_row_and_undoes(self):
        s = screen([task(1, "a")])
        s.handle({"op": "mark", "index": 0, "mark": "delete"})
        self.assertEqual(s.state()["rows"][0]["mark"], "delete")
        self.assertEqual(s.state()["rows"][0]["detail"], "will be deleted")
        self.assertFalse(s.state()["start_enabled"])
        s.handle({"op": "mark", "index": 0, "mark": "delete"})
        self.assertIsNone(s.state()["rows"][0]["mark"])

    def test_deleting_a_typed_row_removes_it_outright(self):
        s = screen()
        s.handle({"op": "add", "title": "typed"})
        s.handle({"op": "mark", "index": 0, "mark": "delete"})
        self.assertEqual(s.state()["rows"], [])

    def test_done_is_refused_on_a_row_that_never_carried(self):
        s = screen([task(1, "a", finish=False)])
        s.handle({"op": "mark", "index": 0, "mark": "done"})
        self.assertIn("carried from an earlier session", s.state()["error"])
        self.assertIsNone(s.state()["rows"][0]["mark"])

    def test_marking_closes_an_open_panel(self):
        # Row numbers shift when a typed row goes, so the panel cannot be
        # trusted to still point at what it was opened on.
        s = screen([task(1, "a"), task(2, "b")])
        s.handle({"op": "details_open", "index": 1})
        self.assertIsNotNone(s.state()["panel"])
        s.handle({"op": "mark", "index": 0, "mark": "delete"})
        self.assertIsNone(s.state()["panel"])

    def test_a_junk_index_is_ignored_not_fatal(self):
        s = screen([task(1, "a")])
        for msg in ({"op": "mark", "index": 9, "mark": "delete"},
                    {"op": "mark", "index": 0, "mark": "nonsense"},
                    {"op": "details_open", "index": -1},
                    {"op": "nosuchop"}):
            s.handle(msg)
        self.assertEqual(len(s.state()["rows"]), 1)
        self.assertIsNone(s.state()["rows"][0]["mark"])


class TestDetails(unittest.TestCase):
    def test_open_seeds_the_panel_and_bumps_the_token(self):
        t = task(1, "a", details=ui.Details("note", "2026-09-20", ("x",)))
        s = screen([t])
        before = s.token
        s.handle({"op": "details_open", "index": 0})
        panel = s.state()["panel"]
        self.assertEqual(panel["notes"], "note")
        self.assertEqual(panel["due"], "2026-09-20")
        self.assertEqual(panel["labels"], ["x"])
        self.assertGreater(panel["token"], before)

    def test_unrelated_messages_do_not_bump_the_token(self):
        s = screen([task(1, "a")])
        s.handle({"op": "details_open", "index": 0})
        token = s.state()["panel"]["token"]
        s.handle({"op": "add_text", "text": "typing"})
        self.assertEqual(s.state()["panel"]["token"], token)

    def test_save_writes_details_and_closes(self):
        s = screen([task(1, "a")])
        s.handle({"op": "details_open", "index": 0})
        s.handle({"op": "details_save", "index": 0, "due": "tomorrow",
                  "notes": "  hi  ", "labels": ["School"], "pending": ""})
        self.assertIsNone(s.state()["panel"])
        row = s.rows.rows[0]
        self.assertEqual(row.details.due_date, "2026-09-20")
        self.assertEqual(row.details.notes, "hi")
        self.assertEqual(row.details.labels, ("School",))

    def test_a_half_typed_label_still_counts(self):
        s = screen([task(1, "a")])
        s.handle({"op": "details_open", "index": 0})
        s.handle({"op": "details_save", "index": 0, "due": "", "notes": "",
                  "labels": ["one"], "pending": "  two "})
        self.assertEqual(s.rows.rows[0].details.labels, ("one", "two"))

    def test_a_bad_due_date_keeps_the_panel_open(self):
        s = screen([task(1, "a")])
        s.handle({"op": "details_open", "index": 0})
        s.handle({"op": "details_save", "index": 0, "due": "20260920",
                  "notes": "", "labels": [], "pending": ""})
        panel = s.state()["panel"]
        self.assertIsNotNone(panel)
        self.assertEqual(panel["error"], ui.BAD_DUE)

    def test_a_marked_row_refuses_an_edit(self):
        s = screen([task(1, "a")])
        s.handle({"op": "details_open", "index": 0})
        s.rows.rows[0].mark = ui.DELETE
        s.handle({"op": "details_save", "index": 0, "due": "", "notes": "x",
                  "labels": [], "pending": ""})
        self.assertEqual(s.state()["panel"]["error"], ui.MARKED)

    def test_a_new_label_is_offered_on_every_row_afterwards(self):
        s = screen([task(1, "a"), task(2, "b")],
                   labels=[ui.Label("school", "#7aa2f7")])
        s.handle({"op": "details_open", "index": 0})
        s.handle({"op": "details_save", "index": 0, "due": "", "notes": "",
                  "labels": ["invented"], "pending": ""})
        offered = s.state()["labels"]
        self.assertEqual([l["name"] for l in offered], ["school", "invented"])
        self.assertTrue(offered[0]["exists"])
        self.assertFalse(offered[1]["exists"])


class TestSubmit(unittest.TestCase):
    def base(self, **kw):
        msg = {"op": "submit", "add_text": "", "minutes": "", "details": None}
        msg.update(kw)
        return msg

    def test_a_plan_lands(self):
        s = screen([task(1, "keep"), task(2, "drop"), task(3, "fin")])
        s.handle({"op": "mark", "index": 1, "mark": "delete"})
        s.handle({"op": "mark", "index": 2, "mark": "done"})
        s.handle(self.base(minutes="90"))
        self.assertIsNotNone(s.plan)
        self.assertEqual(s.plan.keep, [1])
        self.assertEqual(s.plan.delete, [2])
        self.assertEqual(s.plan.done, [3])
        self.assertEqual(s.plan.intended_minutes, 90)

    def test_the_add_box_is_absorbed_on_start(self):
        s = screen()
        s.handle(self.base(add_text="last thing"))
        self.assertIsNotNone(s.plan)
        self.assertEqual([t.title for t in s.plan.new], ["last thing"])

    def test_an_empty_list_refuses_to_start(self):
        s = screen()
        s.handle(self.base())
        self.assertIsNone(s.plan)
        self.assertEqual(s.state()["error"], webstate.EMPTY_HINT)

    def test_bad_minutes_refuse_to_start(self):
        s = screen([task(1, "a")])
        s.handle(self.base(minutes="nope"))
        self.assertIsNone(s.plan)
        self.assertIn("whole number above zero", s.state()["error"])

    def test_blank_minutes_are_open_ended(self):
        s = screen([task(1, "a")])
        s.handle(self.base(minutes="  "))
        self.assertIsNone(s.plan.intended_minutes)

    def test_start_saves_an_open_panel_first(self):
        s = screen([task(1, "a")])
        s.handle({"op": "details_open", "index": 0})
        s.handle(self.base(details={"index": 0, "due": "today", "notes": "n",
                                    "labels": [], "pending": ""}))
        self.assertIsNotNone(s.plan)
        self.assertEqual(s.plan.edits[1].due_date, "2026-09-19")
        self.assertEqual(s.plan.edits[1].notes, "n")

    def test_a_refused_panel_stops_the_start(self):
        s = screen([task(1, "a")])
        s.handle({"op": "details_open", "index": 0})
        s.handle(self.base(details={"index": 0, "due": "not-a-day", "notes": "",
                                    "labels": [], "pending": ""}))
        self.assertIsNone(s.plan)
        self.assertEqual(s.state()["panel"]["error"], ui.BAD_DUE)

    def test_only_changed_details_become_edits(self):
        # The app may be editing the same backlog; writing back merely-loaded
        # values would undo its edit.
        t = task(1, "a", details=ui.Details("keep", "2026-09-20", ("x",)))
        s = screen([t])
        s.handle({"op": "details_open", "index": 0})
        s.handle({"op": "details_save", "index": 0, "due": "2026-09-20",
                  "notes": "keep", "labels": ["x"], "pending": ""})
        s.handle(self.base())
        self.assertEqual(s.plan.edits, {})


if __name__ == "__main__":
    unittest.main()
