"""Merges, generalizes, detects contradictions, decays (§9.3, §14). Runs periodically."""

from datetime import UTC, datetime

from app.agents.base import Strict, clamp, fmt_rows
from app.config import settings
from app.context.ranking import decayed_importance
from app.services import Services
from app.state.models import LIVE_MEMORY, Proposal, union

SYSTEM = """You are the consolidation agent of a persistent-state assistant.
Given memories (episodic and semantic): (1) merge groups that describe the same fact or episode
into one memory, (2) generalize repeated observations into semantic statements with provenance,
(3) flag pairs that contradict each other. Only merge true redundancy; keep distinct episodes
distinct. Use only the given memory ids. Merged and generalized summaries describe the user or
the world in plain language; never describe memories, ids, or this process itself.
Return only the JSON object."""


class Merge(Strict):
    source_ids: list[str]
    summary: str
    importance: float


class Generalization(Strict):
    statement: str
    source_ids: list[str]
    confidence: float


class Contradiction(Strict):
    memory_ids: list[str]
    description: str


class Consolidation(Strict):
    merges: list[Merge]
    generalizations: list[Generalization]
    contradictions: list[Contradiction]


def decay_proposals(memories: list[dict], now: datetime) -> list[Proposal]:
    """Deterministic, code-only decay: archive unreinforced memories below the importance floor."""
    return [
        Proposal(
            agent="consolidation",
            operation="update_memory",
            target=m["id"],
            payload={"status": "archived", "expected_version": m["version"]},
            evidence=["decay"],
            confidence=1.0,
        )
        for m in memories
        if m["kind"] == "episodic"
        and m["access_count"] == 0
        and decayed_importance(m, now, settings.memory_half_life_days)
        < settings.memory_archive_floor
    ]


async def run(svc: Services) -> list[Proposal]:
    now = datetime.now(UTC)
    live = await svc.repo.list_rows("memories", LIVE_MEMORY, 200)
    proposals = decay_proposals(live, now)
    archived = {p.target for p in proposals}
    candidates = [m for m in live if m["id"] not in archived][:30]
    if len(candidates) < 2:
        return proposals
    by_id = {m["id"]: m for m in candidates}
    out = await svc.llm.complete_json(
        SYSTEM,
        "MEMORIES:\n" + fmt_rows(candidates, "kind", "summary", "importance", "created_at"),
        Consolidation,
    )
    used: set[str] = set()
    for mg in out.merges:
        ids = [i for i in dict.fromkeys(mg.source_ids) if i in by_id and i not in used]
        if len(ids) < 2:
            continue
        used.update(ids)
        proposals.append(
            Proposal(
                agent="consolidation",
                operation="merge_memories",
                payload={
                    "source_ids": ids,
                    "summary": mg.summary,
                    "kind": by_id[ids[0]]["kind"],
                    "importance": clamp(mg.importance),
                },
                evidence=ids,
                confidence=0.8,
            )
        )
    for g in out.generalizations:
        ids = [i for i in dict.fromkeys(g.source_ids) if i in by_id]
        if not ids:
            continue
        proposals.append(
            Proposal(
                agent="consolidation",
                operation="create_memory",
                payload={
                    "kind": "semantic",
                    "summary": g.statement,
                    "importance": max(by_id[i]["importance"] for i in ids),
                    "confidence": clamp(g.confidence),
                    "status": "active",
                    "source_events": union([], [e for i in ids for e in by_id[i]["source_events"]]),
                    "entity_ids": union([], [x for i in ids for x in by_id[i]["entity_ids"]]),
                },
                evidence=ids,
                confidence=clamp(g.confidence),
            )
        )
    for c in out.contradictions:
        ids = [i for i in dict.fromkeys(c.memory_ids) if i in by_id]
        if len(ids) < 2:
            continue
        proposals.append(
            Proposal(
                agent="consolidation",
                operation="create_belief",
                payload={"proposition": c.description, "confidence": 0.5, "status": "uncertain"},
                evidence=ids,
                confidence=0.5,
            )
        )
    return proposals
