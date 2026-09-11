"""Forms episodic memories and reinforces existing ones (§9.2, §14)."""

from app.agents.base import Strict, clamp, event_text, fmt_rows, per_message
from app.events.store import Event
from app.services import Services
from app.state.models import LIVE_MEMORY, Proposal

SYSTEM = """You are the memory agent of a persistent-state assistant.
Given one event and the most similar existing memories, decide whether anything is worth
remembering.
Most small talk is not. Prefer reinforcing an existing memory over creating a near-duplicate.
Each new memory is one self-contained sentence stating what happened or what was learned.
Return only the JSON object."""


class NewMemory(Strict):
    summary: str
    importance: float
    confidence: float
    entities: list[str]


class Reinforce(Strict):
    memory_id: str


class MemoryDecision(Strict):
    new_memories: list[NewMemory]
    reinforce: list[Reinforce]


async def run(svc: Services, events: list[Event]) -> list[Proposal]:
    return await per_message(svc, events, _one)


async def _one(svc: Services, e: Event) -> list[Proposal]:
    proposals: list[Proposal] = []
    text = event_text(e)
    (emb,) = await svc.llm.embed([text])
    similar = await svc.repo.similar("memories", emb, 5, LIVE_MEMORY)
    user = (
        f"EVENT:\n[{e['event_id']}] {e['source']}: {text}\n\nSIMILAR MEMORIES:\n"
        f"{fmt_rows(similar, 'summary', 'importance')}"
    )
    out = await svc.llm.complete_json(SYSTEM, user, MemoryDecision)
    known = {m["id"] for m in similar}
    for r in out.reinforce:
        if r.memory_id in known:
            proposals.append(
                Proposal(
                    agent="memory",
                    operation="update_memory",
                    target=r.memory_id,
                    payload={"reinforce": True, "add_source_events": [e["event_id"]]},
                    evidence=[e["event_id"]],
                    confidence=0.8,
                )
            )
    for nm in out.new_memories:
        ents = [x for n in nm.entities if (x := await svc.repo.find_entity(n))]
        imp = clamp(nm.importance)
        proposals.append(
            Proposal(
                agent="memory",
                operation="create_memory",
                payload={
                    "kind": "episodic",
                    "summary": nm.summary,
                    "importance": imp,
                    "confidence": clamp(nm.confidence),
                    "status": "active" if imp >= 0.3 else "candidate",
                    "source_events": [e["event_id"]],
                    "entity_ids": [x["id"] for x in ents],
                },
                evidence=[e["event_id"]],
                confidence=clamp(nm.confidence),
            )
        )
    return proposals
