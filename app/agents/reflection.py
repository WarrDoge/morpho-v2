"""Periodic inspection: inconsistencies, patterns, open questions, predictions (§9.7, §19, §21)."""

import json
from datetime import UTC, datetime, timedelta

from app.agents.base import Strict, clamp, fmt_events, fmt_rows, merge_questions
from app.services import Services
from app.state.models import LIVE_BELIEF, BeliefStatus, Proposal

SYSTEM = """You are the reflection agent of a persistent-state assistant.
Inspect the accumulated state. Report: beliefs that are inconsistent with each other or with
memories (with a revised confidence/status); recurring patterns worth storing as semantic memories
(cite memory or event ids as evidence); unresolved questions; a few concrete falsifiable
predictions with a horizon in days; and verdicts for predictions whose deadline has passed
(verified = true if the prediction came true according to the evidence). Cite the specific
event or memory ids each item rests on. Every statement must be about the user or the world in
plain language; never write statements about memories, beliefs, ids, or the state itself.
Be conservative. Return only the JSON object."""


class BeliefRevision(Strict):
    belief_id: str
    confidence: float
    status: BeliefStatus
    reason: str
    evidence_ids: list[str]


class Pattern(Strict):
    statement: str
    evidence_ids: list[str]
    confidence: float


class NewPrediction(Strict):
    prediction: str
    probability: float
    days_until: int
    evidence_ids: list[str]


class Verification(Strict):
    prediction_id: str
    verified: bool
    evidence_ids: list[str]


class Reflection(Strict):
    inconsistencies: list[BeliefRevision]
    patterns: list[Pattern]
    open_questions: list[str]
    predictions: list[NewPrediction]
    verifications: list[Verification]


async def run(svc: Services) -> list[Proposal]:
    now = datetime.now(UTC)
    repo = svc.repo
    beliefs = await repo.list_rows("beliefs", LIVE_BELIEF, 20)
    memories = await repo.list_rows("memories", ["active", "reinforced", "consolidated"], 30)
    goals = await repo.list_rows("goals", ["active", "blocked"], 20)
    recent = await svc.events.recent(10)
    self_data = (await repo.singleton("self_state"))["data"]
    working = (await repo.singleton("working_state"))["data"]
    due = [
        p
        for p in await repo.list_rows("predictions", limit=50)
        if p["verified"] is None and p["deadline"] and p["deadline"] <= now
    ]
    if not (beliefs or memories or recent):
        return []
    user = (
        f"BELIEFS:\n{fmt_rows(beliefs, 'proposition', 'confidence', 'status')}\n\n"
        f"MEMORIES:\n{fmt_rows(memories, 'kind', 'summary')}\n\n"
        f"GOALS:\n{fmt_rows(goals, 'status', 'description')}\n\n"
        f"PREDICTIONS PAST DEADLINE:\n{fmt_rows(due, 'prediction', 'probability')}\n\n"
        f"SELF MODEL:\n{json.dumps(self_data)}\n\nWORKING STATE:\n{json.dumps(working)}\n\n"
        f"RECENT EVENTS:\n{fmt_events(recent)}"
    )
    out = await svc.llm.complete_json(SYSTEM, user, Reflection)
    evidence = [e["event_id"] for e in recent] or ["reflection"]
    known_b = {b["id"] for b in beliefs}
    proposals = [
        Proposal(
            agent="reflection",
            operation="update_belief",
            target=r.belief_id,
            payload={"confidence": clamp(r.confidence), "status": r.status},
            evidence=r.evidence_ids or evidence,
            confidence=clamp(r.confidence),
        )
        for r in out.inconsistencies
        if r.belief_id in known_b
    ]
    proposals += [
        Proposal(
            agent="reflection",
            operation="create_memory",
            payload={
                "kind": "semantic",
                "summary": p.statement,
                "importance": 0.6,
                "confidence": clamp(p.confidence),
                "status": "active",
            },
            evidence=p.evidence_ids or evidence,
            confidence=clamp(p.confidence),
        )
        for p in out.patterns
    ]
    proposals += [
        Proposal(
            agent="reflection",
            operation="create_prediction",
            payload={
                "prediction": p.prediction,
                "probability": clamp(p.probability),
                "deadline": (now + timedelta(days=max(1, p.days_until))).isoformat(),
            },
            evidence=p.evidence_ids or evidence,
            confidence=clamp(p.probability),
        )
        for p in out.predictions
    ]
    known_p = {p["id"] for p in due}
    proposals += [
        Proposal(
            agent="reflection",
            operation="verify_prediction",
            target=v.prediction_id,
            payload={"verified": v.verified},
            evidence=v.evidence_ids or evidence,
            confidence=0.7,
        )
        for v in out.verifications
        if v.prediction_id in known_p
    ]
    if out.open_questions:
        proposals.append(
            Proposal(
                agent="reflection",
                operation="set_working_state",
                payload={"patch": {"open_questions": merge_questions(working, out.open_questions)}},
                evidence=evidence,
                confidence=0.6,
            )
        )
    return proposals
