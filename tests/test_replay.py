from app.services import Services
from app.state.snapshots import replay, take_snapshot
from tests.helpers import belief, commit, event, memory


async def test_replay_rebuilds_state_from_transitions(fresh: Services) -> None:
    e = await event(fresh, "x")
    a = await memory(fresh, "a", e["event_id"])
    b = await memory(fresh, "b", e["event_id"])
    await belief(fresh, "p", e["event_id"])
    await commit(
        fresh,
        "consolidation",
        "merge_memories",
        {"source_ids": [a, b], "summary": "ab"},
        evidence=[a, b],
    )
    await commit(fresh, "harness", "update_self_state", {"patch": {"commitments": ["c"]}})
    before = await take_snapshot(fresh)
    n_tr = await fresh.pool.fetchval("SELECT count(*) FROM state_transitions")

    await fresh.pool.execute("DELETE FROM memories; DELETE FROM beliefs")
    assert await replay(fresh) == n_tr
    after = await take_snapshot(fresh)
    assert after["data"]["memories"] == before["data"]["memories"]
    assert after["data"]["beliefs"] == before["data"]["beliefs"]
    assert after["data"]["self_state"] == before["data"]["self_state"]
    assert (await fresh.repo.similar("memories", (await fresh.llm.embed(["ab"]))[0], 1))[0][
        "summary"
    ] == "ab"
