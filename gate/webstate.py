"""The welcome screen's state, as data.

No gi, no WebKit, no display. This module turns the front end's messages into
``ui.WelcomeList`` calls and turns the list back into a dict the bundle
renders. That is what makes the graphical gate testable at last: the GTK
version's state *was* its widget tree, so none of it could be asserted without
a display, and ``tests/test_gui.py`` said so.

Python stays the authority for every string on the screen. ``ui.greeting``,
``ui.row_detail`` and the refusal constants are computed here and travel as
finished text, so there is exactly one implementation of each and the front
end cannot disagree with the terminal about what a button means.
"""

import json
from dataclasses import replace
from datetime import date, datetime

from . import autolabel, ui

# What a marked row says instead of its details.
MARK_WORDS = {ui.DONE: "done", ui.DELETE: "will be deleted"}


def parse_keybinds(raw: str) -> dict[str, str]:
    """The shortcuts changed in the app's Settings, as action id -> combo.
    Anything that is not a JSON object of strings is no changes at all: a
    hand edit gone wrong must leave the default keys working, not none."""
    try:
        value = json.loads(raw)
    except (TypeError, ValueError):
        return {}
    if not isinstance(value, dict):
        return {}
    return {k: v for k, v in value.items() if isinstance(k, str) and isinstance(v, str)}


class WelcomeScreen:
    """One run of the welcome screen. ``plan`` is None until Start succeeds."""

    def __init__(
        self,
        tasks: list[ui.ActiveTask],
        notes: list[str],
        error: str,
        labels: list[ui.Label],
        now: datetime | None = None,
        keybinds: str = "{}",
    ):
        self.rows = ui.WelcomeList(tasks)
        self.keys = parse_keybinds(keybinds)
        self.notes = " ".join(notes)
        self.error = error
        self.known = list(labels)
        # Labels invented on this screen. They are offered on every row
        # afterwards, so a tag typed once does not have to be typed again.
        self.created: list[str] = []
        self.editing: int | None = None
        self.panel_error = ""
        # Bumped only when the front end should reseed the panel's fields --
        # on open and on a successful save, never on an unrelated redraw.
        self.token = 0
        self.minutes = ""
        # What is sitting in the add box uncommitted. It counts towards
        # starting, so the count and the Start rule have to know about it.
        self.add_text = ""
        # Bumped each time the add box's task lands, and at no other time: the
        # front end empties the box on that, never on its own, so a refused
        # add (a bad due date in its details) keeps everything that was typed.
        self.added = 0
        self.add_error = ""
        # Typed rows that are not Jev's to label: already asked about, or
        # labelled by hand (even to none) when they were added or saved.
        self._settled: list[ui.Row] = []
        self.plan: ui.Plan | None = None
        self._now = now or datetime.now()

    # ---------------------------------------------------------------- state

    def _today(self) -> date:
        return self._now.date()

    def _row_state(self, index: int, row: ui.Row) -> dict:
        if row.mark:
            detail = MARK_WORDS.get(row.mark, "")
        else:
            detail = ui.row_detail(row, self._today())
            if row.task is None:
                detail = f"new · {detail}" if detail else "new"
        return {
            "index": index,
            "title": row.title,
            "mark": row.mark,
            "today": row.today,
            # Starts in Doing rather than To Do (only ever on a ticked row).
            "doing": row.doing,
            "detail": detail,
            # The pieces of that line the front end lays out itself: the due
            # date in words (blank on a marked row, whose line is the mark's
            # word) and whether there are notes. `detail` stays whole, for
            # the terminal's twin and for a bundle older than this file.
            "due": "" if row.mark else ui.due_label(row.details.due_date, self._today()),
            "notes": bool(row.details.notes),
            "can_finish": bool(row.task and row.task.can_finish),
            "typed": row.task is None,
            # The front end groups rows by these, so a saved edit moves the row.
            "labels": list(row.details.labels),
        }

    def _labels_state(self) -> list[dict]:
        out = [{"name": l.name, "color": l.color, "exists": True} for l in self.known]
        known = {l.name.lower() for l in self.known}
        for name in self.created:
            if name.lower() not in known:
                out.append({"name": name, "color": None, "exists": False})
        return out

    def _panel_state(self) -> dict | None:
        if self.editing is None or not (0 <= self.editing < len(self.rows.rows)):
            return None
        row = self.rows.rows[self.editing]
        return {
            "index": self.editing,
            "token": self.token,
            "due": row.details.due_date or "",
            "notes": row.details.notes,
            "labels": list(row.details.labels),
            "error": self.panel_error,
        }

    def counted(self) -> int:
        """Tasks the session would start with, counting the add box."""
        return self.rows.today_count() + (1 if self.add_text.strip() else 0)

    def state(self) -> dict:
        greeting, date_line = ui.greeting(self._now)
        return {
            "screen": "welcome",
            "greeting": greeting,
            "date": date_line,
            "notes": self.notes,
            "error": self.error,
            "rows": [self._row_state(i, r) for i, r in enumerate(self.rows.rows)],
            "labels": self._labels_state(),
            "panel": self._panel_state(),
            "minutes": self.minutes,
            "start_enabled": self.counted() > 0,
            "added": self.added,
            "add_error": self.add_error,
            "keys": self.keys,
        }

    # -------------------------------------------------------------- sorting

    def candidates(self) -> list[autolabel.Candidate]:
        """Every label a typed task could be filed under: the store's, with
        what is filed there, and the ones invented on this screen, by name."""
        out = [autolabel.Candidate(l.name, tuple(l.examples)) for l in self.known]
        known = {l.name.lower() for l in self.known}
        out += [autolabel.Candidate(name) for name in self.created if name.lower() not in known]
        return out

    def to_sort(self) -> list[ui.Row]:
        """Typed rows with no labels that Jev has not been asked about yet.
        Each is handed out once: asking is the caller's, off this thread."""
        fresh = [
            row for row in self.rows.rows
            if row.task is None and not row.details.labels
            and not any(row is done for done in self._settled)
        ]
        self._settled.extend(fresh)
        return fresh

    def sorted(self, row: ui.Row, label: str | None) -> bool:
        """Jev's answer for a row from to_sort. It goes on only if the row is
        still on the screen, still wears nothing and is not open in the panel:
        an answer that arrives after the user has started choosing loses."""
        if label is None:
            return False
        index = next((i for i, r in enumerate(self.rows.rows) if r is row), None)
        if index is None or row.mark or row.details.labels or self.editing == index:
            return False
        row.details = replace(row.details, labels=(label,))
        return True

    def replan(self) -> ui.Plan | None:
        """The plan again, with any labels that landed after Start."""
        if self.plan is not None:
            self.plan = self.rows.plan(self.plan.intended_minutes)
        return self.plan

    # ------------------------------------------------------------- messages

    def handle(self, msg: dict) -> None:
        """Apply one front-end message. Unknown ops are ignored on purpose:
        a bundle newer than this file must not be able to crash the login."""
        op = msg.get("op")
        handler = getattr(self, f"_op_{op}", None) if isinstance(op, str) else None
        if handler is None:
            return
        # A redraw clears the last refusal, the way the GTK screen did.
        self.error = ""
        handler(msg)

    def _index(self, msg: dict) -> int | None:
        index = msg.get("index")
        if isinstance(index, int) and 0 <= index < len(self.rows.rows):
            return index
        return None

    def _op_add_text(self, msg: dict) -> None:
        self.add_text = str(msg.get("text", ""))
        self.add_error = ""

    def _op_add(self, msg: dict) -> None:
        self._add(str(msg.get("title", "")), msg.get("details"), msg.get("doing") is True)

    def _add(self, title: str, fields, doing: bool = False) -> bool:
        """The add box's task, with whatever its details editor holds, in
        Doing if its switch is on. False = nothing landed: blank, or the
        details were refused (add_error says why), and either way the box
        keeps what was typed."""
        details = None
        if isinstance(fields, dict):
            try:
                details = self._details_from(fields)
            except ValueError as exc:
                self.add_error = str(exc)
                return False
        if not self.rows.add(title):
            return False
        index = len(self.rows.rows) - 1
        if doing:
            self.rows.toggle_doing(index)
        if details is not None:
            # A row typed a moment ago carries no mark, so only the due date
            # could be refused, and _details_from has checked it already.
            self.rows.set_details(index, details)
            self._remember_labels(index)
            if details.labels:
                self._settled.append(self.rows.rows[index])
        self.add_text = ""
        self.add_error = ""
        self.added += 1
        return True

    def _op_today(self, msg: dict) -> None:
        index = self._index(msg)
        if index is None:
            return
        self.rows.toggle_today(index)

    def _op_doing(self, msg: dict) -> None:
        index = self._index(msg)
        if index is None:
            return
        self.rows.toggle_doing(index)

    def _op_mark(self, msg: dict) -> None:
        index = self._index(msg)
        mark = msg.get("mark")
        if index is None or mark not in (ui.DONE, ui.DELETE):
            return
        # Row numbers shift when a typed row is deleted outright, so an open
        # panel can no longer be trusted to point at what it was opened on.
        self.editing = None
        refusal = self.rows.toggle(index, mark)
        if refusal:
            self.error = refusal

    def _op_details_open(self, msg: dict) -> None:
        index = self._index(msg)
        if index is None:
            return
        self.editing = index
        self.panel_error = ""
        self.token += 1

    def _op_details_close(self, _msg: dict) -> None:
        self.editing = None
        self.panel_error = ""

    def _op_details_save(self, msg: dict) -> None:
        self._save_details(msg)

    def _details_from(self, msg: dict) -> ui.Details:
        """The editor's fields as Details, the same for a row's panel and for
        the add box. ValueError = the due date is not one, in words fit to
        show."""
        names = [str(n) for n in msg.get("labels", []) if isinstance(n, (str, int))]
        # A label typed but not yet turned into a chip was still meant.
        pending = str(msg.get("pending", "")).strip()
        if pending:
            names.append(pending)
        due = ui.parse_due(str(msg.get("due", "")), self._today())
        return ui.Details(str(msg.get("notes", "")), due, tuple(names))

    def _remember_labels(self, index: int) -> None:
        """Offer the row's new labels on every row from now on."""
        known = {l.name.lower() for l in self.known} | {n.lower() for n in self.created}
        for name in self.rows.rows[index].details.labels:
            if name.lower() not in known:
                self.created.append(name)
                known.add(name.lower())

    def _save_details(self, msg: dict) -> bool:
        """Mirrors the GTK panel's try_save. False = refused, and the panel
        stays open showing why."""
        index = self._index(msg)
        if index is None:
            return False
        try:
            details = self._details_from(msg)
        except ValueError as exc:
            self.panel_error = str(exc)
            return False
        refusal = self.rows.set_details(index, details)
        if refusal is not None:
            self.panel_error = refusal
            return False
        self._remember_labels(index)
        # Saved by hand: whatever labels it has now, even none, are chosen.
        self._settled.append(self.rows.rows[index])
        self.panel_error = ""
        self.editing = None
        self.token += 1
        return True

    def _op_submit(self, msg: dict) -> None:
        """Start. Everything uncommitted arrives in this one message, so an
        open panel is saved first and a refusal stops the start -- the same
        order the GTK screen used."""
        details = msg.get("details")
        if self.editing is not None and isinstance(details, dict):
            if not self._save_details(details):
                return
        self.minutes = str(msg.get("minutes", ""))
        # The add box's task keeps its details, and a refusal there stops the
        # start just as the panel's does.
        add_text = str(msg.get("add_text", ""))
        if add_text.strip() and not self._add(
            add_text, msg.get("add_details"), msg.get("add_doing") is True
        ):
            return
        self.add_text = ""
        if not self.rows.today_count():
            self.error = ""
            return
        try:
            minutes = ui.parse_minutes(self.minutes)
        except ValueError as exc:
            self.error = str(exc)
            return
        self.plan = self.rows.plan(minutes)
