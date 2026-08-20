"""The model boundary.

A provider runs the AI side of the gate conversation and returns the same Draft
that manual.elicit() produces, so flow.py cannot tell the two apart. Any
provider failure — offline, bad key, rate limit, refusal, runaway conversation —
surfaces as ProviderUnavailable, and the caller falls back to manual entry.
The gate must never hang and never crash because a model misbehaved.
"""

from typing import Protocol

from .manual import Draft


class ProviderUnavailable(Exception):
    """The model can't be used right now. Fall back to manual elicitation."""


class Provider(Protocol):
    def elicit(self, previous_gap_summary: str | None) -> Draft:
        """Run the conversation, return a draft ready for user confirmation.

        previous_gap_summary carries yesterday's intention-vs-outcome gap once
        the debrief exists (v0.3+); None until then.
        """
        ...

    def revise(self, feedback: str) -> Draft:
        """The user rejected the last draft; produce a revised one."""
        ...
