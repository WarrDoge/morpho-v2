"""Interprets one incoming interaction; proposes working-state, entities, explicit goals (§9.1)."""

import json

from app.agents.base import Strict, clamp, event_text, fmt_rows, merge_questions
from app.events.store import Event
from app.services import Services
from app.state.models import LIVE_GOAL, Proposal

SYSTEM = """You are the interaction agent of a persistent-state assistant.
Interpret the latest user input against the current working state and goals.
Identify entities (people, places, projects, things), explicit requests the user made that are not
already covered by an existing goal, open questions, and constraints. Rate how important this input
is to remember long-term (0 = trivia, 1 = critical). Return only the JSON object."""


class Entity(Strict):
    name: str
    kind: str


class Interpretation(Strict):
    current_topic: str
    current_task: str | None
    entities: list[Entity]
    new_explicit_requests: list[str]
    open_questions: list[str]
    constraints: list[str]
    importance: float


async def run(svc: Services, event: Event) -> list[Proposal]:
    working = (await svc.repo.singleton("working_state"))["data"]
    goals = await svc.repo.list_rows("goals", status=LIVE_GOAL)
    user = (
        f"WORKING STATE:\n{json.dumps(working)}\n\nEXISTING GOALS:\n"
        f"{fmt_rows(goals, 'status', 'description')}\n\nINPUT:\n{event_text(event)}"
    )
    out = await svc.llm.complete_json(SYSTEM, user, Interpretation)
    eid = event["event_id"]
    patch = {
        "current_topic": out.current_topic,
        "current_task": out.current_task,
        "active_entities": [e.name for e in out.entities],
        "current_constraints": out.constraints,
        "last_input_importance": clamp(out.importance),
    }
    if out.open_questions:
        patch["open_questions"] = merge_questions(working, out.open_questions)
    proposals = [
        Proposal(
            agent="interaction",
            operation="set_working_state",
            payload={"patch": patch},
            evidence=[eid],
            confidence=0.9,
        )
    ]
    proposals += [
        Proposal(
            agent="interaction",
            operation="upsert_entity",
            payload={"name": e.name, "kind": e.kind or "thing"},
            evidence=[eid],
            confidence=0.8,
        )
        for e in out.entities
    ]
    proposals += [
        Proposal(
            agent="interaction",
            operation="create_goal",
            payload={"description": r, "priority": 0.7, "origin": "user", "status": "active"},
            evidence=[eid],
            confidence=0.85,
        )
        for r in out.new_explicit_requests
    ]
    return proposals
