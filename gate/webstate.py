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

from datetime import date, datetime

from . import ui

# What a marked row says instead of its details.
MARK_WORDS = {ui.DONE: "done", ui.DELETE: "will be deleted"}

EMPTY_HINT = "Add at least one task to start."


class WelcomeScreen:
    """One run of the welcome screen. ``plan`` is None until Start succeeds."""

    def __init__(
        self,
        tasks: list[ui.ActiveTask],
        notes: list[str],
        error: str,
        labels: list[ui.Label],
        now: datetime | None = None,
    ):
        self.rows = ui.WelcomeList(tasks)
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
            "detail": detail,
            "can_finish": bool(row.task and row.task.can_finish),
            "typed": row.task is None,
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
        return self.rows.remaining() + (1 if self.add_text.strip() else 0)

    def status(self) -> str:
        count = self.counted()
        if not count:
            return EMPTY_HINT
        return f"{count} task{'' if count == 1 else 's'} this session."

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
            "status": self.status(),
            "start_enabled": self.counted() > 0,
        }

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

    def _op_add(self, msg: dict) -> None:
        self.rows.add(str(msg.get("title", "")))
        self.add_text = ""

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

    def _save_details(self, msg: dict) -> bool:
        """Mirrors the GTK panel's try_save. False = refused, and the panel
        stays open showing why."""
        index = self._index(msg)
        if index is None:
            return False
        names = [str(n) for n in msg.get("labels", []) if isinstance(n, (str, int))]
        # A label typed but not yet turned into a chip was still meant.
        pending = str(msg.get("pending", "")).strip()
        if pending:
            names.append(pending)
        try:
            due = ui.parse_due(str(msg.get("due", "")), self._today())
        except ValueError as exc:
            self.panel_error = str(exc)
            return False
        details = ui.Details(str(msg.get("notes", "")), due, tuple(names))
        refusal = self.rows.set_details(index, details)
        if refusal:
            self.panel_error = refusal
            return False
        known = {l.name.lower() for l in self.known} | {n.lower() for n in self.created}
        for name in self.rows.rows[index].details.labels:
            if name.lower() not in known:
                self.created.append(name)
                known.add(name.lower())
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
        self.rows.add(str(msg.get("add_text", "")))
        self.add_text = ""
        if not self.rows.remaining():
            self.error = EMPTY_HINT
            return
        try:
            minutes = ui.parse_minutes(self.minutes)
        except ValueError as exc:
            self.error = str(exc)
            return
        self.plan = self.rows.plan(minutes)
