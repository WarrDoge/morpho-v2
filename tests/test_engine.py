import asyncpg
import pytest

from app.agents import interaction
from app.agents.interaction import Interpretation
from app.services import Services
from app.state.models import Proposal
from tests.fake_llm import FakeLLM
from tests.helpers import belief, commit, event, memory


async def test_create_records_proposal_and_transition(fresh: Services) -> None:
    e = await event(fresh, "hello")
    mid = await memory(fresh, "user said hello", e["event_id"])
    row = await fresh.repo.get("memories", mid)
    assert row and row["source_events"] == [e["event_id"]] and row["valid_from"]
    (t,) = await fresh.repo.transitions_for(mid)
    assert t["before"] is None and t["after"]["id"] == mid and t["decision"] == "accepted"
    assert t["event_id"] == e["event_id"]


async def test_rejections_are_recorded(fresh: Services) -> None:
    r = await fresh.engine.commit_one(
        Proposal(agent="memory", operation="create_memory", payload={"summary": "x"})
    )
    assert not r.accepted and r.reason == "evidence required"
    r = await fresh.engine.commit_one(
        Proposal(
            agent="memory",
            operation="create_memory",
            payload={"summary": "x", "importance": 2},
            evidence=["evt_1"],
        )
    )
    assert not r.accepted and "importance" in (r.reason or "")
    rows = await fresh.pool.fetch("SELECT decision FROM agent_proposals")
    assert {r["decision"] for r in rows} == {"rejected"} and len(rows) == 2
    assert await fresh.pool.fetchval("SELECT count(*) FROM state_transitions") == 0


async def test_self_model_cannot_grant_capabilities(fresh: Services) -> None:
    p = Proposal(
        agent="self_model",
        operation="update_self_state",
        payload={"patch": {"capabilities": ["root"]}},
    )
    r = await fresh.engine.commit_one(p)
    assert not r.accepted and "capabilities" in (r.reason or "")
    r = await fresh.engine.commit_one(p.model_copy(update={"agent": "harness"}))
    assert r.accepted
    assert (await fresh.repo.singleton("self_state"))["data"]["capabilities"] == ["root"]


async def test_optimistic_concurrency(fresh: Services) -> None:
    e = await event(fresh, "x")
    mid = await memory(fresh, "m", e["event_id"])
    ok = await fresh.engine.commit_one(
        Proposal(
            agent="memory",
            operation="update_memory",
            target=mid,
            payload={"importance": 0.9, "expected_version": 1},
        )
    )
    stale = await fresh.engine.commit_one(
        Proposal(
            agent="memory",
            operation="update_memory",
            target=mid,
            payload={"importance": 0.1, "expected_version": 1},
        )
    )
    assert ok.accepted and not stale.accepted and "version conflict" in (stale.reason or "")
    row = await fresh.repo.get("memories", mid)
    assert row and row["importance"] == 0.9 and row["version"] == 2


async def test_inferred_goal_origin_forced(fresh: Services) -> None:
    e = await event(fresh, "x")
    gid = await commit(
        fresh,
        "goals",
        "create_goal",
        {"description": "g", "origin": "user"},
        evidence=[e["event_id"]],
    )
    row = await fresh.repo.get("goals", gid)
    assert row and row["origin"] == "inferred"
    gid = await commit(
        fresh,
        "interaction",
        "create_goal",
        {"description": "g2", "origin": "user"},
        evidence=[e["event_id"]],
    )
    row = await fresh.repo.get("goals", gid)
    assert row and row["origin"] == "user"


async def test_events_are_immutable(fresh: Services) -> None:
    e = await event(fresh, "x")
    with pytest.raises(asyncpg.PostgresError):
        await fresh.pool.execute("UPDATE events SET payload = '{}' WHERE id = $1", e["id"])
    with pytest.raises(asyncpg.PostgresError):
        await fresh.pool.execute("DELETE FROM events WHERE id = $1", e["id"])


async def test_concurrent_working_state_writes_do_not_lose_updates(
    fresh: Services, llm: FakeLLM
) -> None:
    e = await event(fresh, "let's plan the Berlin move")
    llm.queue(
        Interpretation(
            current_topic="Berlin move",
            current_task=None,
            entities=[],
            new_explicit_requests=[],
            open_questions=[],
            constraints=[],
            importance=0.5,
        )
    )
    proposals = await interaction.run(fresh, e)  # computed against version 1
    await commit(
        fresh,
        "goals",
        "set_working_state",
        {"patch": {"open_questions": ["q"]}},
        evidence=[e["event_id"]],
    )
    await fresh.engine.commit(proposals, e["event_id"])
    data = (await fresh.repo.singleton("working_state"))["data"]
    assert data["current_topic"] == "Berlin move" and data["open_questions"] == ["q"]

    strict = Proposal(
        agent="goals",
        operation="set_working_state",
        payload={"patch": {"current_task": "x"}, "expected_version": 3},
    )
    assert (await fresh.engine.commit_one(strict)).accepted
    stale = await fresh.engine.commit_one(strict)
    assert not stale.accepted and "version conflict" in (stale.reason or "")


async def test_duplicate_create_is_folded_into_existing(fresh: Services) -> None:
    e1, e2 = await event(fresh, "a"), await event(fresh, "b")
    bid = await belief(fresh, "User prefers tea", e1["event_id"], 0.5)
    r = await fresh.engine.commit_one(
        Proposal(
            agent="beliefs",
            operation="create_belief",
            payload={"proposition": "User prefers tea", "confidence": 0.7},
            evidence=[e2["event_id"]],
        )
    )
    assert r.accepted and r.object_ids == [bid] and "folded" in (r.reason or "")
    b = await fresh.repo.get("beliefs", bid)
    assert b and b["confidence"] == 0.7
    assert set(b["supporting_evidence"]) == {e1["event_id"], e2["event_id"]}
    assert len(await fresh.repo.list_rows("beliefs")) == 1

    mid = await memory(fresh, "User lives in Berlin", e1["event_id"])
    r = await fresh.engine.commit_one(
        Proposal(
            agent="reflection",
            operation="create_memory",
            payload={"summary": "User lives in Berlin"},
            evidence=[e2["event_id"]],
        )
    )
    assert r.accepted and r.object_ids == [mid]
    m = await fresh.repo.get("memories", mid)
    assert m and m["access_count"] == 1 and e2["event_id"] in m["source_events"]
    assert len(await fresh.repo.list_rows("memories")) == 1

    gid = await commit(
        fresh,
        "interaction",
        "create_goal",
        {"description": "Plan the move"},
        evidence=[e1["event_id"]],
    )
    r = await fresh.engine.commit_one(
        Proposal(
            agent="goals",
            operation="create_goal",
            payload={"description": "plan the move"},
            evidence=[e2["event_id"]],
        )
    )
    assert not r.accepted and gid in (r.reason or "")
    rows = await fresh.pool.fetch("SELECT reason FROM agent_proposals WHERE decision = 'accepted'")
    assert sum("folded" in (x["reason"] or "") for x in rows) == 2
