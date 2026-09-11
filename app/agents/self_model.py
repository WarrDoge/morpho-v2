"""Maintains the operational self model (§7, §9.5). Cannot grant capabilities (§23)."""

import json

from app.agents.base import MESSAGE_TYPES, Strict, fmt_events
from app.events.store import Event
from app.services import Services
from app.state.models import Proposal

SYSTEM = """You are the self-model agent of a persistent-state assistant.
Given the current self model and recent events, return the updated self model fields.
Keep each list short (at most 8 items), most recent or most relevant first. Record commitments the
assistant made, actions it took, failures, limitations it hit, and things it is uncertain about.
You cannot add capabilities or permissions. Return only the JSON object."""


class SelfPatch(Strict):
    limitations: list[str]
    commitments: list[str]
    recent_actions: list[str]
    known_failures: list[str]
    uncertainties: list[str]
    current_objectives: list[str]


async def run(svc: Services, events: list[Event]) -> list[Proposal]:
    events = [e for e in events if e["type"] in MESSAGE_TYPES]
    if not events:
        return []
    self_data = (await svc.repo.singleton("self_state"))["data"]
    user = f"SELF MODEL:\n{json.dumps(self_data)}\n\nEVENTS:\n{fmt_events(events)}"
    out = await svc.llm.complete_json(SYSTEM, user, SelfPatch)
    patch = {k: v[:8] for k, v in out.model_dump().items()}
    if all(patch[k] == self_data.get(k) for k in patch):
        return []
    return [
        Proposal(
            agent="self_model",
            operation="update_self_state",
            payload={"patch": patch},
            evidence=[e["event_id"] for e in events],
            confidence=0.7,
        )
    ]
