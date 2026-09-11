from typing import Any

from httpx import AsyncClient

from app.agents.interaction import Entity, Interpretation
from app.agents.self_model import SelfPatch
from app.services import Services, build_services
from app.workers.run import cycle
from tests.conftest import TEST_DSN
from tests.fake_llm import FakeLLM


def interp(**kw: Any) -> Interpretation:
    base: dict[str, Any] = dict(
        current_topic="relocation",
        current_task="plan the move",
        entities=[Entity(name="Berlin", kind="place")],
        new_explicit_requests=["make a moving checklist"],
        open_questions=[],
        constraints=["budget 2000 EUR"],
        importance=0.8,
    )
    return Interpretation(**{**base, **kw})


async def test_state_persists_across_sessions(
    client: AsyncClient, fresh: Services, llm: FakeLLM
) -> None:
    llm.queue(interp())
    llm.text.append("Sure, here is a checklist.")
    r = await client.post("/interact", json={"text": "I moved to Berlin, plan my move"})
    assert r.status_code == 200 and r.json()["response"] == "Sure, here is a checklist."

    other = await build_services(TEST_DSN, FakeLLM())  # a brand-new process/session
    try:
        ws = (await other.repo.singleton("working_state"))["data"]
        assert ws["current_topic"] == "relocation" and ws["active_entities"] == ["Berlin"]
        (g,) = await other.repo.list_rows("goals", "active")
        assert g["description"] == "make a moving checklist" and g["origin"] == "user"
        assert await other.repo.find_entity("berlin")
        assert len(await other.events.recent(10)) == 2
    finally:
        await other.close()


async def test_why_explains_provenance(client: AsyncClient, llm: FakeLLM) -> None:
    llm.queue(interp())
    r = await client.post("/interact", json={"text": "plan my move"})
    eid = r.json()["event_id"]
    (g,) = (await client.get("/goals")).json()
    why = (await client.get(f"/why/{g['id']}")).json()
    assert why["events"][0]["event_id"] == eid
    assert why["history"][0]["proposal_agent"] == "interaction"
    assert why["history"][0]["operation"] == "create_goal"
    assert (await client.get("/why/goal_nope")).status_code == 404


async def test_self_model_evolves(fresh: Services, llm: FakeLLM) -> None:
    await fresh.events.append("user_message", "user", {"text": "remind me tomorrow"})
    await fresh.events.append("assistant_message", "assistant", {"text": "I will remind you"})
    llm.queue(
        SelfPatch(
            limitations=["no clock access"],
            commitments=["remind user tomorrow"],
            recent_actions=["promised a reminder"],
            known_failures=[],
            uncertainties=["user's timezone"],
            current_objectives=[],
        )
    )
    await cycle(fresh)
    s = await fresh.repo.singleton("self_state")
    assert s["data"]["commitments"] == ["remind user tomorrow"] and s["version"] == 2
    assert s["data"]["capabilities"] == []
    (t,) = await fresh.repo.transitions_for("self_state")
    assert t["proposal_agent"] == "self_model" and len(t["evidence"]) == 2
