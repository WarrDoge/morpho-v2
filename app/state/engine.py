from dataclasses import dataclass, field
from datetime import UTC, datetime
from typing import Any

import asyncpg
from pydantic import ValidationError

from app.config import settings
from app.db import new_id
from app.llm import LLM
from app.state import models as m
from app.state.models import union
from app.state.repository import Repository, Row

Transition = tuple[str, str, Row | None, Row | None]  # table, object_id, before, after
TERMINAL_MEMORY = {"deprecated", "archived"}
TERMINAL_BELIEF = {"contradicted", "deprecated"}
GUARDED_SELF_KEYS = {"capabilities", "permissions"}
SINGLETONS = {"set_working_state": "working_state", "update_self_state": "self_state"}
EMBED_FIELD = {
    "create_memory": "summary",
    "update_memory": "summary",
    "merge_memories": "summary",
    "create_belief": "proposition",
}
FOLD = {"create_memory": ("memories", m.LIVE_MEMORY), "create_belief": ("beliefs", m.LIVE_BELIEF)}


class Rejected(Exception):
    pass


@dataclass
class CommitResult:
    proposal_id: str
    accepted: bool
    reason: str | None = None
    object_ids: list[str] = field(default_factory=list)


def _now() -> datetime:
    return datetime.now(UTC)


def _events(p: m.Proposal) -> list[str]:
    return [e for e in p.evidence if e.startswith("evt_")]


def _embed_text(p: m.Proposal) -> str | None:
    f = EMBED_FIELD.get(p.operation)
    v = p.payload.get(f) if f else None
    return v if isinstance(v, str) and v else None


class StateEngine:
    """Sole writer of derived state. Every mutation = proposal + transition rows."""

    def __init__(self, pool: asyncpg.Pool, llm: LLM) -> None:
        self.pool = pool
        self.llm = llm

    async def commit(
        self, proposals: list[m.Proposal], source_event: str | None = None
    ) -> list[CommitResult]:
        texts = {i: t for i, p in enumerate(proposals) if (t := _embed_text(p))}
        embs = (
            dict(zip(texts, await self.llm.embed(list(texts.values())), strict=True))
            if texts
            else {}
        )
        return [
            await self.commit_one(p, source_event, embs.get(i)) for i, p in enumerate(proposals)
        ]

    async def commit_one(
        self, p: m.Proposal, source_event: str | None = None, emb: list[float] | None = None
    ) -> CommitResult:
        pid = new_id("prop")
        source_event = next(iter(_events(p)), source_event)
        try:
            payload = self._validate(p)
            if emb is None and (text := _embed_text(p)):
                (emb,) = await self.llm.embed([text])
            async with self.pool.acquire() as conn, conn.transaction():
                repo = Repository(conn)
                fold = await self._fold_duplicate(repo, p, payload, emb)
                q, qpayload, reason = fold or (p, payload, None)
                transitions = await self._apply(repo, q, qpayload, None if fold else emb)
                await self._record(conn, pid, p, source_event, "accepted", reason)
                for table, oid, before, after in transitions:
                    await conn.execute(
                        "INSERT INTO state_transitions (proposal_id, table_name, object_id, "
                        "before, after, agent, event_id) VALUES ($1, $2, $3, $4, $5, $6, $7)",
                        pid,
                        table,
                        oid,
                        before,
                        after,
                        p.agent,
                        source_event,
                    )
            return CommitResult(pid, True, reason, [t[1] for t in transitions])
        except (Rejected, ValidationError, asyncpg.PostgresError) as e:
            reason = (
                "; ".join(f"{'.'.join(map(str, err['loc']))}: {err['msg']}" for err in e.errors())
                if isinstance(e, ValidationError)
                else str(e).splitlines()[0]
            )[:500]
            async with self.pool.acquire() as conn:
                await self._record(conn, pid, p, source_event, "rejected", reason)
            return CommitResult(pid, False, reason)

    def _validate(self, p: m.Proposal) -> Any:
        payload = m.PAYLOADS[p.operation].model_validate(p.payload)
        if p.operation.startswith(("create_", "merge_", "upsert_", "add_")) and not p.evidence:
            raise Rejected("evidence required")
        if p.operation in {
            "update_memory",
            "update_belief",
            "update_goal",
            "verify_prediction",
        } and (not p.target):
            raise Rejected("target required")
        if isinstance(payload, m.CreateGoal) and p.agent not in {"interaction", "harness"}:
            payload.origin = "inferred"
        if isinstance(payload, m.UpdateSelfState) and p.agent != "harness":
            if GUARDED_SELF_KEYS & set(payload.patch):
                raise Rejected("self-model may not grant capabilities or permissions")
        return payload

    async def _fold_duplicate(
        self, repo: Repository, p: m.Proposal, payload: Any, emb: list[float] | None
    ) -> tuple[m.Proposal, Any, str] | None:
        """Near-duplicate create_* becomes an update of the existing row (or a rejection)."""
        if p.operation in FOLD and emb:
            table, live = FOLD[p.operation]
            hits = await repo.similar(table, emb, 1, live)
            if hits and hits[0]["relevance"] >= settings.dedupe_threshold:
                hit = hits[0]
                if isinstance(payload, m.CreateMemory):
                    op, fields = (
                        "update_memory",
                        {
                            "reinforce": True,
                            "add_source_events": payload.source_events or _events(p),
                            "add_entity_ids": payload.entity_ids,
                        },
                    )
                else:
                    op, fields = (
                        "update_belief",
                        {
                            "confidence": max(hit["confidence"], payload.confidence),
                            "add_supporting": _events(p),
                        },
                    )
                q = p.model_copy(update={"operation": op, "target": hit["id"], "payload": fields})
                return q, m.PAYLOADS[op].model_validate(fields), f"folded into {hit['id']}"
        if isinstance(payload, m.CreateGoal):
            dup = await repo.db.fetchval(
                "SELECT id FROM goals WHERE lower(description) = lower($1) AND status = ANY($2)",
                payload.description,
                m.LIVE_GOAL,
            )
            if dup:
                raise Rejected(f"duplicate of {dup}")
        return None

    async def _record(
        self,
        conn: asyncpg.Connection,
        pid: str,
        p: m.Proposal,
        source_event: str | None,
        decision: str,
        reason: str | None,
    ) -> None:
        await conn.execute(
            "INSERT INTO agent_proposals (id, agent, operation, target, payload, evidence, "
            "confidence, decision, reason, source_event) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
            pid,
            p.agent,
            p.operation,
            p.target,
            p.payload,
            p.evidence,
            p.confidence,
            decision,
            reason,
            source_event,
        )

    async def _load(self, repo: Repository, table: str, target: str | None, payload: Any) -> Row:
        row = await repo.get(table, target or "")
        if row is None:
            raise Rejected(f"{table} {target} not found")
        return _check_version(row, payload)

    async def _apply(
        self, repo: Repository, p: m.Proposal, payload: Any, emb: list[float] | None
    ) -> list[Transition]:
        now = _now()
        op = p.operation

        if op == "create_memory":
            pl: m.CreateMemory = payload
            row = await repo.insert(
                "memories",
                {
                    "id": new_id("mem"),
                    "kind": pl.kind,
                    "summary": pl.summary,
                    "status": pl.status,
                    "importance": pl.importance,
                    "confidence": pl.confidence,
                    "source_events": pl.source_events or _events(p),
                    "evidence": p.evidence,
                    "entity_ids": pl.entity_ids,
                    "valid_from": now,
                    "embedding": emb,
                },
            )
            return [("memories", row["id"], None, row)]

        if op == "update_memory":
            pu: m.UpdateMemory = payload
            before = await self._load(repo, "memories", p.target, pu)
            fields: Row = {
                k: v
                for k, v in pu.model_dump().items()
                if k in {"summary", "importance", "confidence", "status"} and v is not None
            }
            if pu.reinforce:
                fields["access_count"] = before["access_count"] + 1
                fields["last_reinforced_at"] = now
                fields.setdefault("status", "reinforced")
            if pu.add_source_events:
                fields["source_events"] = union(before["source_events"], pu.add_source_events)
            if pu.add_entity_ids:
                fields["entity_ids"] = union(before["entity_ids"], pu.add_entity_ids)
            if fields.get("status") in TERMINAL_MEMORY:
                fields["valid_until"] = now
            if "summary" in fields:
                fields["embedding"] = emb
            after = await repo.update("memories", before["id"], fields | _bump(before, p, now))
            return [("memories", before["id"], before, after)]

        if op == "merge_memories":
            pm: m.MergeMemories = payload
            sources = [await self._load(repo, "memories", sid, None) for sid in pm.source_ids]
            merged = await repo.insert(
                "memories",
                {
                    "id": new_id("mem"),
                    "kind": pm.kind,
                    "summary": pm.summary,
                    "status": "consolidated",
                    "importance": pm.importance,
                    "confidence": pm.confidence,
                    "source_events": union([], [e for s in sources for e in s["source_events"]]),
                    "evidence": union(p.evidence, pm.source_ids),
                    "entity_ids": union([], [e for s in sources for e in s["entity_ids"]]),
                    "access_count": sum(s["access_count"] for s in sources),
                    "valid_from": now,
                    "embedding": emb,
                },
            )
            out: list[Transition] = [("memories", merged["id"], None, merged)]
            for s in sources:
                after = await repo.update(
                    "memories",
                    s["id"],
                    {
                        "status": "deprecated",
                        "valid_until": now,
                        "updated_at": now,
                        "version": s["version"] + 1,
                        "evidence": union(s["evidence"], [merged["id"]]),
                    },
                )
                out.append(("memories", s["id"], s, after))
            return out

        if op == "create_belief":
            pb: m.CreateBelief = payload
            row = await repo.insert(
                "beliefs",
                {
                    "id": new_id("belief"),
                    "proposition": pb.proposition,
                    "confidence": pb.confidence,
                    "status": pb.status,
                    "supporting_evidence": p.evidence,
                    "last_reviewed": now,
                    "valid_from": now,
                    "embedding": emb,
                },
            )
            return [("beliefs", row["id"], None, row)]

        if op == "update_belief":
            ub: m.UpdateBelief = payload
            before = await self._load(repo, "beliefs", p.target, ub)
            fields = {"last_reviewed": now, "updated_at": now, "version": before["version"] + 1}
            if ub.confidence is not None:
                fields["confidence"] = ub.confidence
            if ub.status is not None:
                fields["status"] = ub.status
                if ub.status in TERMINAL_BELIEF:
                    fields["valid_until"] = now
            if ub.add_supporting:
                fields["supporting_evidence"] = union(
                    before["supporting_evidence"], ub.add_supporting
                )
            if ub.add_contradicting:
                fields["contradicting_evidence"] = union(
                    before["contradicting_evidence"], ub.add_contradicting
                )
            after = await repo.update("beliefs", before["id"], fields)
            return [("beliefs", before["id"], before, after)]

        if op == "create_goal":
            pg: m.CreateGoal = payload
            row = await repo.insert(
                "goals", {"id": new_id("goal"), "evidence": p.evidence, **pg.model_dump()}
            )
            return [("goals", row["id"], None, row)]

        if op == "update_goal":
            ug: m.UpdateGoal = payload
            before = await self._load(repo, "goals", p.target, ug)
            fields = _bump(before, p, now)
            if ug.status is not None:
                fields["status"] = ug.status
            if ug.priority is not None:
                fields["priority"] = ug.priority
            after = await repo.update("goals", before["id"], fields)
            return [("goals", before["id"], before, after)]

        if op == "create_prediction":
            pp: m.CreatePrediction = payload
            row = await repo.insert(
                "predictions", {"id": new_id("pred"), "evidence": p.evidence, **pp.model_dump()}
            )
            return [("predictions", row["id"], None, row)]

        if op == "verify_prediction":
            vp: m.VerifyPrediction = payload
            before = await self._load(repo, "predictions", p.target, vp)
            after = await repo.update(
                "predictions",
                before["id"],
                {
                    "verified": vp.verified,
                    "verified_at": now,
                    "version": before["version"] + 1,
                    "evidence": union(before["evidence"], p.evidence),
                },
            )
            return [("predictions", before["id"], before, after)]

        if op == "upsert_entity":
            ue: m.UpsertEntity = payload
            before = await repo.find_entity(ue.name, ue.kind)
            if before is None:
                after = await repo.insert(
                    "entities",
                    {
                        "id": new_id("ent"),
                        "name": ue.name,
                        "kind": ue.kind,
                        "attributes": ue.attributes,
                        "evidence": p.evidence,
                    },
                )
            else:
                after = await repo.update(
                    "entities",
                    before["id"],
                    {"attributes": {**before["attributes"], **ue.attributes}}
                    | _bump(before, p, now),
                )
            return [("entities", after["id"], before, after)]

        if op == "add_relationship":
            ar: m.AddRelationship = payload
            src = await repo.find_entity(ar.src)
            dst = await repo.find_entity(ar.dst)
            if src is None or dst is None:
                raise Rejected(f"unknown entity: {ar.src if src is None else ar.dst}")
            row = await repo.insert(
                "entity_relationships",
                {
                    "id": new_id("rel"),
                    "src": src["id"],
                    "rel": ar.rel,
                    "dst": dst["id"],
                    "confidence": ar.confidence,
                    "evidence": p.evidence,
                    "valid_from": now,
                },
            )
            return [("entity_relationships", row["id"], None, row)]

        if op in SINGLETONS:
            table = SINGLETONS[op]
            before = _check_version(await repo.singleton(table), payload)
            after = await repo.update(
                table,
                1,
                {
                    "data": {**before["data"], **payload.patch},
                    "updated_at": now,
                    "version": before["version"] + 1,
                },
            )
            return [(table, table, before, after)]

        raise Rejected(f"unknown operation {op}")


def _bump(before: Row, p: m.Proposal, now: datetime) -> Row:
    return {
        "updated_at": now,
        "version": before["version"] + 1,
        "evidence": union(before["evidence"], p.evidence),
    }


def _check_version(row: Row, payload: Any) -> Row:
    expected = getattr(payload, "expected_version", None)
    if expected is not None and row["version"] != expected:
        raise Rejected(f"version conflict: expected {expected}, have {row['version']}")
    return row
