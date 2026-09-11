from typing import Literal

from app.agents.beliefs import BeliefDecision, BeliefUpdate, NewBelief
from app.services import Services
from app.state.models import BeliefStatus
from app.workers.run import cycle
from tests.fake_llm import FakeLLM
from tests.helpers import belief, event


async def _revise(
    fresh: Services,
    llm: FakeLLM,
    bid: str,
    text: str,
    rel: Literal["supports", "contradicts"],
    conf: float,
    status: BeliefStatus,
) -> dict:
    await cycle(fresh)  # consume seed events first
    e = await event(fresh, text)
    llm.queue(
        BeliefDecision(
            new_beliefs=[],
            updates=[
                BeliefUpdate(belief_id=bid, relation=rel, new_confidence=conf, new_status=status)
            ],
        )
    )  # type: ignore[arg-type]
    await cycle(fresh)
    row = await fresh.repo.get("beliefs", bid)
    assert row
    return {**row, "event": e["event_id"]}


async def test_supporting_evidence_increases_confidence(fresh: Services, llm: FakeLLM) -> None:
    e = await event(fresh, "I prefer tea")
    bid = await belief(fresh, "User prefers tea over coffee", e["event_id"], 0.6)
    b = await _revise(fresh, llm, bid, "tea again please", "supports", 0.8, "active")
    assert b["confidence"] == 0.8 and b["event"] in b["supporting_evidence"]


async def test_contradicting_evidence_reduces_confidence(fresh: Services, llm: FakeLLM) -> None:
    e = await event(fresh, "I prefer tea")
    bid = await belief(fresh, "User prefers tea over coffee", e["event_id"], 0.6)
    b = await _revise(fresh, llm, bid, "actually I love coffee", "contradicts", 0.3, "uncertain")
    assert b["confidence"] == 0.3 and b["event"] in b["contradicting_evidence"]
    assert b["event"] not in b["supporting_evidence"]


async def test_obsolete_beliefs_retired(fresh: Services, llm: FakeLLM) -> None:
    e = await event(fresh, "I prefer tea")
    bid = await belief(fresh, "User prefers tea over coffee", e["event_id"], 0.6)
    b = await _revise(fresh, llm, bid, "I quit tea forever", "contradicts", 0.1, "deprecated")
    assert b["status"] == "deprecated" and b["valid_until"] is not None
    assert bid not in {x["id"] for x in await fresh.repo.list_rows("beliefs", "active")}


async def test_new_hypothesis_created(fresh: Services, llm: FakeLLM) -> None:
    e = await event(fresh, "I always work late on Fridays")
    llm.queue(
        BeliefDecision(
            new_beliefs=[NewBelief(proposition="User works late on Fridays", confidence=0.5)],
            updates=[],
        )
    )
    await cycle(fresh)
    (b,) = await fresh.repo.list_rows("beliefs")
    assert b["status"] == "hypothesis" and b["supporting_evidence"] == [e["event_id"]]
