"""Jev filing a typed task under a label: the questions, the policy, the one
HTTP call (against a local stub), the store's examples, and the app's twin
in app/src-tauri/src/autolabel.rs worded the same.

Run with:  python3 -m unittest discover tests
"""

import contextlib
import http.server
import inspect
import io
import json
import os
import re
import sqlite3
import tempfile
import queue
import threading
import time
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

from gate import autolabel, config, gui, store, ui, webstate
from gate.autolabel import Candidate

ROOT = Path(__file__).resolve().parent.parent
AUTOLABEL_RS = ROOT / "app/src-tauri/src/autolabel.rs"
JEV_RS = ROOT / "app/src-tauri/src/jev.rs"

CANDIDATES = [Candidate("School", ("HW 6", "Quiz")), Candidate("Home")]


def answers(choice="School", picked=0.9, nouls=(0.95, 0.1)):
    other = "Home" if choice == "School" else "School"
    return {
        "label": {
            "type": "choice", "choice": choice, "confidence": 0.5,
            "probabilities": {choice: picked, other: 1 - picked},
        },
        "l0": {"type": "noul", "noul": nouls[0]},
        "l1": {"type": "noul", "noul": nouls[1]},
    }


class TestQuestions(unittest.TestCase):
    def test_each_label_is_offered_with_what_is_filed_there(self):
        q = autolabel.questions(CANDIDATES)
        self.assertEqual(set(q), {"label", "l0", "l1"})
        self.assertEqual(q["label"]["type"], "choice")
        self.assertEqual(
            q["label"]["criteria"],
            {"School": {"tasks_already_filed_here": ["HW 6", "Quiz"]}, "Home": None},
        )
        self.assertEqual(q["l0"]["type"], "noul")
        self.assertEqual(
            q["l0"]["instructions"]["label"],
            {"name": "School", "tasks_already_filed_here": ["HW 6", "Quiz"]},
        )
        self.assertEqual(q["l1"]["instructions"]["label"], {"name": "Home"})
        self.assertEqual(
            q["l1"]["criteria"],
            {"true": autolabel.CRITERIA_TRUE, "false": autolabel.CRITERIA_FALSE},
        )
        # The whole request must be JSON as it stands.
        json.dumps(q)

    def test_blank_notes_are_left_out(self):
        self.assertEqual(autolabel.state(" HW 7 ", "  "), {"task": {"title": "HW 7"}})
        self.assertEqual(
            autolabel.state("HW 7", " ch. 3 "), {"task": {"title": "HW 7", "notes": "ch. 3"}}
        )


class TestDecide(unittest.TestCase):
    def test_a_label_goes_on_only_when_picked_and_said_to_belong(self):
        self.assertEqual(autolabel.decide(answers(), CANDIDATES), "School")
        self.assertEqual(autolabel.decide(answers(picked=0.7, nouls=(0.7, 0)), CANDIDATES), "School")
        # Split between two labels: neither is picked clearly.
        self.assertIsNone(autolabel.decide(answers(picked=0.6, nouls=(0.95, 0.9)), CANDIDATES))
        # Picked, but doubted to belong anywhere.
        self.assertIsNone(autolabel.decide(answers("Home", 0.9, (0.1, 0.5)), CANDIDATES))
        # The noul read is the picked label's own.
        self.assertIsNone(autolabel.decide(answers("Home", 0.9, (0.95, 0.2)), CANDIDATES))

    def test_a_malformed_answer_is_no_label(self):
        bad = answers()
        bad["label"]["choice"] = "Gone"
        self.assertIsNone(autolabel.decide(bad, CANDIDATES))
        self.assertIsNone(autolabel.decide({}, CANDIDATES))
        bad = answers()
        bad["l0"] = {"type": "noul", "noul": "high"}
        self.assertIsNone(autolabel.decide(bad, CANDIDATES))


class Stub(http.server.BaseHTTPRequestHandler):
    """A TypeSafe that answers from a script: (status, body) per request."""

    script: list = []
    seen: list = []

    def do_POST(self):
        length = int(self.headers["Content-Length"])
        Stub.seen.append((self.headers["Authorization"], json.loads(self.rfile.read(length))))
        status, body = Stub.script.pop(0)
        payload = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, *args):
        pass


class TestAsk(unittest.TestCase):
    def setUp(self):
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Stub)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        home = tempfile.TemporaryDirectory()
        self.addCleanup(home.cleanup)
        self.home = Path(home.name)
        env = mock.patch.dict(os.environ, {
            "HOME": home.name,
            autolabel.API_URL_ENV: f"http://127.0.0.1:{server.server_port}/v1/systemone",
            autolabel.KEY_ENV: "env-key",
        })
        env.start()
        self.addCleanup(env.stop)
        Stub.script, Stub.seen = [], []
        self.log = io.StringIO()
        quiet = contextlib.redirect_stderr(self.log)
        quiet.__enter__()
        self.addCleanup(quiet.__exit__, None, None, None)

    def test_a_sure_answer_files_the_task(self):
        Stub.script = [(200, {"model": "jev-1.14.0", "answers": answers()})]
        self.assertEqual(autolabel.ask("HW 7", "ch. 3", CANDIDATES), "School")
        ((auth, body),) = Stub.seen
        self.assertEqual(auth, "Bearer env-key")
        self.assertEqual(body["model"], autolabel.MODEL)
        self.assertEqual(body["state"], {"task": {"title": "HW 7", "notes": "ch. 3"}})
        self.assertEqual(body["questions"], autolabel.questions(CANDIDATES))
        self.assertIn("'HW 7' filed under 'School'", self.log.getvalue())

    def test_the_key_file_comes_before_the_environment(self):
        (self.home / ".config/intentionality").mkdir(parents=True)
        (self.home / autolabel.KEY_FILE).write_text("file-key\n")
        Stub.script = [(200, {"answers": answers()})]
        autolabel.ask("HW 7", "", CANDIDATES)
        self.assertEqual(Stub.seen[0][0], "Bearer file-key")

    def test_a_doubtful_answer_files_nothing(self):
        Stub.script = [(200, {"answers": answers(picked=0.5)})]
        self.assertIsNone(autolabel.ask("HW 7", "", CANDIDATES))
        self.assertIn("left unlabelled", self.log.getvalue())

    def test_a_rate_limit_is_retried_once(self):
        Stub.script = [(429, {}), (200, {"answers": answers()})]
        self.assertEqual(autolabel.ask("HW 7", "", CANDIDATES), "School")
        self.assertEqual(len(Stub.seen), 2)

    def test_failures_are_no_label_never_an_exception(self):
        Stub.script = [(500, {"detail": "boom"}), (200, {"no": "answers"})]
        self.assertIsNone(autolabel.ask("HW 7", "", CANDIDATES))
        self.assertIsNone(autolabel.ask("HW 7", "", CANDIDATES))
        with mock.patch.dict(os.environ, {autolabel.API_URL_ENV: "http://127.0.0.1:9/"}):
            self.assertIsNone(autolabel.ask("HW 7", "", CANDIDATES, timeout=1))
        self.assertIn("TypeSafe 500", self.log.getvalue())

    def test_nothing_is_sent_without_a_key_labels_or_a_title(self):
        with mock.patch.dict(os.environ, {autolabel.KEY_ENV: ""}):
            self.assertIsNone(autolabel.ask("HW 7", "", CANDIDATES))
        self.assertIsNone(autolabel.ask("HW 7", "", []))
        self.assertIsNone(autolabel.ask("  ", "", CANDIDATES))
        self.assertEqual(Stub.seen, [])


class TestLabelExamples(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        saved = config.STORE_PATH
        config.STORE_PATH = Path(tmp.name) / "store.db"
        self.addCleanup(setattr, config, "STORE_PATH", saved)
        self.conn = store.connect()
        self.addCleanup(self.conn.close)
        store.init(self.conn)

    def label(self, title, *names):
        with self.conn:
            cur = self.conn.execute(
                "INSERT INTO task (title, position, source, created_at) VALUES (?, 1, 'gate', ?)",
                (title, store.now()),
            )
            for name in names:
                self.conn.execute("INSERT OR IGNORE INTO label (name, color) VALUES (?, '#123456')", (name,))
                self.conn.execute(
                    "INSERT INTO task_label SELECT ?, id FROM label WHERE name = ?",
                    (cur.lastrowid, name),
                )

    def test_newest_first_each_wording_once_and_capped(self):
        self.label("HW 5", "School")
        self.label("hw 5 ", "school")
        self.label("HW 6", "School", "Urgent")
        self.label("Quiz", "School")
        with self.conn:
            self.conn.execute("INSERT INTO label (name, color) VALUES ('Home', '#123456')")
        self.assertEqual(
            store.label_examples(self.conn),
            {"School": ("Quiz", "HW 6", "hw 5"), "Urgent": ("HW 6",)},
        )
        self.assertEqual(store.label_examples(self.conn, per_label=2)["School"], ("Quiz", "HW 6"))


class FakeLoop:
    """GLib's main context, as far as the graphical gate's sorting uses it:
    idle_add from any thread, a timeout, and one blocking iteration."""

    def __init__(self):
        self.queue = queue.Queue()
        self.timers = []
        self.Source = SimpleNamespace(remove=lambda tid: None)
        self.MainContext = SimpleNamespace(default=lambda: SimpleNamespace(iteration=self.iteration))

    def idle_add(self, fn, *args):
        self.queue.put((fn, args))

    def timeout_add(self, ms, fn):
        self.timers.append((time.monotonic() + ms / 1000, fn))

    def iteration(self, _block):
        try:
            fn, args = self.queue.get(timeout=0.01)
        except queue.Empty:
            due = [t for t in self.timers if t[0] <= time.monotonic()]
            for t in due:
                self.timers.remove(t)
                t[1]()
            return
        fn(*args)


class TestGraphicalGate(unittest.TestCase):
    """GtkUI asks on a thread and applies the answer on the main context."""

    def setUp(self):
        self.front = gui.GtkUI.__new__(gui.GtkUI)
        self.front.GLib = FakeLoop()
        self.front.closed = False
        self.front._fatal = None
        self.pushed = []
        self.front._push = self.pushed.append
        self.screen = webstate.WelcomeScreen(
            [], [], "", [ui.Label("school", "#7aa2f7", ("HW 6",))]
        )
        self.screen.handle({"op": "add", "title": "HW 7"})

    def test_an_answer_lands_on_the_screen(self):
        asked = []

        def ask(title, notes, candidates):
            asked.append((title, candidates, threading.current_thread()))
            return "school"

        out = []
        with mock.patch.object(autolabel, "ask", ask):
            self.front._sort(self.screen, out)
            self.front._settle(out)
        self.assertEqual(out, [])
        self.assertEqual(self.screen.rows.rows[0].details.labels, ("school",))
        self.assertEqual(self.pushed[-1]["rows"][0]["labels"], ["school"])
        ((title, candidates, thread),) = asked
        self.assertEqual((title, candidates), ("HW 7", [Candidate("school", ("HW 6",))]))
        self.assertIsNot(thread, threading.main_thread())

    def test_start_waits_no_longer_than_sort_wait(self):
        release = threading.Event()
        self.addCleanup(release.set)

        def ask(title, notes, candidates):
            release.wait(5)
            return "school"

        out = []
        with mock.patch.object(autolabel, "ask", ask), mock.patch.object(gui, "SORT_WAIT", 0.05):
            started = time.monotonic()
            self.front._sort(self.screen, out)
            self.front._settle(out)
        self.assertLess(time.monotonic() - started, 2)
        self.assertEqual(len(out), 1)
        self.assertEqual(self.screen.rows.rows[0].details.labels, ())


def rust_str(source: str, name: str) -> str:
    """A `const NAME: &str = "...";` from Rust source, with Rust's own rule
    for a backslash at a line's end: the newline and the next line's leading
    whitespace go."""
    raw = re.search(rf'const {name}: &str = "(.*?)";', source, re.S)
    if raw is None:
        raise AssertionError(f"{name} not found")
    return re.sub(r"\\\n\s*", "", raw[1])


class TestTwinOfTheApp(unittest.TestCase):
    """The board's cards are judged by autolabel.rs. Same words, same bar."""

    def test_the_questions_are_worded_alike(self):
        rs = AUTOLABEL_RS.read_text()
        for name in ("CHOICE_QUESTION", "NOUL_QUESTION", "CRITERIA_TRUE", "CRITERIA_FALSE"):
            self.assertEqual(rust_str(rs, name), getattr(autolabel, name), name)

    def test_the_bar_examples_and_model_match(self):
        rs = AUTOLABEL_RS.read_text()
        self.assertEqual(float(re.search(r"pub const MIN: f64 = ([\d.]+);", rs)[1]), autolabel.MIN)
        per_label = inspect.signature(store.label_examples).parameters["per_label"].default
        self.assertEqual(int(re.search(r"pub const EXAMPLES: usize = (\d+);", rs)[1]), per_label)
        jev = JEV_RS.read_text()
        self.assertEqual(rust_str(jev, "MODEL"), autolabel.MODEL)
        self.assertEqual(rust_str(jev, "API_URL"), autolabel.API_URL)
        self.assertEqual(rust_str(jev, "API_URL_ENV"), autolabel.API_URL_ENV)
        self.assertEqual(rust_str(jev, "KEY_FILE"), autolabel.KEY_FILE)


if __name__ == "__main__":
    unittest.main()
