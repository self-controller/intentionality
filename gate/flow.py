"""The gate's state machine: ELICIT -> CONFIRM -> COMMIT.

Imports neither sqlite3 nor anything network-facing — store and the
elicitation source are reached only through their functions.
"""

from . import manual, store, ui
from .provider import Provider, ProviderUnavailable


def run(conn, provider: Provider | None = None) -> int | None:
    draft = None
    mode = "manual"

    if provider is not None:
        try:
            draft = provider.elicit(None)
            mode = "ai"
        except ProviderUnavailable as exc:
            print(f"AI unavailable ({exc}) — switching to manual entry.")
            provider = None

    while True:
        # ELICIT (manual path, and every redo after the AI path degrades)
        if draft is None:
            draft = manual.elicit()
            mode = "manual"

        # CONFIRM
        ui.show_tasks(draft.statement, draft.intended_minutes, draft.tasks)
        choice = ui.confirm_choice("[y] commit  [r] revise  [q] quit without saving > ", "yrq")

        if choice == "q":
            return None

        if choice == "r":
            if mode == "ai":
                feedback = ui.ask("What should change?\n> ")
                try:
                    draft = provider.revise(feedback)
                    continue
                except ProviderUnavailable as exc:
                    print(f"AI unavailable ({exc}) — switching to manual entry.")
                    provider = None
            draft = None
            continue

        # COMMIT
        session_id = store.commit_draft(
            conn, draft.statement, draft.intended_minutes, mode, draft.tasks
        )
        print(f"Session {session_id} started — {len(draft.tasks)} task(s).")
        return session_id
