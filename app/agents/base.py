import asyncio
import json
from collections.abc import Awaitable, Callable
from typing import Any

from pydantic import BaseModel, ConfigDict

from app.events.store import Event
from app.services import Services
from app.state.models import Proposal, union


class Strict(BaseModel):
    """All fields required + additionalProperties:false, as strict JSON-schema mode needs."""

    model_config = ConfigDict(extra="forbid")


MESSAGE_TYPES = {"user_message", "assistant_message"}


def event_text(e: Event) -> str:
    p = e["payload"]
    return p.get("text") if isinstance(p, dict) and "text" in p else json.dumps(p)


def fmt_events(events: list[Event]) -> str:
    return "\n".join(
        f"[{e['event_id']}] {e['source']}/{e['type']}: {event_text(e)}" for e in events
    )


def fmt_rows(rows: list[dict[str, Any]], *fields: str) -> str:
    return (
        "\n".join(f"[{r['id']}] " + " | ".join(f"{f}={r.get(f)}" for f in fields) for r in rows)
        or "(none)"
    )


def clamp(x: float) -> float:
    return min(1.0, max(0.0, x))


def merge_questions(working: dict[str, Any], new: list[str]) -> list[str]:
    return union(working.get("open_questions", []), new)[:10]


async def per_message(
    svc: Services,
    events: list[Event],
    fn: Callable[[Services, Event], Awaitable[list[Proposal]]],
) -> list[Proposal]:
    batches = await asyncio.gather(*(fn(svc, e) for e in events if e["type"] in MESSAGE_TYPES))
    return [p for b in batches for p in b]
