"""One gate on screen at a time.

The login gate and the resume gate are separate processes on separate VTs,
and until this nothing kept both from being up at once. Close the lid on the
login gate's welcome screen and waking fired the resume gate on VT9 — the
login gate's recovery had already closed the old session and it had not yet
committed a new one, so "no open session" read as "fire". You answered VT9,
were switched back to the login gate's stale screen, and answered it again:
two sessions, the tasks split between them, and the desktop started for the
second one only.

So every gate holds this lock while its welcome screen is up, and the resume
check (resume.should_gate) stands down while anyone holds it.

An flock, not a marker in the store: the kernel drops it when the holder
exits by any route — SIGKILL, a crash, logind tearing the session down — so it
can never go stale and silence the wake gate for good. It belongs to the open
file description, not the process, so a second open() in the same process
conflicts too (which is what makes it testable in-process), and O_CLOEXEC
keeps it out of WebKit's children and out of the desktop.
"""

import fcntl
import os
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path

from . import config


def path() -> Path:
    # Beside the store and read at call time, so a rehearsal on
    # INTENTIONALITY_STORE — and every test that points STORE_PATH at a temp
    # dir — gets a lock of its own instead of the real one.
    return config.STORE_PATH.with_name("gate.lock")


@contextmanager
def hold() -> Iterator[bool]:
    """Hold the lock for the length of the with-block, if it can be had;
    yields whether it was.

    Never waits and never raises: the login path must not be the thing a held
    or unwritable lock can shut you out of, so a gate that cannot take it
    runs anyway, exactly as it did before there was a lock.
    """
    fd = None
    try:
        fd = os.open(path(), os.O_RDWR | os.O_CREAT | os.O_CLOEXEC, 0o600)
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        if fd is not None:
            os.close(fd)
            fd = None
    try:
        yield fd is not None
    finally:
        if fd is not None:
            os.close(fd)  # closing the only descriptor drops the flock


def held() -> bool:
    """Whether some gate has its welcome screen up right now.

    False whenever it cannot tell (no lock file yet, an unreadable one), so a
    broken lock leaves the resume gate behaving as it did without one.
    """
    try:
        fd = os.open(path(), os.O_RDONLY | os.O_CLOEXEC)
    except OSError:
        return False
    try:
        fcntl.flock(fd, fcntl.LOCK_SH | fcntl.LOCK_NB)
    except BlockingIOError:
        return True
    except OSError:
        return False
    finally:
        os.close(fd)
    return False
