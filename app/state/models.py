from datetime import datetime
from typing import Any, Literal

from pydantic import BaseModel, Field

MemoryKind = Literal["episodic", "semantic"]
MemoryStatus = Literal[
    "candidate", "active", "reinforced", "consolidated", "deprecated", "archived"
]
BeliefStatus = Literal["hypothesis", "active", "uncertain", "contradicted", "deprecated"]
GoalStatus = Literal["proposed", "active", "blocked", "completed", "abandoned", "superseded"]
GoalOrigin = Literal["user", "system", "inferred"]

LIVE_MEMORY = ["candidate", "active", "reinforced", "consolidated"]
LIVE_BELIEF = ["hypothesis", "active", "uncertain", "contradicted"]
LIVE_GOAL = ["proposed", "active", "blocked"]

Operation = Literal[
    "create_memory",
    "update_memory",
    "merge_memories",
    "create_belief",
    "update_belief",
    "create_goal",
    "update_goal",
    "create_prediction",
    "verify_prediction",
    "upsert_entity",
    "add_relationship",
    "set_working_state",
    "update_self_state",
]


class Proposal(BaseModel):
    agent: str
    operation: Operation
    target: str | None = None
    payload: dict[str, Any] = Field(default_factory=dict)
    evidence: list[str] = Field(default_factory=list)
    confidence: float = Field(ge=0, le=1, default=0.5)


class CreateMemory(BaseModel):
    kind: MemoryKind = "episodic"
    summary: str = Field(min_length=1)
    importance: float = Field(ge=0, le=1, default=0.5)
    confidence: float = Field(ge=0, le=1, default=0.8)
    status: MemoryStatus = "active"
    source_events: list[str] = Field(default_factory=list)
    entity_ids: list[str] = Field(default_factory=list)


class UpdateMemory(BaseModel):
    summary: str | None = None
    importance: float | None = Field(ge=0, le=1, default=None)
    confidence: float | None = Field(ge=0, le=1, default=None)
    status: MemoryStatus | None = None
    reinforce: bool = False
    add_source_events: list[str] = Field(default_factory=list)
    add_entity_ids: list[str] = Field(default_factory=list)
    expected_version: int | None = None


class MergeMemories(BaseModel):
    source_ids: list[str] = Field(min_length=2)
    summary: str = Field(min_length=1)
    kind: MemoryKind = "episodic"
    importance: float = Field(ge=0, le=1, default=0.5)
    confidence: float = Field(ge=0, le=1, default=0.8)


class CreateBelief(BaseModel):
    proposition: str = Field(min_length=1)
    confidence: float = Field(ge=0, le=1, default=0.5)
    status: BeliefStatus = "hypothesis"


class UpdateBelief(BaseModel):
    confidence: float | None = Field(ge=0, le=1, default=None)
    status: BeliefStatus | None = None
    add_supporting: list[str] = Field(default_factory=list)
    add_contradicting: list[str] = Field(default_factory=list)
    expected_version: int | None = None


class CreateGoal(BaseModel):
    description: str = Field(min_length=1)
    priority: float = Field(ge=0, le=1, default=0.5)
    origin: GoalOrigin = "inferred"
    status: GoalStatus = "active"
    parent_goal: str | None = None
    deadline: datetime | None = None


class UpdateGoal(BaseModel):
    status: GoalStatus | None = None
    priority: float | None = Field(ge=0, le=1, default=None)
    expected_version: int | None = None


class CreatePrediction(BaseModel):
    prediction: str = Field(min_length=1)
    probability: float = Field(ge=0, le=1)
    deadline: datetime | None = None


class VerifyPrediction(BaseModel):
    verified: bool


class UpsertEntity(BaseModel):
    name: str = Field(min_length=1)
    kind: str = "thing"
    attributes: dict[str, Any] = Field(default_factory=dict)


class AddRelationship(BaseModel):
    src: str
    rel: str = Field(min_length=1)
    dst: str
    confidence: float = Field(ge=0, le=1, default=0.7)


class SetWorkingState(BaseModel):
    patch: dict[str, Any]
    expected_version: int | None = None


class UpdateSelfState(BaseModel):
    patch: dict[str, Any]
    expected_version: int | None = None


PAYLOADS: dict[str, type[BaseModel]] = {
    "create_memory": CreateMemory,
    "update_memory": UpdateMemory,
    "merge_memories": MergeMemories,
    "create_belief": CreateBelief,
    "update_belief": UpdateBelief,
    "create_goal": CreateGoal,
    "update_goal": UpdateGoal,
    "create_prediction": CreatePrediction,
    "verify_prediction": VerifyPrediction,
    "upsert_entity": UpsertEntity,
    "add_relationship": AddRelationship,
    "set_working_state": SetWorkingState,
    "update_self_state": UpdateSelfState,
}

TABLE_OF_PREFIX = {
    "mem": "memories",
    "belief": "beliefs",
    "goal": "goals",
    "pred": "predictions",
    "ent": "entities",
    "rel": "entity_relationships",
}


def table_for(object_id: str) -> str | None:
    return TABLE_OF_PREFIX.get(object_id.split("_", 1)[0])


def union(a: list[str], b: list[str]) -> list[str]:
    return list(dict.fromkeys([*a, *b]))
