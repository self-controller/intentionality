import os
import sys
from datetime import datetime

from . import config, debrief, flow, handoff, resume, store
from .ui import GateAborted


def select_ui() -> None:
    """INTENTIONALITY_GATE_UI=gtk asks for the graphical front end. It is
    only ever a request: no GTK, no display, no compositor -- the terminal
    answers instead, with one line on stderr saying why. A GUI that could
    fail closed would be a GUI that could lock the login path."""
    if os.environ.get("INTENTIONALITY_GATE_UI") != "gtk":
        return
    try:
        from . import gui

        gui.install()
    except Exception as exc:  # ImportError, ValueError from gi, RuntimeError
        print(f"gate: no graphical front end ({exc}); using the terminal.", file=sys.stderr)


def recover_open_sessions(conn) -> list[str]:
    """Close whatever the last session left behind and carry its unfinished
    tasks into the backlog. Returns what it printed, for the welcome screen.

    It asks nothing: the carried tasks are the first thing the welcome screen
    shows, with Done and Delete on each, so resolving them happens there.
    Shared by every entry point that opens a new session, so the login gate
    and the resume gate can never drift apart on how a session is salvaged.
    """
    notes = []
    for row in store.get_open_sessions(conn):
        store.mark_recovered(conn, row["id"])
        # mark_recovered has run, so this session will never be offered
        # again: nothing between here and the carry may skip it.
        try:
            ended = store.get_session(conn, row["id"])["ended_at"]
            if ended:
                notes.append(f"Session {row['id']} ended around {local_time(ended)}.")
            else:
                notes.append(
                    f"Session {row['id']} was never closed — no heartbeat, end time unknown."
                )
            print(notes[-1])
        finally:
            carried = store.carry_unfinished(conn, row["id"])
        if carried:
            tasks = "task" if carried == 1 else "tasks"
            notes.append(f"{carried} unfinished {tasks} carried over.")
            print(notes[-1])
    return notes


def local_time(stamp: str) -> str:
    """'7:37 PM, Sep 16' for a stored UTC stamp."""
    when = datetime.fromisoformat(stamp).astimezone()
    return f"{when.hour % 12 or 12}:{when:%M %p}, {when:%b} {when.day}"


def gate(conn) -> int:
    try:
        session_id = flow.run(conn, recover_open_sessions(conn))
    except GateAborted:
        print("\nNothing saved.")
        return 0

    if not config.DESKTOP_CMD:
        print("No desktop command configured — run `gate close` to end the session.")
        return 0

    # HANDOFF: the desktop runs as a child; the gate waits. In a real console
    # login this wait rarely returns — logind tears the whole scope down when
    # GNOME exits — so the close below is best-effort. The desktop app's
    # heartbeat plus the recovery block above are the real close mechanism.
    if handoff.launch_and_wait(session_id) is None:
        return 1
    _close_debrief_carry(conn, session_id)
    return 0


def resume_cmd(conn) -> int:
    """The gate again, because the machine just woke up.

    Same recovery sweep and same elicitation as a login gate — only the
    handoff is missing. The desktop is already running on another VT; this
    console exists for the length of the conversation and no longer.
    """
    try:
        flow.run(conn, recover_open_sessions(conn))
    except GateAborted:
        print("\nNothing saved — no session is open.")
        return 0
    print("\nBack to your desktop.")
    return 0


def handoff_cmd(conn) -> int:
    """The second half of `gate`, for a launcher that ran the first half
    under a compositor.

    A kiosk compositor holds the GPU for as long as its client lives, so the
    desktop cannot be that client's child the way gate() makes it: the
    conversation runs under cage, cage exits, and then this launches the
    desktop for whatever the conversation committed. "Whatever" is exact:
    the recovery sweep just closed every other open session, so an open one
    now is the one just stated, and none means the gate was interrupted.
    """
    row = store.latest_open_session(conn)
    if row is None:
        print("No open session — not starting the desktop.")
        return 0
    if not config.DESKTOP_CMD:
        print("No desktop command configured — run `gate close` to end the session.")
        return 0
    if handoff.launch_and_wait(row["id"]) is None:
        return 1
    _close_debrief_carry(conn, row["id"])
    return 0


def resume_needed_cmd(conn) -> int:
    # The exit status is the whole interface: this runs as the resume unit's
    # ExecCondition, where 0 fires the gate and 1 skips it cleanly — a skip,
    # not a failure, so nothing flickers and nothing is logged as broken.
    return 0 if resume.should_gate(conn) else 1


def close_cmd(conn) -> int:
    row = store.latest_open_session(conn)
    if row is None:
        print("No open session.")
        return 0
    _close_debrief_carry(conn, row["id"])
    return 0


def migrate_cmd(conn) -> int:
    # store.init() already migrated on connect; this subcommand exists so the
    # desktop app can print one actionable instruction when the schema is old.
    print(f"store is at schema v{store.get_setting(conn, 'schema_version')}.")
    return 0


def _close_debrief_carry(conn, session_id: int) -> None:
    # Close first (a skipped debrief still leaves a complete record); carry
    # last, so what the debrief just resolved doesn't land in the backlog.
    store.close_session(conn, session_id)
    try:
        debrief.run(conn, session_id)
    except GateAborted:
        print("\nDebrief skipped.")
    finally:
        carried = store.carry_unfinished(conn, session_id)
        if carried:
            print(f"{carried} unfinished task(s) moved to the backlog.")


def main(argv: list[str]) -> int:
    conn = store.connect()
    store.init(conn)
    if argv[1:] == ["migrate"]:
        return migrate_cmd(conn)
    if argv[1:] == ["resume-needed"]:
        return resume_needed_cmd(conn)
    if argv[1:] == ["handoff"]:
        # Runs after the compositor has gone: a GUI here has nowhere to draw.
        return handoff_cmd(conn)
    if argv[1:] and argv[1:] not in (["close"], ["resume"]):
        print(
            f"usage: {argv[0]} [close | migrate | resume | resume-needed | handoff]",
            file=sys.stderr,
        )
        return 2
    # The conversational entry points, and only those, may open a window.
    select_ui()
    if argv[1:] == ["close"]:
        return close_cmd(conn)
    if argv[1:] == ["resume"]:
        return resume_cmd(conn)
    return gate(conn)


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv))
    except Exception as exc:  # this program must fail politely, never traceback
        print(f"gate error: {exc}", file=sys.stderr)
        sys.exit(1)
