import sys

from . import config, debrief, flow, handoff, store
from .ui import GateAborted


def gate(conn) -> int:
    try:
        for row in store.get_open_sessions(conn):
            store.mark_recovered(conn, row["id"])
            ended = store.get_session(conn, row["id"])["ended_at"]
            if ended:
                print(f"Session {row['id']} ended around {ended} (from heartbeat).")
            else:
                print(f"Session {row['id']} was never closed — no heartbeat, end time unknown.")
            if ui_yes("Resolve its tasks now? [y/n] > "):
                debrief.resolve_tasks(conn, row["id"])
            # Carry AFTER the debrief: what the user just resolved must not
            # reappear in the backlog.
            carried = store.carry_unfinished(conn, row["id"])
            if carried:
                print(f"{carried} unfinished task(s) moved to the backlog.")

        session_id = flow.run(conn)
    except GateAborted:
        print("\nNothing saved.")
        return 0
    if session_id is None:
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


def ui_yes(prompt: str) -> bool:
    from . import ui

    return ui.confirm_choice(prompt, "yn") == "y"


def main(argv: list[str]) -> int:
    conn = store.connect()
    store.init(conn)
    if argv[1:] == ["close"]:
        return close_cmd(conn)
    if argv[1:] == ["migrate"]:
        return migrate_cmd(conn)
    if argv[1:]:
        print(f"usage: {argv[0]} [close | migrate]", file=sys.stderr)
        return 2
    return gate(conn)


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv))
    except Exception as exc:  # this program must fail politely, never traceback
        print(f"gate error: {exc}", file=sys.stderr)
        sys.exit(1)
