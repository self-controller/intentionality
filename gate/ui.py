"""Terminal I/O helpers. All interaction goes through here.

A different front end can take over: gui.install() sets `backend` to an
object answering ask / confirm_choice / welcome, and the functions below hand
those three to it. Everything else is built on top of them, and the welcome
screen's list logic lives here in WelcomeList, so both front ends edit the
list by the same rules and a front end only ever draws it.
The terminal stays the default, and stays importable with nothing installed,
so a missing display can never cost the login path.
"""

import re
from collections.abc import Iterable, Sequence
from dataclasses import dataclass, field
from datetime import date, datetime, timedelta
from typing import NamedTuple


class GateAborted(Exception):
    """User bailed out (Ctrl-C / Ctrl-D on the terminal, SIGINT to the
    graphical gate). Nothing has been written."""


# None = this terminal. Set by gui.install(); read on every call so tests
# can swap it (and can keep replacing the functions themselves, as before).
backend = None


def _input(prompt: str) -> str:
    try:
        return input(prompt)
    except (EOFError, KeyboardInterrupt):
        raise GateAborted from None


def ask(prompt: str) -> str:
    return _input(prompt).strip()


def confirm_choice(prompt: str, choices: str) -> str:
    if backend is not None:
        return backend.confirm_choice(prompt, choices)
    while True:
        raw = ask(prompt).lower()
        if len(raw) == 1 and raw in choices:
            return raw
        print(f"Choose one of: {', '.join(choices)}")


# -- the welcome screen --


class Label(NamedTuple):
    """A label that already exists, as the details editor offers it."""

    name: str
    color: str


def clean_labels(names: Iterable[str]) -> tuple[str, ...]:
    """Trimmed, blanks gone, and each name once whatever its case -- the
    store's label names are case-insensitive, so "Billing" is "billing"."""
    seen: set[str] = set()
    out = []
    for name in names:
        name = name.strip()
        if name and name.lower() not in seen:
            seen.add(name.lower())
            out.append(name)
    return tuple(out)


@dataclass(frozen=True)
class Details:
    """What a task carries besides its title. Immutable, so a row can hold
    the loaded value and the edited one without copying."""

    notes: str = ""
    due_date: str | None = None  # 'YYYY-MM-DD', as the store keeps it
    labels: tuple[str, ...] = ()

    def same_as(self, other: "Details") -> bool:
        # Label order is only the order they were clicked in.
        def key(d: Details):
            return d.notes, d.due_date, sorted(n.lower() for n in d.labels)

        return key(self) == key(other)


@dataclass
class ActiveTask:
    """A backlog task as the welcome screen shows it."""

    id: int
    title: str
    carry_count: int
    can_finish: bool  # carried from a session, so Done has a session to be recorded in
    details: Details = field(default_factory=Details)


@dataclass
class NewTask:
    """A task typed on the welcome screen."""

    title: str
    details: Details = field(default_factory=Details)


@dataclass
class Plan:
    """What the welcome screen decided. Every task shown lands in exactly one
    of keep / done / delete; nothing is written until flow commits this."""

    keep: list[int]
    done: list[int]
    delete: list[int]
    new: list[NewTask]
    intended_minutes: int | None
    # Kept tasks whose details were changed here, and only those: the desktop
    # app may be editing the same backlog, and writing back what was merely
    # loaded would undo its edit.
    edits: dict[int, Details] = field(default_factory=dict)


DONE, DELETE = "done", "delete"
MARKED = "Undo the mark before changing its details."


@dataclass
class Row:
    title: str
    task: ActiveTask | None = None  # None = typed on this screen
    mark: str | None = None  # None = kept; DONE or DELETE
    details: Details | None = None

    def __post_init__(self):
        if self.details is None:
            self.details = self.task.details if self.task else Details()


class WelcomeList:
    """The welcome screen's list, with no drawing in it."""

    def __init__(self, tasks: list[ActiveTask]):
        self.rows = [Row(t.title, t) for t in tasks]

    def add(self, title: str) -> bool:
        title = title.strip()
        if title:
            self.rows.append(Row(title))
        return bool(title)

    def toggle(self, index: int, mark: str) -> str | None:
        """Mark a row DONE or DELETE, or unmark it if it already is. Returns
        why not, when it can't be done."""
        row = self.rows[index]
        if row.task is None:
            if mark == DONE:
                return "Only a task carried from an earlier session can be marked done."
            # Typed a minute ago: nothing to undo, so deleting just removes it.
            del self.rows[index]
            return None
        if mark == DONE and not row.task.can_finish:
            return "Only a task carried from an earlier session can be marked done."
        row.mark = None if row.mark == mark else mark
        return None

    def set_details(self, index: int, details: Details) -> str | None:
        """Replace a row's details. Returns why not, when it can't be done."""
        row = self.rows[index]
        if row.mark:
            return MARKED
        if details.due_date is not None and not is_day(details.due_date):
            return BAD_DUE
        row.details = Details(details.notes.strip(), details.due_date, clean_labels(details.labels))
        return None

    def remaining(self) -> int:
        """How many tasks the session would start with."""
        return sum(row.mark is None for row in self.rows)

    def plan(self, intended_minutes: int | None) -> Plan:
        carried = [row for row in self.rows if row.task is not None]
        kept = [row for row in carried if row.mark is None]
        return Plan(
            keep=[row.task.id for row in kept],
            done=[row.task.id for row in carried if row.mark == DONE],
            delete=[row.task.id for row in carried if row.mark == DELETE],
            new=[NewTask(row.title, row.details) for row in self.rows if row.task is None],
            intended_minutes=intended_minutes,
            edits={
                row.task.id: row.details
                for row in kept
                if not row.details.same_as(row.task.details)
            },
        )


def welcome(
    tasks: list[ActiveTask],
    notes: Sequence[str] = (),
    error: str = "",
    labels: Sequence[Label] = (),
) -> Plan:
    """The whole gate conversation: the tasks there are, edited into the
    tasks this session starts with.

    `notes` is what the recovery sweep said. The terminal printed it already
    and ignores it here; a graphical front end shows it under the greeting.
    `error` is why the last answer was not accepted. `labels` are the ones
    that exist, to tag a task with; a new one can always be typed.
    """
    if backend is not None:
        return backend.welcome(list(tasks), list(notes), error, list(labels))
    return _terminal_welcome(tasks, error, list(labels))


def greeting(now: datetime) -> tuple[str, str]:
    """('Good morning', 'Thursday, September 17')."""
    if 5 <= now.hour < 12:
        part = "morning"
    elif 12 <= now.hour < 17:
        part = "afternoon"
    else:
        part = "evening"
    return f"Good {part}", f"{now:%A, %B} {now.day}"


MINUTES_QUESTION = "How long will you be here? (minutes, blank = open-ended)"


def parse_minutes(raw: str) -> int | None:
    """Blank = open-ended. Anything else must be a whole number above zero,
    or ValueError says so in words fit to show."""
    raw = raw.strip()
    if not raw:
        return None
    try:
        minutes = int(raw)
    except ValueError:
        minutes = 0
    if minutes < 1:
        raise ValueError("Minutes must be a whole number above zero, or blank for open-ended.")
    return minutes


EMPTY_HINT = "Add at least one task to start."
BAD_DUE = "A due date is a day like 2026-09-18, or today or tomorrow."


def is_day(text: str) -> bool:
    """'YYYY-MM-DD' exactly. fromisoformat alone also takes '20260918', and
    the store's string order is date order only in the one fixed width."""
    try:
        return date.fromisoformat(text).isoformat() == text
    except ValueError:
        return False


def parse_due(raw: str, today: date) -> str | None:
    """A typed due date, as the store keeps it. Blank or '-' = none."""
    raw = raw.strip().lower()
    if raw in ("", "-"):
        return None
    if raw == "today":
        return today.isoformat()
    if raw == "tomorrow":
        return (today + timedelta(days=1)).isoformat()
    if not is_day(raw):
        raise ValueError(BAD_DUE)
    return raw


def due_label(due_date: str | None, today: date) -> str:
    """A due date in words, relative to today. The welcome screen is where a
    due date decides anything, so it is shown there and nowhere else here.

    '' for no date -- and for a malformed one: this runs at login, and a bad
    row must cost its label, never the gate.
    """
    if not due_date:
        return ""
    try:
        due = date.fromisoformat(due_date)
    except ValueError:
        return ""
    days = (due - today).days
    if days < 0:
        return f"overdue, was due {due:%b} {due.day}"
    if days == 0:
        return "due today"
    if days == 1:
        return "due tomorrow"
    if days < 7:
        return f"due {due:%A}"
    return f"due {due:%b} {due.day}"


def row_detail(row: Row, today: date) -> str:
    """'due today · school, urgent · notes' -- what the welcome
    screen says beside a title. Built from the row's current details, so an
    edit shows at once."""
    parts = []
    due = due_label(row.details.due_date, today)
    if due:
        parts.append(due)
    if row.details.labels:
        parts.append(", ".join(row.details.labels))
    if row.details.notes:
        parts.append("notes")
    return " · ".join(parts)


def ask_minutes() -> int | None:
    while True:
        try:
            return parse_minutes(ask(f"{MINUTES_QUESTION}\n> "))
        except ValueError as exc:
            print(exc)


_HELP = (
    "Type a task to add it · d N = done · x N = delete (again to undo)"
    " · e N = due date, notes, labels · blank line to start"
)
_COMMAND = re.compile(r"([dxe])\s*(\d+)", re.IGNORECASE)


def _terminal_welcome(tasks: list[ActiveTask], error: str, labels: list[Label]) -> Plan:
    title, date_line = greeting(datetime.now())
    print(f"\n{title}. {date_line}.")
    print(_HELP)
    rows = WelcomeList(tasks)
    message = error
    show = True
    while True:
        # The list is reprinted when it changed in a way typing didn't show.
        if show:
            _print_rows(rows)
            show = False
        if message:
            print(message)
            message = ""
        raw = ask("> ")
        if not raw:
            if rows.remaining():
                break
            message = "Add at least one task first."
            continue
        command = _COMMAND.fullmatch(raw)
        if command is None:
            rows.add(raw)
            continue
        number = int(command[2])
        if not 1 <= number <= len(rows.rows):
            message = f"There is no task {number}."
            continue
        letter = command[1].lower()
        if letter == "e":
            message = _terminal_details(rows, number - 1, labels)
        else:
            mark = DONE if letter == "d" else DELETE
            message = rows.toggle(number - 1, mark) or ""
        show = True
    return rows.plan(ask_minutes())


def _terminal_details(rows: WelcomeList, index: int, labels: list[Label]) -> str:
    """e N: three questions, each kept by a blank answer and cleared by '-'."""
    row = rows.rows[index]
    if row.mark:
        return MARKED
    old = row.details
    print(f'Details for "{row.title}". Blank keeps what is there, - clears it.')
    while True:
        raw = ask(f"Due date (YYYY-MM-DD, today, tomorrow) [{old.due_date or 'none'}] > ")
        if not raw:
            due = old.due_date
            break
        try:
            due = parse_due(raw, date.today())
            break
        except ValueError as exc:
            print(exc)
    raw = ask(f"Notes, one line [{_preview(old.notes)}] > ")
    notes = old.notes if not raw else "" if raw == "-" else raw
    existing = ", ".join(label.name for label in labels) or "none yet"
    print(f"Labels there are: {existing}. A name that isn't there is created.")
    raw = ask(f"Labels, comma-separated [{', '.join(old.labels) or 'none'}] > ")
    chosen = old.labels if not raw else () if raw == "-" else tuple(raw.split(","))
    return rows.set_details(index, Details(notes, due, chosen)) or ""


def _preview(notes: str) -> str:
    if not notes:
        return "none"
    first = notes.splitlines()[0]
    more = len(first) > 40 or "\n" in notes
    return first[:40] + ("…" if more else "")


def _print_rows(rows: WelcomeList) -> None:
    if not rows.rows:
        print("  (no tasks yet)")
    today = date.today()
    for number, row in enumerate(rows.rows, start=1):
        detail = row_detail(row, today)
        detail = f"  ({detail})" if detail else ""
        mark = f"  [{row.mark}]" if row.mark else ""
        print(f"  {number}. {row.title}{detail}{mark}")
