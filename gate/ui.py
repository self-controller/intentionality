"""Terminal I/O helpers. All interaction goes through here."""


class GateAborted(Exception):
    """User bailed out (Ctrl-C / Ctrl-D). Nothing has been written."""


def _input(prompt: str) -> str:
    try:
        return input(prompt)
    except (EOFError, KeyboardInterrupt):
        raise GateAborted from None


def ask(prompt: str) -> str:
    return _input(prompt).strip()


def ask_int_or_blank(prompt: str) -> int | None:
    while True:
        raw = ask(prompt)
        if raw == "":
            return None
        try:
            return int(raw)
        except ValueError:
            print("Enter a whole number of minutes, or leave blank.")


def confirm_choice(prompt: str, choices: str) -> str:
    while True:
        raw = ask(prompt).lower()
        if len(raw) == 1 and raw in choices:
            return raw
        print(f"Choose one of: {', '.join(choices)}")


def show_tasks(statement: str, intended_minutes: int | None, titles: list[str]) -> None:
    duration = f"{intended_minutes} min" if intended_minutes is not None else "open-ended"
    print(f"\n  {statement}  ({duration})")
    for pos, title in enumerate(titles, start=1):
        print(f"  {pos}. {title}")
    print()
