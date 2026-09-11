import asyncio

from app.agents.memory import MemoryDecision, NewMemory
from app.services import Services
from app.workers.run import cycle
from tests.fake_llm import FakeLLM


def decision(summary: str) -> MemoryDecision:
    return MemoryDecision(
        new_memories=[NewMemory(summary=summary, importance=0.8, confidence=0.9, entities=[])],
        reinforce=[],
    )


async def test_failed_batch_is_retried(fresh: Services, llm: FakeLLM) -> None:
    e = await fresh.events.append("user_message", "user", {"text": "I moved to Berlin"})
    llm.errors.append(RuntimeError("provider down"))
    await cycle(fresh)
    assert await fresh.repo.cursor("memory") == 0
    assert await fresh.repo.list_rows("memories") == []
    llm.queue(decision("User moved to Berlin"))
    await cycle(fresh)
    (m,) = await fresh.repo.list_rows("memories")
    assert m["source_events"] == [e["event_id"]]
    assert await fresh.repo.cursor("memory") == e["id"]


async def test_poison_batch_skipped_after_max_failures(fresh: Services, llm: FakeLLM) -> None:
    e = await fresh.events.append("user_message", "user", {"text": "x"})
    llm.errors.extend(RuntimeError("boom") for _ in range(20))
    for _ in range(2):
        await cycle(fresh)
        assert await fresh.repo.cursor("memory") == 0
    await cycle(fresh)
    assert await fresh.repo.cursor("memory") == e["id"]
    llm.errors.clear()
    await fresh.events.append("user_message", "user", {"text": "y"})
    llm.queue(decision("y happened"))
    await cycle(fresh)
    assert len(await fresh.repo.list_rows("memories")) == 1


async def test_concurrent_cycles_process_each_event_once(fresh: Services, llm: FakeLLM) -> None:
    await fresh.events.append("user_message", "user", {"text": "I moved to Berlin"})
    llm.queue(decision("User moved to Berlin"), decision("User moved to Berlin"))
    await asyncio.gather(cycle(fresh), cycle(fresh))
    n = await fresh.pool.fetchval("SELECT count(*) FROM agent_proposals WHERE agent = 'memory'")
    assert n == 1
