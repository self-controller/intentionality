"""End-of-session debrief: show the gap, resolve tasks in one keystroke each.

Budget is about sixty seconds — anything longer gets rage-skipped and takes
the ritual's credibility with it. Skipping (s, or Ctrl-C) is always allowed
and loses nothing: the session record is already closed before this runs.
"""

from datetime import datetime

from . import store, ui


def run(conn, session_id: int) -> None:
    session = store.get_session(conn, session_id)
    print(f"\n— Session {session_id}: {session['statement']}")
    print(f"  {_duration_line(session)}")
    resolve_tasks(conn, session_id)


def resolve_tasks(conn, session_id: int) -> None:
    tasks = [t for t in store.get_tasks(conn, session_id) if t["status"] == "planned"]
    if not tasks:
        print("  No open tasks.")
        return

    print("  [d] done   [n] not done   [s] skip")
    done = 0
    for task in tasks:
        choice = ui.confirm_choice(f"  {task['position']}. {task['title']}  > ", "dns")
        if choice == "d":
            store.resolve_task(conn, task["id"], "done")
            done += 1
        elif choice == "n":
            store.resolve_task(conn, task["id"], "dropped")
        # s: stays planned, unresolved — honest about not knowing

    print(f"  {done}/{len(tasks)} done.")


def _duration_line(session) -> str:
    if session["ended_at"] is None:
        return "never closed — duration unknown"
    start = datetime.fromisoformat(session["started_at"])
    end = datetime.fromisoformat(session["ended_at"])
    minutes = round((end - start).total_seconds() / 60)
    if session["intended_minutes"] is not None:
        return f"{minutes} min (intended {session['intended_minutes']})"
    return f"{minutes} min (open-ended)"
