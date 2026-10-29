"""The GNOME Shell extension that draws the at-risk timer.

Its drawing only runs inside gnome-shell, so what is checked here is the part
that can run anywhere: the clock arithmetic (under gjs, which ships with
GNOME) and that the extension is laid out the way the shell loads it.
"""

import json
import shutil
import subprocess
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
EXTENSION = REPO / "gnome-extension" / "at-risk-timer@intentionality"


class TestExtension(unittest.TestCase):
    @unittest.skipUnless(shutil.which("gjs"), "gjs not installed")
    def test_clock_arithmetic(self):
        result = subprocess.run(
            ["gjs", "-m", str(REPO / "tests" / "extension_clock_test.js")],
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "ok")

    def test_the_shell_finds_it_under_its_uuid(self):
        # gnome-shell loads ~/.local/share/gnome-shell/extensions/<uuid>/ and
        # refuses one whose metadata names a different uuid.
        metadata = json.loads((EXTENSION / "metadata.json").read_text())
        self.assertEqual(metadata["uuid"], EXTENSION.name)
        for name in ("extension.js", "clock.js", "stylesheet.css"):
            self.assertTrue((EXTENSION / name).is_file(), name)


if __name__ == "__main__":
    unittest.main()
