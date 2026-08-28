"""The gate's state machine: PULL -> ELICIT -> CONFIRM -> COMMIT.

Imports neither sqlite3 nor anything network-facing — the store is reached
only through its functions. Elicitation is manual by design: the gate is a
list you type, not a conversation.
"""

from . import manual, store, ui


def run(conn) -> int | None:
    pulled = _pick_backlog(conn, store.get_backlog(conn))

    draft = None
    while True:
        # ELICIT
        if draft is None:
            draft = manual.elicit(allow_empty=bool(pulled))

        # CONFIRM
        titles = draft.tasks + [f"{t['title']}  (carried)" for t in pulled]
        ui.show_tasks(draft.statement, draft.intended_minutes, titles)
        choice = ui.confirm_choice("[y] commit  [r] revise  [q] quit without saving > ", "yrq")

        if choice == "q":
            return None
        if choice == "r":
            draft = None
            continue

        # COMMIT
        session_id = store.commit_draft(
            conn, draft.statement, draft.intended_minutes, "manual", draft.tasks,
            backlog_ids=[t["id"] for t in pulled],
        )
        print(f"Session {session_id} started — {len(titles)} task(s).")
        return session_id


def _pick_backlog(conn, backlog) -> list:
    """Offer the backlog; return the rows the user pulls into this session."""
    if not backlog:
        return []
    print("\nCarried over:")
    for i, task in enumerate(backlog, start=1):
        count = store.carry_count(conn, task["id"])
        times = f"  (carried {count}×)" if count else ""
        print(f"  {i}. {task['title']}{times}")
    picks = ui.pick_numbers("Pull in? (numbers, blank for none) > ", len(backlog))
    return [backlog[i - 1] for i in picks]
