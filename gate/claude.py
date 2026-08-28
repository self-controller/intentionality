"""Remote Claude implementation of the provider contract.

The conversation is free-form, but its output never is: the model must call
the strict `propose_tasks` tool, so the draft is machine-shaped by
construction. stop_reason is the state machine — "end_turn" means the model
asked a clarifying question, "tool_use" means the validated tool input is the
draft.
"""

import os

import anthropic

from . import config, ui
from .manual import Draft
from .provider import ProviderUnavailable

SYSTEM = """\
You are the morning gate on the user's laptop: a brief check-in that turns a
statement of intent into a concrete task list before their desktop starts.

The user has just answered "What do you want to get done this session?".

- If the answer is clear enough, call propose_tasks immediately.
- If it is vague, ask ONE short question to make it concrete. Never more than
  two questions in the whole conversation — this is a check-in, not an
  interrogation, and the user may be groggy.
- Keep every reply under 40 words. No greetings, no pep talks.
- statement: one sentence in the user's own words summarizing the session.
- intended_minutes: only if the user stated or clearly implied a duration;
  otherwise null. Never invent one.
- tasks: 1-7 concrete items, phrased close to the user's own words. Split
  compound answers into separate tasks; do not pad with tasks they never
  mentioned.
- When the user asks for changes to a proposal, call propose_tasks again with
  the revised list."""

PROPOSE_TASKS = {
    "name": "propose_tasks",
    "description": "Present the parsed session plan to the user for confirmation.",
    "strict": True,
    "input_schema": {
        "type": "object",
        "properties": {
            "statement": {"type": "string"},
            "intended_minutes": {"type": ["integer", "null"]},
            # The API's strict-tool schema subset rejects minItems/maxItems;
            # the 1-7 bound lives in the system prompt, non-empty is enforced
            # in _accept_proposal.
            "tasks": {
                "type": "array",
                "items": {"type": "string"},
            },
        },
        "required": ["statement", "intended_minutes", "tasks"],
        "additionalProperties": False,
    },
}

MAX_CLARIFICATIONS = 2  # after this many model questions, force a proposal
MAX_TURNS = 8  # hard stop: a runaway conversation must not trap the user


def _make_client() -> anthropic.Anthropic:
    key = ""
    if config.KEY_PATH.exists():
        key = config.KEY_PATH.read_text().strip()
    if not key:
        key = os.environ.get("ANTHROPIC_API_KEY", "")
    if not key:
        raise ProviderUnavailable(f"no API key at {config.KEY_PATH}")
    return anthropic.Anthropic(
        api_key=key,
        timeout=anthropic.Timeout(config.REQUEST_TIMEOUT, connect=config.CONNECT_TIMEOUT),
        max_retries=0,
    )


class ClaudeProvider:
    def __init__(self, client: anthropic.Anthropic | None = None):
        self._client = client if client is not None else _make_client()
        self._messages: list[dict] = []
        self._questions_asked = 0
        self._last_tool_use_id: str | None = None

    def elicit(self, previous_gap_summary: str | None) -> Draft:
        answer = ""
        while not answer:
            answer = ui.ask("What do you want to get done this session?\n> ")
        if previous_gap_summary:
            answer = f"(Yesterday's gap: {previous_gap_summary})\n{answer}"
        self._messages.append({"role": "user", "content": answer})
        return self._advance()

    def revise(self, feedback: str) -> Draft:
        self._messages.append(
            {
                "role": "user",
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": self._last_tool_use_id,
                        "content": f"The user wants changes: {feedback}",
                    }
                ],
            }
        )
        # A rejection is not an invitation to interrogate — re-propose directly.
        return self._advance(force_proposal=True)

    def _advance(self, force_proposal: bool = False) -> Draft:
        for turn in range(MAX_TURNS):
            force = force_proposal or self._questions_asked >= MAX_CLARIFICATIONS
            response = self._request(force)

            if response.stop_reason == "tool_use":
                return self._accept_proposal(response)

            if response.stop_reason == "end_turn":
                question = "".join(
                    b.text for b in response.content if b.type == "text"
                ).strip()
                if not question:
                    raise ProviderUnavailable("model returned an empty reply")
                self._messages.append({"role": "assistant", "content": response.content})
                self._questions_asked += 1
                reply = ""
                while not reply:
                    reply = ui.ask(f"{question}\n> ")
                self._messages.append({"role": "user", "content": reply})
                continue

            raise ProviderUnavailable(f"unexpected stop_reason: {response.stop_reason}")

        raise ProviderUnavailable("conversation did not converge")

    def _request(self, force_proposal: bool):
        kwargs = {}
        if force_proposal:
            kwargs["tool_choice"] = {"type": "tool", "name": "propose_tasks"}
        try:
            return self._client.messages.create(
                model=config.MODEL,
                max_tokens=1024,
                output_config={"effort": "low"},
                system=SYSTEM,
                tools=[PROPOSE_TASKS],
                messages=self._messages,
                **kwargs,
            )
        except anthropic.APIConnectionError as exc:  # includes APITimeoutError
            raise ProviderUnavailable("offline or model unreachable") from exc
        except anthropic.AuthenticationError as exc:
            raise ProviderUnavailable("API key rejected") from exc
        except anthropic.APIError as exc:
            raise ProviderUnavailable(f"API error: {exc.__class__.__name__}") from exc

    def _accept_proposal(self, response) -> Draft:
        block = next(b for b in response.content if b.type == "tool_use")
        self._messages.append({"role": "assistant", "content": response.content})
        self._last_tool_use_id = block.id
        data = block.input  # strict: True — already validated against the schema
        tasks = [t.strip() for t in data["tasks"] if t.strip()]
        if not tasks:
            raise ProviderUnavailable("model proposed an empty task list")
        return Draft(
            statement=data["statement"].strip(),
            intended_minutes=data["intended_minutes"],
            tasks=tasks,
        )
