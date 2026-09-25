"""The gate's state machine: WELCOME -> COMMIT.

Imports neither sqlite3 nor anything network-facing — the store is reached
only through its functions. The gate is a list you edit, not a conversation:
the tasks already waiting, kept, finished or deleted, plus any you add.
There is no way out of run() without a session of at least one task, other
than an abort (GateAborted).
"""

from collections.abc import Sequence

from . import store, ui

EMPTY = ui.EMPTY_HINT
VANISHED = "Those tasks changed while you were here — add one to start."


def run(conn, notes: Sequence[str] = ()) -> int:
    error = ""
    while True:
        # WELCOME. Read fresh each time round: the desktop app may be running
        # (a resume gate) and editing the same backlog.
        labels_of = store.labels_by_task(conn)
        carried = store.backlog_carry_counts(conn)
        active = [
            ui.ActiveTask(
                id=task["id"],
                title=task["title"],
                carry_count=carried.get(task["id"], 0),
                can_finish=task["carried_from"] is not None,
                details=ui.Details(
                    task["notes"], task["due_date"], tuple(labels_of.get(task["id"], ()))
                ),
            )
            for task in store.get_backlog(conn)
            if task["status"] in store.UNFINISHED
        ]
        known = [ui.Label(row["name"], row["color"]) for row in store.list_labels(conn)]
        plan = ui.welcome(active, notes, error, known)

        # The front ends refuse an empty list already; this is the rule's
        # authority, so a front end that slips can't start an empty session.
        if not plan.keep and not plan.new:
            error = EMPTY
            continue

        # COMMIT. The statement is empty: the gate no longer asks for one.
        # The column stays for the sessions that have one, and
        # store.session_label falls back to the tasks for the ones that don't.
        try:
            session_id = store.commit_plan(
                conn, plan.intended_minutes, plan.keep, [task.title for task in plan.new],
                done_ids=plan.done, delete_ids=plan.delete,
                new_details=[task.details for task in plan.new], edits=plan.edits,
            )
        except store.EmptyPlan:
            # Every kept task left the backlog while the screen was up, and
            # nothing was typed. Rolled back, so nothing was deleted either.
            error = VANISHED
            continue
        count = len(store.get_tasks(conn, session_id))
        print(f"Session {session_id} started — {count} task(s).")
        return session_id

