"""A new task, filed under one of the labels there are, by asking Jev.

A task typed on the welcome screen with no labels of its own is shown to
TypeSafe's Jev beside every label and the tasks already filed under each, and
comes back with at most one of them. The desktop app does the same for the
cards it creates (app/src-tauri/src/autolabel.rs). The questions are worded
once in each language and tests/test_autolabel.py holds the two to the same
text, so a task typed here and one typed on the board are judged alike.

One request, two kinds of question, answered in parallel (about 0.2 s):

- a Choice over the labels -- which one the task goes under. Its probability
  is shared out between labels that overlap ("School" and a course's own
  label), so a split between two of them shows as neither being picked
  clearly.
- a Noul per label -- does the task belong there at all? Each is answered on
  its own, so all of them can be low, which the Choice cannot say.

The label goes on only when Jev both picks it (Choice probability >= MIN) and
says it belongs (its Noul >= MIN). Anything less leaves the task as it was: a
wrong label costs a removal, so a doubtful one is not put on. MIN was set on
the live store's own labelled tasks (2026-10-29, leave-one-out): at 0.7 every
clear case was filed and every pick under 0.45 -- the wrong ones -- was not.

This is the one network call the gate makes. It never raises and never
blocks the screen: no key, no network or a refusal all mean no label.
Stdlib only, like the rest of the gate.
"""

import json
import os
import sys
import time
import urllib.error
import urllib.request
from collections.abc import Sequence
from pathlib import Path
from typing import NamedTuple

# Kept equal to the app's constants of the same names (tests/test_autolabel.py).
MODEL = "jev-latest"
API_URL = "https://api.typesafe.ai/v1/systemone"
# Points the call somewhere else: a local stub, for testing without a key.
API_URL_ENV = "TYPESAFE_API_URL"
KEY_FILE = ".config/intentionality/typesafe_key"
KEY_ENV = "TYPESAFE_API_KEY"
MIN = 0.7
# Long enough for a slow network, short enough that an answer still lands
# while the screen is up.
TIMEOUT = 5.0
# 429 and 529 ask for a backoff: one short wait and one more try.
RETRY_AFTER = 0.4

CHOICE_QUESTION = (
    "The user files each of their tasks under a label. Which of these labels should the task "
    "given in `task` (its title, and the user's notes on it if any) be filed under? Each label "
    "is described by its name and the tasks the user has already filed under it, newest first. "
    "If more than one label fits, choose the most specific: the one for that particular course, "
    "project or job rather than a broader one it also falls under."
)
NOUL_QUESTION = (
    "Does the task given in `task` (its title, and the user's notes on it if any) belong under "
    "the label given in this question's `label`, going by the label's name and the tasks the "
    "user has already filed under it?"
)
CRITERIA_TRUE = (
    "The task is part of the course, project, job or area of life that the label's name names "
    "or that the tasks already filed under it share, so the user would file it there too."
)
CRITERIA_FALSE = (
    "The task belongs to a different course, project, job or area of life, or nothing in the "
    "label's name or its tasks connects the task to it."
)


class Candidate(NamedTuple):
    """A label a task could go under, and titles already filed there."""

    name: str
    examples: tuple[str, ...] = ()


def state(title: str, notes: str) -> dict:
    """The task as Jev is shown it. Empty notes are left out, not sent blank."""
    task = {"title": title.strip()}
    if notes.strip():
        task["notes"] = notes.strip()
    return {"task": task}


def questions(candidates: Sequence[Candidate]) -> dict:
    """The Choice under `label` and a Noul per candidate under `l<index>`. A
    label nothing wears yet is offered by its name alone."""
    out = {
        "label": {
            "type": "choice",
            "instructions": CHOICE_QUESTION,
            "criteria": {
                c.name: {"tasks_already_filed_here": list(c.examples)} if c.examples else None
                for c in candidates
            },
        }
    }
    for index, c in enumerate(candidates):
        label = {"name": c.name}
        if c.examples:
            label["tasks_already_filed_here"] = list(c.examples)
        out[f"l{index}"] = {
            "type": "noul",
            "instructions": {"question": NOUL_QUESTION, "label": label},
            "criteria": {"true": CRITERIA_TRUE, "false": CRITERIA_FALSE},
        }
    return out


def _unit(value) -> float:
    return float(value) if isinstance(value, (int, float)) else 0.0


def decide(answers: dict, candidates: Sequence[Candidate]) -> str | None:
    """The label to put on, or None. Policy lives here, not in the model."""
    names = [c.name for c in candidates]
    pick = answers.get("label")
    if not isinstance(pick, dict) or pick.get("choice") not in names:
        return None
    name = pick["choice"]
    probabilities = pick.get("probabilities")
    picked = _unit(probabilities.get(name)) if isinstance(probabilities, dict) else 0.0
    fits = answers.get(f"l{names.index(name)}")
    belongs = _unit(fits.get("noul")) if isinstance(fits, dict) else 0.0
    return name if picked >= MIN and belongs >= MIN else None


def api_key() -> str | None:
    try:
        key = (Path.home() / KEY_FILE).read_text().strip()
    except OSError:
        key = ""
    return key or os.environ.get(KEY_ENV, "").strip() or None


def ask(title: str, notes: str, candidates: Sequence[Candidate], timeout: float = TIMEOUT) -> str | None:
    """The label Jev files the task under, or None. Blocks for one request;
    the graphical gate calls it off the main thread. Logs to stderr -- the
    gate's stdout is the screen's log pane, and this may run on a thread."""
    if not candidates or not title.strip():
        return None
    key = api_key()
    if key is None:
        return None
    body = json.dumps(
        {"model": MODEL, "state": state(title, notes), "questions": questions(candidates)}
    ).encode()
    request = urllib.request.Request(
        os.environ.get(API_URL_ENV) or API_URL,
        data=body,
        headers={"Authorization": f"Bearer {key}", "Content-Type": "application/json"},
        method="POST",
    )
    for attempt in range(2):
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                payload = json.load(response)
            break
        except urllib.error.HTTPError as exc:
            if exc.code in (429, 529) and attempt == 0:
                time.sleep(RETRY_AFTER)
                continue
            _log(f"could not sort {title!r}: TypeSafe {exc.code}")
            return None
        except (OSError, ValueError) as exc:
            _log(f"could not sort {title!r}: {exc}")
            return None
    answers = payload.get("answers") if isinstance(payload, dict) else None
    if not isinstance(answers, dict):
        _log(f"could not sort {title!r}: TypeSafe answered without answers")
        return None
    label = decide(answers, candidates)
    _log(f"{title!r} filed under {label!r}" if label else f"{title!r} left unlabelled")
    return label


def _log(line: str) -> None:
    print(f"intentionality gate: {line}", file=sys.stderr, flush=True)
