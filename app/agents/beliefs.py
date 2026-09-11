"""Creates hypotheses and revises belief confidence from evidence (§9.4, §16)."""

from typing import Literal

from app.agents.base import Strict, clamp, event_text, fmt_rows, per_message
from app.events.store import Event
from app.services import Services
from app.state.models import LIVE_BELIEF, BeliefStatus, Proposal

SYSTEM = """You are the belief agent of a persistent-state assistant.
Beliefs are propositions about the user or the world held with explicit uncertainty.
Given one event and similar existing beliefs: mark which beliefs this event supports or contradicts
(with a revised confidence and status), and propose new hypotheses only for genuinely new,
generalizable propositions. Contradictory beliefs may coexist; do not force resolution.
Statuses: hypothesis, active, uncertain, contradicted, deprecated. Return only the JSON object."""


class NewBelief(Strict):
    proposition: str
    confidence: float


class BeliefUpdate(Strict):
    belief_id: str
    relation: Literal["supports", "contradicts"]
    new_confidence: float
    new_status: BeliefStatus


class BeliefDecision(Strict):
    new_beliefs: list[NewBelief]
    updates: list[BeliefUpdate]


async def run(svc: Services, events: list[Event]) -> list[Proposal]:
    return await per_message(svc, events, _one)


async def _one(svc: Services, e: Event) -> list[Proposal]:
    proposals: list[Proposal] = []
    text = event_text(e)
    (emb,) = await svc.llm.embed([text])
    similar = await svc.repo.similar("beliefs", emb, 5, LIVE_BELIEF)
    user = (
        f"EVENT:\n[{e['event_id']}] {e['source']}: {text}\n\nSIMILAR BELIEFS:\n"
        f"{fmt_rows(similar, 'proposition', 'confidence', 'status')}"
    )
    out = await svc.llm.complete_json(SYSTEM, user, BeliefDecision)
    known = {b["id"] for b in similar}
    for u in out.updates:
        if u.belief_id not in known:
            continue
        key = "add_supporting" if u.relation == "supports" else "add_contradicting"
        proposals.append(
            Proposal(
                agent="beliefs",
                operation="update_belief",
                target=u.belief_id,
                payload={
                    "confidence": clamp(u.new_confidence),
                    "status": u.new_status,
                    key: [e["event_id"]],
                },
                evidence=[e["event_id"]],
                confidence=clamp(u.new_confidence),
            )
        )
    proposals += [
        Proposal(
            agent="beliefs",
            operation="create_belief",
            payload={
                "proposition": nb.proposition,
                "confidence": clamp(nb.confidence),
                "status": "hypothesis",
            },
            evidence=[e["event_id"]],
            confidence=clamp(nb.confidence),
        )
        for nb in out.new_beliefs
    ]
    return proposals
