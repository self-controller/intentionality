"""The install surface: the systemd units, the two launchers, and the sway
helper and sway config they share.

No systemd unit may execute a file under /home directly.

SELinux is enforcing on this machine and labels everything in the repo
user_home_t. systemd runs as init_t, which may not execute user_home_t files,
so a unit whose ExecStart= names bin/resume-gate itself dies with
status=203/EXEC before the script ever runs. Starting it through an
interpreter in /usr/bin (bin_t) transitions to unconfined_service_t, which may
read the script — so the executable has to live outside /home even when the
script does. Run with:  python3 -m unittest discover tests
"""

import os
import re
import shlex
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
UNITS = REPO / "systemd"
LAUNCHERS = [REPO / "bin" / "gate-login", REPO / "bin" / "resume-gate"]
# Sourced by both launchers: how sway is started, and what its end means.
HELPER = REPO / "bin" / "graphical-gate.fish"
TEMPLATE = REPO / "bin" / "gate-sway.conf.in"
GATE_LOG = "~/.local/state/intentionality/gate.log"
EXEC_LINE = re.compile(r"^(Exec\w+)=(.*)$")


def executables(unit: Path):
    """(key, executable) for every Exec*= line, systemd's prefixes stripped."""
    for line in unit.read_text().splitlines():
        match = EXEC_LINE.match(line.strip())
        if not match:
            continue
        # + ! !! @ - : | are special-executable prefixes, not part of the path.
        words = match.group(2).lstrip("+-!@:|").split()
        if words:  # a bare "ExecStart=" resets the list; nothing to check
            yield match.group(1), words[0]


class UnitExecCase(unittest.TestCase):
    def test_units_are_found(self):
        # Guards the test itself: a moved directory must not pass vacuously.
        self.assertTrue(list(UNITS.glob("*.service")))

    def test_nothing_executes_from_home(self):
        for unit in sorted(UNITS.glob("*.service")):
            for key, exe in executables(unit):
                with self.subTest(unit=unit.name, key=key):
                    self.assertFalse(exe.startswith("/home/"), f"{key}={exe}")

    def test_resume_gate_has_a_logind_session(self):
        """The graphical gate needs one (no session, no GPU); the PAM stack
        it names must ship with the unit and register that session."""
        unit = (UNITS / "intentionality-resume-gate.service").read_text()
        (pam_name,) = re.findall(r"^PAMName=(\S+)$", unit, re.MULTILINE)
        stack = UNITS / "pam.d" / pam_name
        self.assertTrue(stack.is_file(), f"{stack} missing")
        lines = [
            line.split()
            for line in stack.read_text().splitlines()
            if line.strip() and not line.startswith("#")
        ]
        self.assertIn(["session", "required", "pam_systemd.so"], lines)
        # systemd never runs the auth stack for PAMName=; a line there would
        # be dead at best and, if it were ever run, a password prompt on VT9.
        self.assertFalse([l for l in lines if l[0] == "auth"], "no auth lines")

    def test_launchers_keep_what_the_compositor_prints(self):
        """Both of them, and in the same place. The resume unit's stdout is
        /dev/tty9: a graphical gate that fails there leaves no other trace,
        which is exactly how five aborted wake gates went unnoticed."""
        for launcher in LAUNCHERS:
            with self.subTest(launcher=launcher.name):
                text = launcher.read_text()
                self.assertIn(f"set -l log {GATE_LOG}", text)
                self.assertIn("source $repo/bin/graphical-gate.fish", text)
                self.assertRegex(text, r"graphical_gate \$log \$py -m gate")
        helper = HELPER.read_text()
        self.assertRegex(helper, r"(?m)^\s*sway -c \$conf >> \$log 2>&1$")

    def test_launchers_leave_the_gate_environment_to_gui_py(self):
        """One contract, in gate/gui.py, so a gate started any other way --
        by hand from another console, say -- is started the same way."""
        owned = ("GTK_A11Y", "GIO_USE_VFS", "WEBKIT_DISABLE_SANDBOX")
        for launcher in [*LAUNCHERS, HELPER, TEMPLATE]:
            text = launcher.read_text()
            for name in owned:
                with self.subTest(launcher=launcher.name, var=name):
                    self.assertNotRegex(text, rf"(?m)^\s*set -[a-z]*x[a-z]* {name}")
                    self.assertNotIn(f"{name}=", text)

    def test_parser_sees_through_prefixes(self):
        # Without this, a "-/home/…" line would read as "-/home/…" and slip
        # past the startswith check above.
        with tempfile.TemporaryDirectory() as tmp:
            unit = Path(tmp) / "x.service"
            unit.write_text(
                "[Service]\n"
                "# ExecStart=/home/commented/out\n"
                "ExecStartPre=+/bin/sh -c 'true'\n"
                "ExecStart=-/home/someone/script arg\n"
                "ExecStart=\n"
            )
            self.assertEqual(
                [("ExecStartPre", "/bin/sh"), ("ExecStart", "/home/someone/script")],
                list(executables(unit)),
            )


# Stand-ins for the programs bin/graphical-gate.fish runs, so its choices can
# be tested without a compositor or a GPU. The fake sway runs its config's
# exec line the way sway does (sh -c, as its child), so the real template's
# status-file dance is what gets exercised.
FAKE_SWAY = """#!/bin/sh
echo "sway $*" >> "$FAKE_CALLS"
if [ "$1" = -C ]; then exit "${FAKE_SWAY_VALID:-0}"; fi
[ -n "$FAKE_SWAY_BROKEN" ] && exit 1
line=$(sed -n 's/^exec //p' "$2")
sh -c "$line"
"""
FAKE_GATE = """#!/bin/sh
env | grep -E '^(INTENTIONALITY_|PYTHONUNBUFFERED)' | sort > "$FAKE_GATE_ENV"
exit "${FAKE_GATE_STATUS:-0}"
"""
# gsettings as GNOME's opposite: proves the values are read, not assumed.
FAKE_GSETTINGS = """#!/bin/sh
case "$2 $3" in
  *touchpad\\ natural-scroll) echo false ;;
  *touchpad\\ tap-to-click) echo false ;;
  *mouse\\ natural-scroll) echo true ;;
  *) exit 1 ;;
esac
"""
BROKEN_GSETTINGS = "#!/bin/sh\nexit 1\n"
# By path: the tests run it with a PATH holding only the stand-ins.
FISH = shutil.which("fish")
# What the helper and the stand-ins run besides fish's builtins.
TOOLS = ("sh", "cat", "rm", "mkdir", "date", "timeout", "env", "grep", "sort", "sed")


@unittest.skipUnless(FISH, "fish not installed")
class GraphicalGateCase(unittest.TestCase):
    """bin/graphical-gate.fish and the sway config it fills in."""

    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.tmp = Path(tmp.name)
        self.bin = self.tmp / "bin"
        self.bin.mkdir()
        self.run_dir = self.tmp / "run"
        self.run_dir.mkdir(mode=0o700)
        self.calls = self.tmp / "calls"
        self.gate_env = self.tmp / "gate.env"
        self.gate = self.install("gate", FAKE_GATE)
        # Only the stand-ins and the plain tools they and the helper use:
        # with /usr/bin on PATH, an installed sway would be started.
        self.tools = self.tmp / "tools"
        self.tools.mkdir()
        for tool in TOOLS:
            found = shutil.which(tool, path="/usr/bin:/bin")
            if found:
                (self.tools / tool).symlink_to(found)

    def install(self, name: str, script: str) -> Path:
        path = self.bin / name
        path.write_text(script)
        path.chmod(0o755)
        return path

    def fish(self, script: str, path: str | None = None, **env: str) -> subprocess.CompletedProcess:
        full_env = {
            "HOME": str(self.tmp),
            "PATH": path or f"{self.bin}:{self.tools}",
            "XDG_RUNTIME_DIR": str(self.run_dir),
            "FAKE_CALLS": str(self.calls),
            "FAKE_GATE_ENV": str(self.gate_env),
            **env,
        }
        return subprocess.run(
            [FISH, "--no-config", "-c", f"source {HELPER}; {script}"],
            env=full_env,
            capture_output=True,
            text=True,
            timeout=30,
        )

    def config(self, *command: str, **env: str) -> subprocess.CompletedProcess:
        status = self.run_dir / "intentionality" / "sway-1.status"
        args = " ".join([str(status), *(command or ("/usr/bin/python3", "-m", "gate", "resume"))])
        return self.fish(f"graphical_gate_sway_config {args}", **env)

    def gate_run(self, **env: str) -> tuple[int, list[str]]:
        """graphical_gate's status, and each sway it started ("sway -C" for the check)."""
        log = self.tmp / "gate.log"
        result = self.fish(f"graphical_gate {log} {self.gate}; echo $status", **env)
        self.assertEqual("", result.stderr)
        calls = self.calls.read_text().splitlines() if self.calls.exists() else []
        return int(result.stdout.strip().splitlines()[-1]), [c.split()[0] + (" -C" if " -C " in c else "") for c in calls]

    # ------------------------------------------------------------ the config

    def lines(self, text: str) -> list[str]:
        return [l.strip() for l in text.splitlines() if l.strip() and not l.strip().startswith("#")]

    def test_template_has_no_bindings(self):
        """sway -c reads this file and nothing else, and sway has no built-in
        bindings: so none at all, and no way to exit but answering."""
        for line in self.lines(TEMPLATE.read_text()):
            self.assertFalse(line.startswith("bind"), line)
            if not line.startswith("exec "):  # the gate's own line ends sway
                self.assertNotIn("exit", line)

    def test_filled_config_is_a_kiosk(self):
        self.install("gsettings", BROKEN_GSETTINGS)
        result = self.config()
        self.assertEqual(0, result.returncode, result.stderr)
        lines = self.lines(result.stdout)
        for expected in (
            "xwayland disable",
            "swaybg_command -",
            "swaynag_command -",
            "default_border none",
            "for_window [all] fullscreen enable",
            "output * scale 2",
            "seat * xcursor_theme Adwaita 24",
            # Ctrl+Alt+Fn stay F-keys: no console switch out of the gate.
            "input type:keyboard xkb_options srvrkeys:none",
        ):
            self.assertIn(expected, lines)
        self.assertNotRegex(result.stdout, r"@[A-Z_]+@")
        self.assertFalse([l for l in lines if l.startswith("bind")])

    def test_exec_line_is_last_and_outside_every_block(self):
        """An unclosed block would swallow it: sway up, no gate, no way out."""
        self.install("gsettings", BROKEN_GSETTINGS)
        lines = self.lines(self.config().stdout)
        depth = 0
        for line in lines:
            if line.startswith("exec"):
                self.assertEqual(0, depth, "exec inside a block")
            depth += line.count("{") - line.count("}")
        self.assertEqual(0, depth)
        self.assertEqual(1, len([l for l in lines if l.startswith("exec")]))
        self.assertTrue(lines[-1].startswith("exec sh -c '"))

    def test_exec_line_runs_the_gate_and_keeps_its_status(self):
        self.install("gsettings", BROKEN_GSETTINGS)
        status = self.run_dir / "intentionality" / "sway-1.status"
        (exec_line,) = [l for l in self.lines(self.config().stdout) if l.startswith("exec")]
        self.assertIn(f"cd {REPO} && ", exec_line)
        for env in (
            "INTENTIONALITY_GATE_UI=gtk ",
            # Blank: the desktop is started after sway exits, never inside it.
            "INTENTIONALITY_DESKTOP_CMD= ",
            "PYTHONUNBUFFERED=1 ",
        ):
            self.assertIn(env, exec_line)
        self.assertIn("/usr/bin/python3 -m gate resume; ", exec_line)
        self.assertIn(f"echo $? > {status}; ", exec_line)

    def test_pointer_settings_come_from_gnome(self):
        self.install("gsettings", FAKE_GSETTINGS)
        text = self.config().stdout
        self.assertRegex(text, r"type:touchpad \{\s*natural_scroll disabled\s*tap disabled\s*\}")
        self.assertIn("input type:pointer natural_scroll enabled", text)

    def test_pointer_settings_fall_back_to_gnome_defaults(self):
        self.install("gsettings", BROKEN_GSETTINGS)
        text = self.config().stdout
        self.assertRegex(text, r"type:touchpad \{\s*natural_scroll enabled\s*tap enabled\s*\}")
        self.assertIn("input type:pointer natural_scroll disabled", text)

    def test_values_that_need_quoting_are_refused(self):
        """They would land inside sh -c '...'. Refused, the launcher runs the
        text gate instead; nothing is half-written."""
        self.install("gsettings", BROKEN_GSETTINGS)
        for bad in ("/tmp/a b", "/tmp/it's", "/tmp/$HOME", "/tmp/a;reboot"):
            with self.subTest(bad=bad):
                result = self.fish(f"graphical_gate_sway_config {shlex.quote(bad)} /usr/bin/python3 -m gate")
                self.assertNotEqual(0, result.returncode)
                self.assertEqual("", result.stdout)

    @unittest.skipUnless(shutil.which("sway"), "sway not installed")
    def test_sway_accepts_the_config(self):
        self.install("gsettings", BROKEN_GSETTINGS)
        conf = self.tmp / "sway.conf"
        conf.write_text(self.config().stdout)
        result = subprocess.run(
            ["sway", "-C", "-c", str(conf)],
            env={**os.environ, "WLR_BACKENDS": "headless", "WLR_RENDERER": "pixman",
                 "WLR_LIBINPUT_NO_DEVICES": "1", "XDG_RUNTIME_DIR": str(self.run_dir)},
            capture_output=True, text=True, timeout=30,
        )
        self.assertEqual(0, result.returncode, result.stderr)

    # ------------------------------------------------------ whether to try

    def test_not_possible_without_a_runtime_dir(self):
        self.install("sway", "#!/bin/sh\nexit 0\n")
        result = self.fish("set -e XDG_RUNTIME_DIR; graphical_gate_possible", path=str(self.bin))
        self.assertNotEqual(0, result.returncode)
        self.assertEqual(0, self.fish("graphical_gate_possible", path=str(self.bin)).returncode)

    def test_not_possible_without_sway(self):
        self.assertNotEqual(0, self.fish("graphical_gate_possible", path=str(self.bin)).returncode)

    # -------------------------------------------------------- the whole run

    def sway(self):
        self.install("sway", FAKE_SWAY)
        self.install("gsettings", BROKEN_GSETTINGS)

    def log(self) -> str:
        return (self.tmp / "gate.log").read_text()

    def test_a_finished_gate_is_the_answer(self):
        """Status 0 is what sends the login launcher on to `gate handoff`."""
        self.sway()
        self.assertEqual((0, ["sway -C", "sway"]), self.gate_run())
        env = self.gate_env.read_text().splitlines()
        self.assertIn("INTENTIONALITY_DESKTOP_CMD=", env)
        self.assertIn("INTENTIONALITY_GATE_UI=gtk", env)
        self.assertIn("PYTHONUNBUFFERED=1", env)

    def test_a_failed_gate_is_final(self):
        """Its status goes back as it is, and the launchers run the text gate
        next; sway is not started a second time."""
        self.sway()
        self.assertEqual((3, ["sway -C", "sway"]), self.gate_run(FAKE_GATE_STATUS="3"))

    def test_sway_that_never_ran_the_gate_means_the_text_gate(self):
        """No status file: sway ended before the gate finished."""
        self.sway()
        status, calls = self.gate_run(FAKE_SWAY_BROKEN="1")
        self.assertNotEqual(0, status)
        self.assertEqual(["sway -C", "sway"], calls)
        self.assertFalse(self.gate_env.exists(), "the gate never ran")
        self.assertIn("gate: sway exited 1 before the gate finished.", self.log())

    def test_a_config_sway_rejects_is_never_started(self):
        self.sway()
        status, calls = self.gate_run(FAKE_SWAY_VALID="1")
        self.assertNotEqual(0, status)
        self.assertEqual(["sway -C"], calls)
        self.assertIn("gate: no usable sway config; not starting sway.", self.log())

    def test_without_sway_nothing_is_started(self):
        self.install("gsettings", BROKEN_GSETTINGS)
        status, calls = self.gate_run()
        self.assertNotEqual(0, status)
        self.assertEqual([], calls)
        self.assertIn("gate: sway is not installed.", self.log())

    def test_files_of_dead_launchers_are_swept_and_live_ones_kept(self):
        """A gate that outlives a crashed sway writes its status after its
        launcher has moved on; another launcher may be running right now."""
        self.sway()
        state = self.run_dir / "intentionality"
        state.mkdir()
        dead = self._dead_pid()
        (state / f"sway-{dead}.status").write_text("0\n")
        (state / f"sway-{dead}.conf").write_text("")
        live = state / f"sway-{os.getpid()}.status"
        live.write_text("0\n")
        self.gate_run()
        self.assertEqual([live], list(state.iterdir()))

    def _dead_pid(self) -> int:
        proc = subprocess.Popen(["true"])
        proc.wait()
        return proc.pid

    def test_nothing_is_left_in_the_runtime_dir(self):
        """The status file above all: one left behind could be read as a later
        run's answer."""
        self.sway()
        for env in ({}, {"FAKE_GATE_STATUS": "3"}, {"FAKE_SWAY_BROKEN": "1"}, {"FAKE_SWAY_VALID": "1"}):
            with self.subTest(**env):
                self.gate_run(**env)
                self.assertEqual([], list((self.run_dir / "intentionality").iterdir()))


if __name__ == "__main__":
    unittest.main()
