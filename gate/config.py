"""Paths and constants. No logic lives here."""

import os
import shlex
from pathlib import Path

# INTENTIONALITY_STORE overrides the store location (used by tests / dry runs).
STORE_PATH = Path(
    os.environ.get(
        "INTENTIONALITY_STORE",
        Path.home() / ".local/share/intentionality/store.db",
    )
)

# Touch this file to disable the gate: the tty1 login block checks it, and so
# does the resume gate. An escape hatch you can reach without a rebuild.
SKIP_PATH = Path.home() / ".config/intentionality/skip"

# Command the gate launches as a child and waits on after COMMIT. Empty =
# no handoff: the session stays open and `python -m gate close` ends it later
# (dev mode, running inside an existing desktop). At a real console this is
# the desktop session, e.g. "dbus-run-session -- gnome-session".
DESKTOP_CMD = shlex.split(os.environ.get("INTENTIONALITY_DESKTOP_CMD", ""))
