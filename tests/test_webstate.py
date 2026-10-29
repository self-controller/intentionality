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
        self.assertEqual(row["detail"], "due today · school")
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

    def test_backlog_rows_start_unticked_and_typed_rows_ticked(self):
        s = screen([task(1, "a")])
        s.handle({"op": "add", "title": "typed"})
        self.assertEqual([r["today"] for r in s.state()["rows"]], [False, True])

    def test_no_count_line_but_start_still_counts_ticks_and_the_add_box(self):
        s = screen([task(1, "a")])
        self.assertNotIn("status", s.state())
        self.assertFalse(s.state()["start_enabled"])
        s.handle({"op": "today", "index": 0})
        self.assertTrue(s.state()["start_enabled"])
        s.handle({"op": "today", "index": 0})
        s.handle({"op": "add_text", "text": "half typed"})
        self.assertTrue(s.state()["start_enabled"])

    def test_rows_carry_their_labels_and_a_save_moves_them(self):
        t = task(1, "a", details=ui.Details("", None, ("school", "urgent")))
        s = screen([t, task(2, "b")])
        self.assertEqual([r["labels"] for r in s.state()["rows"]], [["school", "urgent"], []])
        s.handle({"op": "details_open", "index": 1})
        s.handle({"op": "details_save", "index": 1, "due": "", "notes": "",
                  "labels": ["errands"], "pending": ""})
        self.assertEqual(s.state()["rows"][1]["labels"], ["errands"])

    def test_ticking_twice_puts_a_row_back_in_the_backlog(self):
        s = screen([task(1, "a")])
        s.handle({"op": "today", "index": 0})
        s.handle({"op": "today", "index": 0})
        self.assertFalse(s.state()["rows"][0]["today"])
        self.assertFalse(s.state()["start_enabled"])

    def test_marking_unticks_and_ticking_unmarks(self):
        s = screen([task(1, "a")])
        s.handle({"op": "today", "index": 0})
        s.handle({"op": "mark", "index": 0, "mark": "delete"})
        row = s.state()["rows"][0]
        self.assertEqual((row["mark"], row["today"], row["detail"]), ("delete", False, "will be deleted"))
        s.handle({"op": "today", "index": 0})
        row = s.state()["rows"][0]
        self.assertEqual((row["mark"], row["today"]), (None, True))

    def test_a_bad_row_number_is_ignored(self):
        s = screen([task(1, "a")])
        s.handle({"op": "today", "index": 5})
        self.assertFalse(s.state()["rows"][0]["today"])

    def test_start_needs_a_task_but_the_box_counts(self):
        s = screen()
        self.assertFalse(s.state()["start_enabled"])
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
        self.assertEqual(panel["error"], "")

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


class TestAddWithDetails(unittest.TestCase):
    FIELDS = {"due": "tomorrow", "notes": "  bring the essay ", "labels": ["School"],
              "pending": " reading "}

    def test_a_typed_task_lands_with_its_details(self):
        s = screen()
        s.handle({"op": "add", "title": "write", "details": dict(self.FIELDS)})
        row = s.rows.rows[0]
        self.assertEqual(row.title, "write")
        self.assertEqual(row.details.due_date, "2026-09-20")
        self.assertEqual(row.details.notes, "bring the essay")
        self.assertEqual(row.details.labels, ("School", "reading"))
        self.assertEqual(s.state()["rows"][0]["labels"], ["School", "reading"])

    def test_landing_bumps_added_and_nothing_else_does(self):
        s = screen()
        before = s.state()["added"]
        s.handle({"op": "add_text", "text": "x"})
        s.handle({"op": "add", "title": "   ", "details": dict(self.FIELDS)})
        self.assertEqual(s.state()["added"], before)
        s.handle({"op": "add", "title": "x"})
        self.assertEqual(s.state()["added"], before + 1)

    def test_a_bad_due_date_refuses_and_keeps_the_box(self):
        s = screen()
        s.handle({"op": "add_text", "text": "write"})
        s.handle({"op": "add", "title": "write",
                  "details": {"due": "20260920", "notes": "", "labels": [], "pending": ""}})
        st = s.state()
        self.assertEqual(st["rows"], [])
        self.assertEqual(st["added"], 0)
        self.assertEqual(st["add_error"], "")
        # Still counted: the task is sitting in the box.
        self.assertTrue(st["start_enabled"])

    def test_its_new_labels_are_offered_afterwards(self):
        s = screen(labels=[ui.Label("school", "#7aa2f7")])
        s.handle({"op": "add", "title": "a",
                  "details": {"due": "", "notes": "", "labels": ["SCHOOL", "gym"], "pending": ""}})
        offered = s.state()["labels"]
        self.assertEqual([l["name"] for l in offered], ["school", "gym"])
        self.assertFalse(offered[1]["exists"])

    def test_start_keeps_the_box_details(self):
        s = screen()
        s.handle({"op": "submit", "add_text": "last thing", "minutes": "", "details": None,
                  "add_details": dict(self.FIELDS)})
        self.assertIsNotNone(s.plan)
        self.assertEqual(s.plan.new[0].details.labels, ("School", "reading"))
        self.assertEqual(s.plan.new[0].details.due_date, "2026-09-20")

    def test_a_refused_box_stops_the_start(self):
        s = screen([task(1, "a")])
        s.handle({"op": "today", "index": 0})
        s.handle({"op": "submit", "add_text": "b", "minutes": "", "details": None,
                  "add_details": {"due": "soon", "notes": "", "labels": [], "pending": ""}})
        self.assertIsNone(s.plan)
        self.assertEqual(len(s.rows.rows), 1)

    def test_a_refused_start_does_not_add_the_box_twice(self):
        # The box lands even when bad minutes then stop the start; added tells
        # the front end to empty it, so a second Start cannot add it again.
        s = screen()
        s.handle({"op": "submit", "add_text": "once", "minutes": "nope", "details": None,
                  "add_details": None})
        self.assertIsNone(s.plan)
        self.assertEqual([r.title for r in s.rows.rows], ["once"])
        self.assertEqual(s.state()["added"], 1)


class TestSubmit(unittest.TestCase):
    def base(self, **kw):
        msg = {"op": "submit", "add_text": "", "minutes": "", "details": None}
        msg.update(kw)
        return msg

    def test_a_plan_lands(self):
        s = screen([task(1, "today"), task(2, "drop"), task(3, "fin"), task(4, "later")])
        s.handle({"op": "today", "index": 0})
        s.handle({"op": "mark", "index": 1, "mark": "delete"})
        s.handle({"op": "mark", "index": 2, "mark": "done"})
        s.handle(self.base(minutes="90"))
        self.assertIsNotNone(s.plan)
        self.assertEqual(s.plan.today, [1])
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
        self.assertEqual(s.state()["error"], "")

    def test_bad_minutes_refuse_to_start(self):
        s = screen([task(1, "a")])
        s.handle({"op": "today", "index": 0})
        s.handle(self.base(minutes="nope"))
        self.assertIsNone(s.plan)
        self.assertIn("whole number above zero", s.state()["error"])

    def test_blank_minutes_are_open_ended(self):
        s = screen([task(1, "a")])
        s.handle({"op": "today", "index": 0})
        s.handle(self.base(minutes="  "))
        self.assertIsNone(s.plan.intended_minutes)

    def test_unticked_rows_alone_refuse_to_start(self):
        s = screen([task(1, "a")])
        s.handle(self.base())
        self.assertIsNone(s.plan)
        self.assertEqual(s.state()["error"], "")

    def test_a_typed_row_left_unticked_goes_to_the_backlog(self):
        s = screen([task(1, "a")])
        s.handle({"op": "today", "index": 0})
        s.handle({"op": "add", "title": "later"})
        s.handle({"op": "today", "index": 1})
        s.handle(self.base())
        self.assertEqual([t.title for t in s.plan.new_backlog], ["later"])
        self.assertEqual(s.plan.new, [])

    def test_start_saves_an_open_panel_first(self):
        s = screen([task(1, "a")])
        s.handle({"op": "today", "index": 0})
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
        self.assertEqual(s.state()["panel"]["error"], "")

    def test_only_changed_details_become_edits(self):
        # The app may be editing the same backlog; writing back merely-loaded
        # values would undo its edit.
        t = task(1, "a", details=ui.Details("keep", "2026-09-20", ("x",)))
        s = screen([t])
        s.handle({"op": "today", "index": 0})
        s.handle({"op": "details_open", "index": 0})
        s.handle({"op": "details_save", "index": 0, "due": "2026-09-20",
                  "notes": "keep", "labels": ["x"], "pending": ""})
        s.handle(self.base())
        self.assertEqual(s.plan.edits, {})


if __name__ == "__main__":
    unittest.main()


class TestSorting(unittest.TestCase):
    """Jev filing typed tasks: which rows are asked about, and when an answer
    may still go on."""

    LABELS = [ui.Label("school", "#7aa2f7", ("HW 6",)), ui.Label("home", "#9ece6a")]

    def typed(self, s, title, **details):
        s.handle({"op": "add", "title": title, **({"details": details} if details else {})})
        return s.rows.rows[-1]

    def test_only_typed_unlabelled_rows_are_asked_about_and_each_once(self):
        s = screen([task(1, "carried")], labels=self.LABELS)
        bare = self.typed(s, "HW 7")
        self.typed(s, "tagged", labels=["home"])
        self.assertEqual(s.to_sort(), [bare])
        self.assertEqual(s.to_sort(), [])
        later = self.typed(s, "nap")
        self.assertEqual(s.to_sort(), [later])

    def test_a_row_saved_by_hand_is_not_asked_about_even_with_no_labels(self):
        s = screen(labels=self.LABELS)
        row = self.typed(s, "HW 7", labels=["home"])
        s.handle({"op": "details_open", "index": 0})
        s.handle({"op": "details_save", "index": 0, "labels": []})
        self.assertEqual(row.details.labels, ())
        self.assertEqual(s.to_sort(), [])

    def test_an_answer_goes_on_and_moves_the_row(self):
        s = screen(labels=self.LABELS)
        row = self.typed(s, "HW 7")
        s.to_sort()
        self.assertTrue(s.sorted(row, "school"))
        self.assertEqual(s.state()["rows"][0]["labels"], ["school"])
        self.assertFalse(s.sorted(row, None))

    def test_a_late_answer_loses_to_the_user(self):
        s = screen(labels=self.LABELS)
        gone = self.typed(s, "gone")
        mine = self.typed(s, "mine")
        open_ = self.typed(s, "open")
        s.to_sort()
        s.handle({"op": "mark", "index": 0, "mark": ui.DELETE})  # removed outright
        self.assertFalse(s.sorted(gone, "school"))
        s.handle({"op": "details_open", "index": 0})
        s.handle({"op": "details_save", "index": 0, "labels": ["home"]})
        self.assertFalse(s.sorted(mine, "school"))
        self.assertEqual(mine.details.labels, ("home",))
        s.handle({"op": "details_open", "index": 1})
        self.assertFalse(s.sorted(open_, "school"))
        self.assertEqual(open_.details.labels, ())

    def test_replan_takes_labels_that_landed_after_start(self):
        s = screen(labels=self.LABELS)
        s.handle({"op": "submit", "minutes": "", "add_text": "HW 7"})
        self.assertEqual(s.plan.new, [ui.NewTask("HW 7")])
        (row,) = s.to_sort()
        s.sorted(row, "school")
        self.assertEqual(s.replan().new, [ui.NewTask("HW 7", ui.Details(labels=("school",)))])

    def test_candidates_are_the_stores_labels_then_this_screens(self):
        s = screen(labels=self.LABELS)
        self.typed(s, "x", labels=["Errands", "SCHOOL"])
        self.assertEqual(
            s.candidates(),
            [ui.autolabel.Candidate("school", ("HW 6",)), ui.autolabel.Candidate("home"),
             ui.autolabel.Candidate("Errands")],
        )
