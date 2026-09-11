from typing import Any

from app.services import Services
from app.state.models import Operation, Proposal


async def event(svc: Services, text: str, source: str = "user") -> dict[str, Any]:
    return await svc.events.append(f"{source}_message", source, {"text": text})


async def commit(
    svc: Services, agent: str, op: Operation, payload: dict[str, Any], **kw: Any
) -> str:
    p = Proposal(agent=agent, operation=op, payload=payload, **kw)
    r = await svc.engine.commit_one(p)
    assert r.accepted, r.reason
    return r.object_ids[0]


async def memory(svc: Services, summary: str, evt: str, **fields: Any) -> str:
    return await commit(
        svc, "memory", "create_memory", {"summary": summary, **fields}, evidence=[evt]
    )


async def belief(svc: Services, proposition: str, evt: str, confidence: float = 0.6) -> str:
    return await commit(
        svc,
        "beliefs",
        "create_belief",
        {"proposition": proposition, "confidence": confidence, "status": "active"},
        evidence=[evt],
    )
