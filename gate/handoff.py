"""Launch the desktop session as a child and wait for it.

Waiting — not exec'ing — is the point: the gate regains control when the
desktop exits, which is what makes the closing write and the debrief possible.
The handoff carries only the session ID, via the environment; everything else
is looked up from the store.
"""

import os
import subprocess

from . import config


def launch_and_wait(session_id: int) -> int | None:
    """Run the configured desktop command; block until it exits.

    Returns its exit code, or None if it could not be launched at all.
    """
    env = dict(os.environ, INTENTIONALITY_SESSION_ID=str(session_id))
    try:
        proc = subprocess.Popen(config.DESKTOP_CMD, env=env)
    except OSError as exc:
        print(f"could not launch desktop ({exc}) — session left open.")
        return None

    # A waiting gate offers no prompt and no escape hatch: Ctrl-C at the
    # console must not kill the supervisor while the desktop is running.
    while True:
        try:
            return proc.wait()
        except KeyboardInterrupt:
            continue
