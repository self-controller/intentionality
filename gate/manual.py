"""Manual elicitation: v0.1's whole brain, v0.2's fallback when the model is unreachable."""

from dataclasses import dataclass

from . import ui


@dataclass
class Draft:
    intended_minutes: int | None
    tasks: list[str]


def elicit(allow_empty: bool = False) -> Draft:
    # No "what do you want to get done?" question: the task list below is
    # that answer, written out.
    intended_minutes = ui.ask_int_or_blank(
        "How long will you be here? (minutes, blank = open-ended)\n> "
    )

    print("Tasks, one per line. Blank line to finish.")
    tasks: list[str] = []
    while True:
        title = ui.ask("> ")
        if title:
            tasks.append(title)
        elif tasks or allow_empty:
            break
        else:
            print("At least one task.")

    return Draft(intended_minutes=intended_minutes, tasks=tasks)
