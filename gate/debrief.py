"""End-of-session debrief: show the gap, resolve tasks in one keystroke each.

Budget is about sixty seconds — anything longer gets rage-skipped and takes
the ritual's credibility with it. Bailing out (Ctrl-C) is always allowed and
loses nothing: the session record is already closed before this runs, and
every task left unanswered is still unfinished, so the carry picks it up.
"""

from datetime import datetime

from . import store, ui


def run(conn, session_id: int) -> None:
    session = store.get_session(conn, session_id)
    print(f"\n— Session {session_id}: {store.session_label(conn, session)}")
    print(f"  {_duration_line(session)}")
    resolve_tasks(conn, session_id)


def resolve_tasks(conn, session_id: int) -> None:
    tasks = [t for t in store.get_tasks(conn, session_id) if t["status"] in store.UNFINISHED]
    if not tasks:
        print("  No open tasks.")
        return

    print("  [d] done   [n] not done   [x] drop for good")
    done = dropped = 0
    for task in tasks:
        doing = " (doing)" if task["status"] == "doing" else ""
        choice = ui.confirm_choice(f"  {task['position']}. {task['title']}{doing}  > ", "dnx")
        if choice == "d":
            store.resolve_task(conn, task["id"], "done")
            done += 1
        elif choice == "x":
            # The only answer that ends a task here. 'n' writes nothing at
            # all: staying unfinished is exactly what the carry looks for.
            store.resolve_task(conn, task["id"], "dropped")
            dropped += 1

    # No carried count: the caller prints it right after the carry itself,
    # which is the number that actually landed in the backlog.
    tail = f", {dropped} dropped" if dropped else ""
    print(f"  {done}/{len(tasks)} done{tail}.")


def _duration_line(session) -> str:
    if session["ended_at"] is None:
        return "never closed — duration unknown"
    start = datetime.fromisoformat(session["started_at"])
    end = datetime.fromisoformat(session["ended_at"])
    minutes = round((end - start).total_seconds() / 60)
    if session["intended_minutes"] is not None:
        return f"{minutes} min (intended {session['intended_minutes']})"
    return f"{minutes} min (open-ended)"
