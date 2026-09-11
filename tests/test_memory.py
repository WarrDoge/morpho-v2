from app.agents import consolidation
from app.agents.consolidation import Consolidation, Merge
from app.agents.memory import MemoryDecision, NewMemory, Reinforce
from app.services import Services
from app.workers.run import cycle
from tests.fake_llm import FakeLLM
from tests.helpers import event, memory


async def test_important_information_persists(fresh: Services, llm: FakeLLM) -> None:
    e = await event(fresh, "I moved to Berlin last week")
    llm.queue(
        MemoryDecision(
            new_memories=[
                NewMemory(
                    summary="User moved to Berlin", importance=0.8, confidence=0.9, entities=[]
                )
            ],
            reinforce=[],
        )
    )
    await cycle(fresh)
    (m,) = await fresh.repo.list_rows("memories")
    assert m["status"] == "active" and m["source_events"] == [e["event_id"]]
    assert await fresh.repo.cursor("memory") == e["id"]


async def test_reinforcement(fresh: Services, llm: FakeLLM) -> None:
    e = await event(fresh, "I live in Berlin")
    mid = await memory(fresh, "User lives in Berlin", e["event_id"])
    await cycle(fresh)
    e2 = await event(fresh, "Berlin is my home now")
    llm.queue(MemoryDecision(new_memories=[], reinforce=[Reinforce(memory_id=mid)]))
    await cycle(fresh)
    m = await fresh.repo.get("memories", mid)
    assert m and m["access_count"] == 1 and m["status"] == "reinforced"
    assert set(m["source_events"]) == {e["event_id"], e2["event_id"]}


async def test_irrelevant_information_decays(fresh: Services) -> None:
    e = await event(fresh, "weather is fine")
    mid = await memory(fresh, "weather was fine", e["event_id"], importance=0.2)
    await fresh.pool.execute(
        "UPDATE memories SET created_at = now() - interval '400 days' WHERE id = $1", mid
    )
    await fresh.engine.commit(await consolidation.run(fresh))
    m = await fresh.repo.get("memories", mid)
    assert m and m["status"] == "archived" and m["valid_until"]
    assert await fresh.events.get(e["event_id"])  # source event never deleted


async def test_duplicates_merge_with_provenance(fresh: Services, llm: FakeLLM) -> None:
    e1 = await event(fresh, "I like espresso")
    e2 = await event(fresh, "espresso is my favourite")
    a = await memory(fresh, "User likes espresso", e1["event_id"])
    b = await memory(fresh, "User's favourite coffee is espresso", e2["event_id"])
    llm.queue(
        Consolidation(
            merges=[
                Merge(source_ids=[a, b], summary="User strongly prefers espresso", importance=0.7)
            ],
            generalizations=[],
            contradictions=[],
        )
    )
    await fresh.engine.commit(await consolidation.run(fresh))
    (merged,) = await fresh.repo.list_rows("memories", "consolidated")
    assert set(merged["source_events"]) == {e1["event_id"], e2["event_id"]}
    assert set(merged["evidence"]) >= {a, b}
    for old in (a, b):
        row = await fresh.repo.get("memories", old)
        assert row and row["status"] == "deprecated" and merged["id"] in row["evidence"]
    history = await fresh.repo.transitions_for(a)
    assert [h["operation"] for h in history] == ["create_memory", "merge_memories"]


async def test_semantic_memories_do_not_decay(fresh: Services) -> None:
    e = await event(fresh, "seed")
    mid = await memory(
        fresh, "User tends to work late", e["event_id"], kind="semantic", importance=0.2
    )
    await fresh.pool.execute(
        "UPDATE memories SET created_at = now() - interval '400 days' WHERE id = $1", mid
    )
    await fresh.engine.commit(await consolidation.run(fresh))
    m = await fresh.repo.get("memories", mid)
    assert m and m["status"] == "active"


async def test_consolidated_memories_are_candidates_again(fresh: Services, llm: FakeLLM) -> None:
    e = await event(fresh, "seed")
    a = await memory(fresh, "User likes espresso", e["event_id"])
    b = await memory(fresh, "User's favourite coffee is espresso", e["event_id"])
    llm.queue(
        Consolidation(
            merges=[Merge(source_ids=[a, b], summary="User prefers espresso", importance=0.7)],
            generalizations=[],
            contradictions=[],
        )
    )
    await fresh.engine.commit(await consolidation.run(fresh))
    (merged,) = await fresh.repo.list_rows("memories", "consolidated")
    await memory(fresh, "User drinks espresso every morning", e["event_id"])
    await consolidation.run(fresh)
    assert merged["id"] in llm.calls[-1][1]
