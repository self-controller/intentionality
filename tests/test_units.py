"""No systemd unit may execute a file under /home directly.

SELinux is enforcing on this machine and labels everything in the repo
user_home_t. systemd runs as init_t, which may not execute user_home_t files,
so a unit whose ExecStart= names bin/resume-gate itself dies with
status=203/EXEC before the script ever runs. Starting it through an
interpreter in /usr/bin (bin_t) transitions to unconfined_service_t, which may
read the script — so the executable has to live outside /home even when the
script does. Run with:  python3 -m unittest discover tests
"""

import re
import tempfile
import unittest
from pathlib import Path

UNITS = Path(__file__).resolve().parent.parent / "systemd"
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


if __name__ == "__main__":
    unittest.main()
