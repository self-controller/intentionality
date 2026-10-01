"""The welcome screen's rules, which both front ends share: ui.WelcomeList,
the terminal loop built on it, and the minutes and greeting helpers.

Run with:  python3 -m unittest discover tests
"""

import contextlib
import io
import unittest
from datetime import date, datetime
from unittest import mock

from gate import ui
from gate.ui import DELETE, DONE, ActiveTask, Details, NewTask, Row, WelcomeList

CARRIED = ActiveTask(1, "carried", 1, can_finish=True)
LOOSE = ActiveTask(2, "from a meeting", 0, can_finish=False)
DETAILED = ActiveTask(
    3, "detailed", 0, can_finish=False,
    details=Details("old notes", "2026-09-13", ("school", "urgent")),
)
TODAY = date(2026, 9, 11)  # a Friday


class TestWelcomeList(unittest.TestCase):
    def test_backlog_tasks_default_to_the_backlog_and_typed_ones_to_today(self):
        rows = WelcomeList([CARRIED, LOOSE])
        self.assertTrue(rows.add("  typed  "))
        self.assertFalse(rows.add("   "))
        self.assertEqual([row.today for row in rows.rows], [False, False, True])
        self.assertEqual(rows.today_count(), 1)
        self.assertEqual(rows.plan(20), ui.Plan([], [], [], [NewTask("typed")], 20))

    def test_ticking_moves_a_task_to_today_and_unticking_puts_it_back(self):
        rows = WelcomeList([CARRIED, LOOSE])
        rows.toggle_today(1)
        self.assertEqual(rows.today_count(), 1)
        self.assertEqual(rows.plan(None).today, [2])
        rows.toggle_today(1)
        self.assertEqual(rows.today_count(), 0)
        self.assertEqual(rows.plan(None).today, [])

    def test_an_unticked_typed_row_goes_to_the_backlog(self):
        rows = WelcomeList([])
        rows.add("later")
        rows.add("now")
        rows.toggle_today(0)
        p = rows.plan(None)
        self.assertEqual((p.new, p.new_backlog), ([NewTask("now")], [NewTask("later")]))

    def test_marks_toggle_and_undo(self):
        rows = WelcomeList([CARRIED, LOOSE])
        rows.toggle_today(1)
        self.assertIsNone(rows.toggle(0, DONE))
        self.assertIsNone(rows.toggle(1, DELETE))  # a mark takes the tick off
        self.assertEqual(rows.today_count(), 0)
        self.assertEqual(rows.plan(None), ui.Plan([], [1], [2], [], None))
        rows.toggle(1, DELETE)  # the same mark again is the undo: back to the backlog
        self.assertEqual(rows.plan(None), ui.Plan([], [1], [], [], None))
        rows.toggle_today(1)
        rows.toggle(0, DELETE)  # a different mark replaces it
        self.assertEqual(rows.plan(None), ui.Plan([2], [], [1], [], None))

    def test_ticking_a_marked_row_takes_the_mark_off(self):
        rows = WelcomeList([CARRIED])
        rows.toggle(0, DELETE)
        rows.toggle_today(0)
        self.assertEqual(rows.plan(None), ui.Plan([1], [], [], [], None))

    def test_done_needs_a_session_to_be_recorded_in(self):
        rows = WelcomeList([LOOSE])
        rows.add("typed")
        for index in (0, 1):
            with self.subTest(index=index):
                self.assertIn("carried from an earlier session", rows.toggle(index, DONE))
        self.assertEqual(rows.today_count(), 1)

    def test_deleting_a_typed_row_removes_it(self):
        rows = WelcomeList([CARRIED])
        rows.add("typo")
        rows.add("real")
        self.assertIsNone(rows.toggle(1, DELETE))
        self.assertEqual([row.title for row in rows.rows], ["carried", "real"])
        self.assertEqual(rows.plan(None).new, [NewTask("real")])

    def test_every_shown_task_lands_exactly_once(self):
        tasks = [ActiveTask(i, f"t{i}", 0, True) for i in range(1, 7)]
        rows = WelcomeList(tasks)
        rows.toggle_today(0)
        rows.toggle(1, DONE)
        rows.toggle(2, DELETE)
        rows.toggle(4, DONE)
        rows.toggle(4, DONE)
        p = rows.plan(None)
        # Ticked, done and deleted are disjoint; the rest simply stay.
        marked = p.today + p.done + p.delete
        self.assertEqual(len(marked), len(set(marked)))
        self.assertEqual((p.today, p.done, p.delete), ([1], [2], [3]))


class TestDetails(unittest.TestCase):
    def test_only_changed_rows_are_edits(self):
        rows = WelcomeList([CARRIED, DETAILED])
        self.assertEqual(rows.plan(None).edits, {})
        # The same labels in another order and case is no change.
        self.assertIsNone(rows.set_details(1, Details("old notes", "2026-09-13", ("URGENT", "school"))))
        self.assertEqual(rows.plan(None).edits, {})
        changed = Details("new notes", None, ("school",))
        self.assertIsNone(rows.set_details(0, changed))
        self.assertEqual(rows.plan(None).edits, {1: changed})

    def test_details_are_cleaned(self):
        rows = WelcomeList([CARRIED])
        rows.set_details(0, Details("  padded \n", None, (" a ", "", "A", "b")))
        self.assertEqual(rows.rows[0].details, Details("padded", None, ("a", "b")))

    def test_a_marked_or_deleted_row_is_not_edited(self):
        rows = WelcomeList([CARRIED, DETAILED])
        rows.set_details(1, Details("changed"))
        rows.toggle(1, DELETE)
        self.assertEqual(rows.set_details(1, Details("again")), ui.MARKED)
        # A deleted task's edit is not written either.
        self.assertEqual(rows.plan(None).edits, {})

    def test_a_bad_date_is_refused(self):
        rows = WelcomeList([CARRIED])
        for bad in ("2026-9-1", "20260901", "2026-02-30", "soon"):
            with self.subTest(bad=bad):
                self.assertEqual(rows.set_details(0, Details(due_date=bad)), ui.BAD_DUE)
        self.assertEqual(rows.rows[0].details, Details())

    def test_typed_rows_take_details_along(self):
        rows = WelcomeList([])
        rows.add("typed")
        rows.set_details(0, Details("n", "2026-09-12", ("x",)))
        self.assertEqual(
            rows.plan(None).new, [NewTask("typed", Details("n", "2026-09-12", ("x",)))]
        )
        self.assertEqual(rows.plan(None).edits, {})


class TestDueWords(unittest.TestCase):
    def test_parse_due(self):
        self.assertIsNone(ui.parse_due("  ", TODAY))
        self.assertIsNone(ui.parse_due("-", TODAY))
        self.assertEqual(ui.parse_due(" Today ", TODAY), "2026-09-11")
        self.assertEqual(ui.parse_due("tomorrow", TODAY), "2026-09-12")
        self.assertEqual(ui.parse_due("2026-10-01", TODAY), "2026-10-01")
        for bad in ("2026-9-1", "20261001", "2026-13-01", "next week"):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                ui.parse_due(bad, TODAY)

    def test_no_date_says_nothing(self):
        self.assertEqual(ui.due_label(None, TODAY), "")
        self.assertEqual(ui.due_label("", TODAY), "")

    def test_junk_says_nothing_rather_than_raising(self):
        # due_label runs at login: a bad row costs its label, never the gate.
        for junk in ("tomorrow", "2026-13-01", "09/12/2026"):
            with self.subTest(junk=junk):
                self.assertEqual(ui.due_label(junk, TODAY), "")

    def test_relative_words(self):
        self.assertEqual(ui.due_label("2026-09-09", TODAY), "overdue, was due Sep 9")
        self.assertEqual(ui.due_label("2026-09-11", TODAY), "due today")
        self.assertEqual(ui.due_label("2026-09-12", TODAY), "due tomorrow")
        self.assertEqual(ui.due_label("2026-09-16", TODAY), "due Wednesday")
        # A week out, the weekday would be ambiguous.
        self.assertEqual(ui.due_label("2026-09-18", TODAY), "due Sep 18")

    def test_row_detail(self):
        for row, expected in (
            (Row("typed"), ""),
            (Row("c", ActiveTask(1, "c", 2, True)), ""),
            (Row("d", ActiveTask(1, "d", 0, False, Details(due_date="2026-09-11"))), "due today"),
            (Row("x", DETAILED), "due Sunday · school, urgent · notes"),
            (
                Row("c", ActiveTask(1, "c", 1, True, Details(due_date="2026-09-12", labels=("a",)))),
                "due tomorrow · a",
            ),
        ):
            with self.subTest(title=row.title):
                self.assertEqual(ui.row_detail(row, TODAY), expected)

    def test_row_detail_follows_edits(self):
        rows = WelcomeList([DETAILED])
        rows.set_details(0, Details())
        self.assertEqual(ui.row_detail(rows.rows[0], TODAY), "")


class TerminalCase(unittest.TestCase):
    def setUp(self):
        self.addCleanup(setattr, ui, "_input", ui._input)
        self.addCleanup(setattr, ui, "backend", None)
        ui.backend = None
        self.out = io.StringIO()
        printed = contextlib.redirect_stdout(self.out)
        printed.__enter__()
        self.addCleanup(printed.__exit__, None, None, None)

    def script(self, *lines):
        queue = list(lines)
        self.prompts = []

        def answer(prompt):
            self.prompts.append(prompt)
            return queue.pop(0)

        ui._input = answer


FAKE_TODAY = mock.patch.object(
    ui, "date", type("FakeDate", (date,), {"today": classmethod(lambda cls: TODAY)})
)


class TestTerminalWelcome(TerminalCase):
    def setUp(self):
        super().setUp()
        FAKE_TODAY.start()
        self.addCleanup(FAKE_TODAY.stop)

    def test_blank_line_needs_a_task(self):
        # Nothing is ticked, then a deleted row is ticked back in: only then
        # does a blank line start.
        self.script("x 1", "", "t 1", "", "")
        p = ui.welcome([CARRIED], ["already printed"])
        self.assertEqual(p, ui.Plan([1], [], [], [], None))
        out = self.out.getvalue()
        self.assertIn("e N = due date, notes, labels", out)
        self.assertEqual(out.count(ui.EMPTY_HINT), 1)
        self.assertIn("  1. [ ] carried  [delete]", out)
        # The recovery sweep printed the notes; the terminal doesn't repeat them.
        self.assertNotIn("already printed", out)

    def test_add_mark_and_minutes(self):
        self.script("write report", "d 1", "D1", "d1", "t 2", "", "abc", "0", "45")
        p = ui.welcome([CARRIED, LOOSE])
        self.assertEqual(p, ui.Plan([2], [1], [], [NewTask("write report")], 45))
        out = self.out.getvalue()
        self.assertEqual(out.count("whole number above zero"), 2)
        self.assertIn("  1. [ ] carried  [done]", out)
        self.assertIn("  3. [x] write report", out)

    def test_bad_numbers_and_refusals_are_said(self):
        self.script("x 9", "d 1", "t 1", "", "")
        p = ui.welcome([LOOSE], error="Those tasks changed while you were here.")
        self.assertEqual(p.today, [2])
        out = self.out.getvalue()
        self.assertIn("Those tasks changed while you were here.", out)
        self.assertIn("There is no task 9.", out)
        self.assertIn("carried from an earlier session", out)

    def test_details_by_number(self):
        labels = [ui.Label("school", "#7aa2f7"), ui.Label("urgent", "#9ece6a")]
        self.script(
            "e 1", "someday", "tomorrow", "read chapter 3", "school, Errands ,,",
            "e 2", "", "", "",  # all kept
            "e 3", "-", "-", "-",  # all cleared
            "new one", "e 4", "2026-10-01", "", "school",
            "x 1", "e 1",  # a marked row is refused before any question
            "", "",
        )
        p = ui.welcome([CARRIED, LOOSE, DETAILED], labels=labels)
        self.assertEqual(p.today, [])
        self.assertEqual(p.delete, [1])
        # Row 1 was edited and then deleted: nothing to write. Row 2 kept all.
        # Row 3 is edited although it stays in the backlog.
        self.assertEqual(p.edits, {3: Details()})
        self.assertEqual(p.new, [NewTask("new one", Details("", "2026-10-01", ("school",)))])
        out = self.out.getvalue()
        self.assertIn(ui.BAD_DUE, out)
        self.assertIn(ui.MARKED, out)
        self.assertIn("Labels there are: school, urgent.", out)
        self.assertIn("due tomorrow · school, Errands · notes", out)
        prompts = "".join(self.prompts)
        self.assertIn("[2026-09-13]", prompts)
        self.assertIn("[old notes]", prompts)
        self.assertIn("[school, urgent]", prompts)

    def test_details_without_labels_yet(self):
        self.script("e 1", "", "a note that runs on well past forty characters", "", "",
                    "t 1", "", "")
        p = ui.welcome([CARRIED])
        self.assertEqual(p.edits[1].notes, "a note that runs on well past forty characters")
        self.assertIn("Labels there are: none yet.", self.out.getvalue())
        self.assertIn("[none]", self.prompts[2])

    def test_long_notes_are_shortened_in_the_prompt(self):
        many = ActiveTask(1, "t", 0, False, Details("line one\nline two"))
        self.script("e 1", "", "", "", "t 1", "", "")
        p = ui.welcome([many])
        self.assertEqual(p.edits, {})
        self.assertIn("[line one…]", self.prompts[2])

    def test_ctrl_c_aborts(self):
        def interrupted(prompt):
            raise ui.GateAborted

        ui._input = interrupted
        with self.assertRaises(ui.GateAborted):
            ui.welcome([CARRIED])


class TestHelpers(unittest.TestCase):
    def test_parse_minutes(self):
        self.assertIsNone(ui.parse_minutes("  "))
        self.assertEqual(ui.parse_minutes(" 90 "), 90)
        for bad in ("0", "-5", "ninety", "1.5"):
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                ui.parse_minutes(bad)

    def test_greeting(self):
        for hour, word in ((4, "evening"), (5, "morning"), (12, "afternoon"), (17, "evening")):
            with self.subTest(hour=hour):
                title, date_line = ui.greeting(datetime(2026, 9, 17, hour))
                self.assertEqual(title, f"Good {word}")
                self.assertEqual(date_line, "Thursday, September 17")


if __name__ == "__main__":
    unittest.main()
