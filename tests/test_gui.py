"""The graphical front end's seam, without a display.

Five things are pinned: ui hands exactly the three questions to a backend
(and nothing else, so the terminal helpers built on them keep working), the
prompt-to-button parsing that gives the buttons their words, the environment
the webview is started in, that the window offers no way out but SIGINT, and
`gate handoff`, the half of the login gate that runs after the compositor is
gone.
GTK itself is not exercised here — there is no display in the test run. The
welcome screen's list rules are ui.WelcomeList, tested in test_welcome.py.

Run with:  python3 -m unittest discover tests
"""

import contextlib
import io
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

from gate import __main__ as gate_main
from gate import config, gui, handoff, store, ui


class FakeBackend:
    def __init__(self, choices=(), plans=()):
        self.choices, self.plans = list(choices), list(plans)
        self.seen: list[tuple] = []

    def confirm_choice(self, prompt, choices):
        self.seen.append(("confirm", prompt, choices))
        return self.choices.pop(0)

    def welcome(self, tasks, notes, error, labels):
        self.seen.append(("welcome", tasks, notes, error, labels))
        return self.plans.pop(0)


class BackendCase(unittest.TestCase):
    def setUp(self):
        self.addCleanup(setattr, ui, "backend", None)
        self.out = io.StringIO()
        printed = contextlib.redirect_stdout(self.out)
        printed.__enter__()
        self.addCleanup(printed.__exit__, None, None, None)


class TestUiDispatch(BackendCase):
    def test_confirm_and_welcome_go_straight_through(self):
        plan = ui.Plan(keep=[], done=[], delete=[], new=[ui.NewTask("x")], intended_minutes=None)
        ui.backend = back = FakeBackend(choices=["n"], plans=[plan])
        task = ui.ActiveTask(7, "carried", 1, True)
        label = ui.Label("school", "#7aa2f7")
        self.assertEqual(ui.confirm_choice("  1. a task  > ", "dnx"), "n")
        self.assertIs(ui.welcome([task], ("Session 3 ended.",), "try again", (label,)), plan)
        # Lists, whatever the caller passed: a front end may keep them.
        self.assertEqual(
            back.seen[1], ("welcome", [task], ["Session 3 ended."], "try again", [label])
        )


class TestPromptParsing(unittest.TestCase):
    def test_known_sets_when_the_prompt_is_bare(self):
        # The debrief prints its legend once and then prompts per task.
        self.assertEqual(
            gui.parse_choices("  3. write the thing (doing)  > ", "dnx"),
            [("d", "Done"), ("n", "Not done"), ("x", "Drop for good")],
        )

    def test_unknown_key_falls_back_to_itself(self):
        self.assertEqual(gui.parse_choices("pick > ", "ab"), [("a", "a"), ("b", "b")])

    def test_question_text_drops_caret(self):
        self.assertEqual(
            gui.question_text(f"{ui.MINUTES_QUESTION}\n> "), ui.MINUTES_QUESTION
        )


class TestWebkitEnv(unittest.TestCase):
    """The environment the webview is started in. It lives in gui.py rather
    than in the launchers so a gate started any other way gets it too."""

    def test_defaults_and_no_sandbox(self):
        self.assertEqual(
            gui.webkit_env({}),
            {
                "GTK_A11Y": "none",
                "GIO_USE_VFS": "local",
                "WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS": "1",
            },
        )

    def test_a_value_set_on_purpose_is_left_alone(self):
        extra = gui.webkit_env({"GTK_A11Y": "atspi", "GIO_USE_VFS": "gvfs"})
        self.assertNotIn("GTK_A11Y", extra)
        self.assertNotIn("GIO_USE_VFS", extra)

    def test_an_empty_value_counts_as_unset(self):
        # A launcher that exports an empty string has said nothing.
        self.assertEqual(gui.webkit_env({"GTK_A11Y": ""})["GTK_A11Y"], "none")

    def test_sandbox_can_be_put_back_for_a_run(self):
        # The diagnostic path: INTENTIONALITY_GATE_SANDBOX=1 restores the
        # sandbox, which is how to make xdg-dbus-proxy fail out loud again.
        self.assertNotIn(
            "WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS",
            gui.webkit_env({"INTENTIONALITY_GATE_SANDBOX": "1"}),
        )

    def test_nothing_else_is_touched(self):
        env = {"PATH": "/usr/bin", "DBUS_SESSION_BUS_ADDRESS": "unix:path=/run/user/1000/bus"}
        self.assertEqual(gui.webkit_env(env).keys() & env.keys(), set())


class TestNoWayOut(unittest.TestCase):
    """The window cannot be closed and Escape is not an abort; SIGINT still
    is. The handlers are exercised on a bare GtkUI with a stand-in Gdk."""

    def setUp(self):
        self.front = gui.GtkUI.__new__(gui.GtkUI)
        self.front.closed = False
        self.front.Gdk = SimpleNamespace(
            KEY_Escape=0xFF1B,
            ModifierType=SimpleNamespace(CONTROL_MASK=1 << 2, ALT_MASK=1 << 3),
            keyval_to_unicode=lambda keyval: 0x1B if keyval == 0xFF1B else keyval,
        )
        self.front.GLib = SimpleNamespace(idle_add=lambda fn: fn())

    def test_close_request_is_refused(self):
        # True stops the close, whether the compositor or GTK asked for it.
        self.assertTrue(self.front._on_close(None))
        self.assertFalse(self.front.closed)

    def test_escape_does_nothing(self):
        self.assertFalse(self.front._on_key(None, self.front.Gdk.KEY_Escape, 0, 0))
        self.assertFalse(self.front.closed)

    def test_choice_keys_still_answer(self):
        pressed = []
        self.front._keys = {"d": lambda: pressed.append("d")}
        self.assertTrue(self.front._on_key(None, ord("d"), 0, 0))
        self.assertEqual(pressed, ["d"])

    def test_sigint_still_aborts(self):
        self.front._abort()
        self.assertTrue(self.front.closed)


class TestHandoffCommand(BackendCase):
    def setUp(self):
        super().setUp()
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.addCleanup(setattr, config, "STORE_PATH", config.STORE_PATH)
        config.STORE_PATH = Path(self._tmp.name) / "store.db"
        self.conn = store.connect()
        self.addCleanup(self.conn.close)
        store.init(self.conn)
        self.addCleanup(setattr, config, "DESKTOP_CMD", config.DESKTOP_CMD)
        config.DESKTOP_CMD = ["fake-desktop"]
        self.launched: list[int] = []
        self.addCleanup(setattr, handoff, "launch_and_wait", handoff.launch_and_wait)
        handoff.launch_and_wait = lambda sid: (self.launched.append(sid), 0)[1]
        self.addCleanup(setattr, ui, "confirm_choice", ui.confirm_choice)
        ui.confirm_choice = lambda prompt, choices: "n"  # debrief: not done

    def test_no_open_session_means_no_desktop(self):
        # The gate was quit under the compositor: nothing to start.
        self.assertEqual(gate_main.handoff_cmd(self.conn), 0)
        self.assertEqual(self.launched, [])
        self.assertIn("not starting the desktop", self.out.getvalue())

    def test_launches_the_committed_session_then_closes_it(self):
        sid = store.commit_draft(self.conn, "", 60, "manual", ["a task"])

        self.assertEqual(gate_main.handoff_cmd(self.conn), 0)

        self.assertEqual(self.launched, [sid])
        self.assertIsNotNone(store.get_session(self.conn, sid)["ended_at"])
        # The debrief said "not done", so the carry ran, like gate() does.
        self.assertEqual([t["title"] for t in store.get_backlog(self.conn)], ["a task"])

    def test_recovered_session_is_not_open(self):
        # ended_at NULL but close_reason set is the no-heartbeat recovery
        # shape; latest_open_session must not hand it to the desktop.
        sid = store.commit_draft(self.conn, "", 60, "manual", ["old"])
        store.mark_recovered(self.conn, sid)
        self.assertEqual(gate_main.handoff_cmd(self.conn), 0)
        self.assertEqual(self.launched, [])

    def test_launch_failure_leaves_the_session_open(self):
        sid = store.commit_draft(self.conn, "", 60, "manual", ["a task"])
        handoff.launch_and_wait = lambda sid: None
        self.assertEqual(gate_main.handoff_cmd(self.conn), 1)
        self.assertIsNone(store.get_session(self.conn, sid)["ended_at"])


if __name__ == "__main__":
    unittest.main()
