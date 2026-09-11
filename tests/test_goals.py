from app.agents.goals import GoalChange, GoalReview, NewGoal
from app.services import Services
from app.workers.run import cycle
from tests.fake_llm import FakeLLM
from tests.helpers import commit, event


async def test_goals_lifecycle_and_conflicts(fresh: Services, llm: FakeLLM) -> None:
    e = await event(fresh, "plan a trip")
    gid = await commit(
        fresh,
        "interaction",
        "create_goal",
        {"description": "plan a trip", "origin": "user"},
        evidence=[e["event_id"]],
    )
    for _ in range(3):
        await cycle(fresh)  # nothing happens without an LLM opinion
    g = await fresh.repo.get("goals", gid)
    assert g and g["status"] == "active"

    await event(fresh, "trip is booked, thanks. also I want to save money")
    llm.queue(
        GoalReview(
            changes=[GoalChange(goal_id=gid, status="completed", reason="booked")],
            inferred_goals=[NewGoal(description="reduce spending", priority=0.4)],
            conflicts=["saving money conflicts with expensive trip"],
        )
    )
    await cycle(fresh)
    assert gid not in {g["id"] for g in await fresh.repo.list_rows("goals", "active")}
    (inferred,) = await fresh.repo.list_rows("goals", "proposed")
    assert inferred["origin"] == "inferred"
    ws = (await fresh.repo.singleton("working_state"))["data"]
    assert ws["open_questions"] == ["saving money conflicts with expensive trip"]
