"""Tracks goal lifecycle and conflicts (§8, §9.6). Inferred goals are marked inferred."""

from app.agents.base import MESSAGE_TYPES, Strict, clamp, fmt_events, fmt_rows, merge_questions
from app.events.store import Event
from app.services import Services
from app.state.models import LIVE_GOAL, GoalStatus, Proposal

SYSTEM = """You are the goal agent of a persistent-state assistant.
Given the current goals and recent events: mark goals that are now completed, blocked, abandoned or
superseded; list new goals that are strongly implied but were never explicitly requested (these are
inferred, be conservative); and describe conflicts between goals. Return only the JSON object."""


class GoalChange(Strict):
    goal_id: str
    status: GoalStatus
    reason: str


class NewGoal(Strict):
    description: str
    priority: float


class GoalReview(Strict):
    changes: list[GoalChange]
    inferred_goals: list[NewGoal]
    conflicts: list[str]


async def run(svc: Services, events: list[Event]) -> list[Proposal]:
    events = [e for e in events if e["type"] in MESSAGE_TYPES]
    if not events:
        return []
    goals = await svc.repo.list_rows("goals", status=LIVE_GOAL)
    if not goals and len(events) < 2:
        return []
    user = (
        f"GOALS:\n{fmt_rows(goals, 'status', 'origin', 'priority', 'description')}\n\n"
        f"EVENTS:\n{fmt_events(events)}"
    )
    out = await svc.llm.complete_json(SYSTEM, user, GoalReview)
    evidence = [e["event_id"] for e in events]
    known = {g["id"]: g for g in goals}
    proposals = [
        Proposal(
            agent="goals",
            operation="update_goal",
            target=c.goal_id,
            payload={"status": c.status},
            evidence=evidence,
            confidence=0.75,
        )
        for c in out.changes
        if c.goal_id in known and known[c.goal_id]["status"] != c.status
    ]
    proposals += [
        Proposal(
            agent="goals",
            operation="create_goal",
            payload={
                "description": g.description,
                "priority": clamp(g.priority),
                "origin": "inferred",
                "status": "proposed",
            },
            evidence=evidence,
            confidence=0.5,
        )
        for g in out.inferred_goals
    ]
    if out.conflicts:
        working = (await svc.repo.singleton("working_state"))["data"]
        proposals.append(
            Proposal(
                agent="goals",
                operation="set_working_state",
                payload={"patch": {"open_questions": merge_questions(working, out.conflicts)}},
                evidence=evidence,
                confidence=0.6,
            )
        )
    return proposals
