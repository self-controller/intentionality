import sys

from . import config, debrief, flow, handoff, store
from .provider import ProviderUnavailable
from .ui import GateAborted


def _load_provider():
    """Build the AI provider, or return None so the gate runs on manual entry.

    A missing anthropic package, a missing key, or any construction failure
    must degrade the gate, never break it.
    """
    try:
        from .claude import ClaudeProvider
    except ModuleNotFoundError as exc:
        if exc.name != "anthropic":  # a missing SDK is expected; a broken gate is not
            print(f"AI unavailable (import failed: {exc}) — manual entry.")
        return None
    try:
        return ClaudeProvider()
    except ProviderUnavailable as exc:
        print(f"AI unavailable ({exc}) — manual entry.")
        return None


def gate(conn) -> int:
    try:
        for row in store.get_open_sessions(conn):
            store.mark_recovered(conn, row["id"])
            print(f"Previous session {row['id']} was never closed — marked recovered.")
            if ui_yes("Resolve its tasks now? [y/n] > "):
                debrief.resolve_tasks(conn, row["id"])

        session_id = flow.run(conn, provider=_load_provider())
    except GateAborted:
        print("\nNothing saved.")
        return 0
    if session_id is None:
        return 0

    if not config.DESKTOP_CMD:
        print("No desktop command configured — run `gate close` to end the session.")
        return 0

    # HANDOFF: the desktop runs as a child; the gate waits, then closes and
    # debriefs. The closing write happens before the debrief so a skipped
    # debrief still leaves a complete record.
    if handoff.launch_and_wait(session_id) is None:
        return 1
    store.close_session(conn, session_id)
    _debrief_politely(conn, session_id)
    return 0


def close_cmd(conn) -> int:
    row = store.latest_open_session(conn)
    if row is None:
        print("No open session.")
        return 0
    store.close_session(conn, row["id"])
    _debrief_politely(conn, row["id"])
    return 0


def _debrief_politely(conn, session_id: int) -> None:
    # The session is already closed; skipping the debrief loses nothing.
    try:
        debrief.run(conn, session_id)
    except GateAborted:
        print("\nDebrief skipped.")


def ui_yes(prompt: str) -> bool:
    from . import ui

    return ui.confirm_choice(prompt, "yn") == "y"


def main(argv: list[str]) -> int:
    conn = store.connect()
    store.init(conn)
    if argv[1:] == ["close"]:
        return close_cmd(conn)
    if argv[1:]:
        print(f"usage: {argv[0]} [close]", file=sys.stderr)
        return 2
    return gate(conn)


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv))
    except Exception as exc:  # this program must fail politely, never traceback
        print(f"gate error: {exc}", file=sys.stderr)
        sys.exit(1)
