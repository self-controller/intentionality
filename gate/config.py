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

# Command the gate launches as a child and waits on after COMMIT. Empty =
# no handoff: the session stays open and `python -m gate close` ends it later
# (dev mode, running inside an existing desktop). At a real console this is
# the desktop session, e.g. "dbus-run-session -- gnome-session".
DESKTOP_CMD = shlex.split(os.environ.get("INTENTIONALITY_DESKTOP_CMD", ""))
