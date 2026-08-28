"""Dashboard entry point.

    python3 -m dashboard          latest session in detail + recent list
    python3 -m dashboard N        session N in detail
    python3 -m dashboard list     recent sessions only

Reads the gate's store and ActivityWatch's local API; writes nothing.
"""

import sys

from gate import store

from . import report


def main(argv: list[str]) -> int:
    conn = store.connect()
    store.init(conn)

    arg = argv[1] if argv[1:] else None
    sessions = conn.execute(
        "SELECT * FROM session ORDER BY id DESC LIMIT 10"
    ).fetchall()
    if not sessions:
        print("No sessions yet — run `python3 -m gate` first.")
        return 0

    if arg == "list":
        report.print_session_list(sessions)
        return 0

    if arg is not None:
        try:
            session = store.get_session(conn, int(arg))
        except ValueError:
            print(f"usage: {argv[0]} [N | list]", file=sys.stderr)
            return 2
        if session is None:
            print(f"No session {arg}.", file=sys.stderr)
            return 1
    else:
        session = sessions[0]

    report.print_session(session, store.get_tasks(conn, session["id"]))
    if arg is None and len(sessions) > 1:
        print()
        report.print_session_list(sessions[1:])
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv))
    except Exception as exc:  # same politeness rule as the gate
        print(f"dashboard error: {exc}", file=sys.stderr)
        sys.exit(1)
