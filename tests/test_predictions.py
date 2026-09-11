from typing import Any

from app.agents.reflection import BeliefRevision, NewPrediction, Reflection, Verification
from app.services import Services
from app.workers.run import cycle
from tests.fake_llm import FakeLLM
from tests.helpers import belief, event


def reflection(**kw: Any) -> Reflection:
    base: dict[str, Any] = dict(
        inconsistencies=[], patterns=[], open_questions=[], predictions=[], verifications=[]
    )
    return Reflection(**{**base, **kw})


async def test_predictions_recorded_and_verified(fresh: Services, llm: FakeLLM) -> None:
    await fresh.events.append("user_message", "user", {"text": "deploy is tomorrow"})
    llm.queue(
        reflection(
            predictions=[
                NewPrediction(
                    prediction="deploy will slip", probability=0.6, days_until=2, evidence_ids=[]
                )
            ]
        )
    )
    stats = await cycle(fresh, force_reflect=True)
    assert stats["snapshot"] == 1
    (p,) = await fresh.repo.list_rows("predictions")
    assert p["verified"] is None and p["deadline"] is not None

    await fresh.pool.execute("UPDATE predictions SET deadline = now() - interval '1 day'")
    await fresh.events.append("user_message", "user", {"text": "deploy slipped a week"})
    llm.queue(
        reflection(
            verifications=[Verification(prediction_id=p["id"], verified=True, evidence_ids=[])]
        )
    )
    await cycle(fresh, force_reflect=True)
    p2 = await fresh.repo.get("predictions", p["id"])
    assert p2 and p2["verified"] is True and p2["verified_at"] and p2["version"] == 2
    assert len(await fresh.repo.snapshots()) == 2


async def test_reflection_cites_specific_evidence(fresh: Services, llm: FakeLLM) -> None:
    e1 = await event(fresh, "I love tea")
    e2 = await event(fresh, "actually I switched to coffee")
    await event(fresh, "unrelated chatter")
    bid = await belief(fresh, "User prefers tea", e1["event_id"], 0.7)
    llm.queue(
        reflection(
            inconsistencies=[
                BeliefRevision(
                    belief_id=bid,
                    confidence=0.2,
                    status="uncertain",
                    reason="switched to coffee",
                    evidence_ids=[e2["event_id"]],
                )
            ],
            predictions=[
                NewPrediction(
                    prediction="user will order coffee",
                    probability=0.6,
                    days_until=3,
                    evidence_ids=[e2["event_id"]],
                )
            ],
        )
    )
    await cycle(fresh, force_reflect=True)
    history = await fresh.repo.transitions_for(bid)
    assert history[-1]["evidence"] == [e2["event_id"]]
    (p,) = await fresh.repo.list_rows("predictions")
    assert p["evidence"] == [e2["event_id"]]
