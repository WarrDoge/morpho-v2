"""HTTP surface: interaction lifecycle (§30) and state inspection (§25)."""

import asyncio
import logging
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager
from typing import Any

from fastapi import FastAPI, HTTPException, Request
from pydantic import BaseModel

from app.agents import interaction
from app.config import settings
from app.context.composer import compose
from app.services import Services, build_services
from app.state.models import table_for
from app.workers.run import cycle, run_forever

log = logging.getLogger("morpho.api")
RESPONSE_SYSTEM = """You are an assistant with a persistent cognitive state. The STATE below is a
projection of what you currently remember, believe, and are working on; it is your only source of
continuity. Treat it as a fallible model, not ground truth, and say when you are uncertain.
Answer the user's message helpfully and concisely."""
LIST_TABLES = {
    "memories": "memories",
    "beliefs": "beliefs",
    "goals": "goals",
    "predictions": "predictions",
    "entities": "entities",
    "relationships": "entity_relationships",
}


@asynccontextmanager
async def lifespan(app: FastAPI) -> AsyncIterator[None]:
    svc = getattr(app.state, "svc", None) or await build_services()
    app.state.svc = svc
    task = asyncio.create_task(run_forever(svc)) if settings.worker_inprocess else None
    try:
        yield
    finally:
        if task:
            task.cancel()
        await svc.close()


app = FastAPI(title="morpho", lifespan=lifespan)


def _svc(request: Request) -> Services:
    return request.app.state.svc


class Interact(BaseModel):
    text: str
    session_id: str | None = None


@app.post("/interact")
async def interact(body: Interact, request: Request) -> dict[str, Any]:
    svc = _svc(request)
    event = await svc.events.append("user_message", "user", {"text": body.text}, body.session_id)
    eid = event["event_id"]
    try:
        await svc.engine.commit(await interaction.run(svc, event), eid)
    except Exception:
        log.exception("interaction agent failed; responding from existing state")
    context, manifest = await compose(svc, body.text)
    reply = await svc.llm.complete_text(f"{RESPONSE_SYSTEM}\n\n# STATE\n{context}", body.text)
    reply_event = await svc.events.append(
        "assistant_message", "assistant", {"text": reply, "in_reply_to": eid}, body.session_id
    )
    return {
        "response": reply,
        "event_id": eid,
        "response_event_id": reply_event["event_id"],
        "context": manifest,
    }


@app.get("/health")
async def health() -> dict[str, str]:
    return {"status": "ok"}


@app.get("/state")
async def state(request: Request) -> dict[str, Any]:
    repo = _svc(request).repo
    return {
        "working_state": await repo.singleton("working_state"),
        "self_state": await repo.singleton("self_state"),
    }


@app.get("/events")
async def events(request: Request, limit: int = 50) -> list[dict[str, Any]]:
    return await _svc(request).events.recent(limit)


@app.get("/proposals")
async def proposals(
    request: Request, decision: str | None = None, limit: int = 100
) -> list[dict[str, Any]]:
    rows = await _svc(request).pool.fetch(
        "SELECT * FROM agent_proposals WHERE ($1::text IS NULL OR decision = $1) "
        "ORDER BY created_at DESC LIMIT $2",
        decision,
        limit,
    )
    return [dict(r) for r in rows]


@app.get("/transitions")
async def transitions(request: Request, limit: int = 100, after: int = 0) -> list[dict[str, Any]]:
    return await _svc(request).repo.transitions(limit, after)


@app.get("/snapshots")
async def snapshots(request: Request, limit: int = 20) -> list[dict[str, Any]]:
    return await _svc(request).repo.snapshots(limit)


@app.get("/why/{object_id}")
async def why(object_id: str, request: Request) -> dict[str, Any]:
    """Provenance chain: object -> transitions -> proposals -> evidence (§25)."""
    svc = _svc(request)
    table = table_for(object_id)
    if table is None:
        raise HTTPException(404, "unknown object id prefix")
    obj = await svc.repo.get(table, object_id)
    if obj is None:
        raise HTTPException(404, "not found")
    history = await svc.repo.transitions_for(object_id)
    ids: set[str] = set()
    for key in ("evidence", "source_events", "supporting_evidence", "contradicting_evidence"):
        ids.update(obj.get(key) or [])
    for t in history:
        ids.update(t["evidence"] or [])
    events = await svc.events.get_many([i for i in ids if i.startswith("evt_")])
    derived = []
    for i in ids:
        t = table_for(i)
        if t and i != object_id and (row := await svc.repo.get(t, i)):
            derived.append(row)
    return {
        "object": obj,
        "table": table,
        "history": history,
        "events": events,
        "derived_from": derived,
    }


@app.get("/context/preview")
async def context_preview(text: str, request: Request) -> dict[str, Any]:
    context, manifest = await compose(_svc(request), text)
    return {"context": context, "manifest": manifest}


@app.post("/admin/cycle")
async def admin_cycle(request: Request, reflect: bool = False) -> dict[str, Any]:
    return await cycle(_svc(request), force_reflect=reflect)


@app.get("/{table}")  # last: catch-all list endpoint for state tables
async def list_table(
    table: str, request: Request, status: str | None = None, limit: int = 100
) -> list[dict[str, Any]]:
    if table not in LIST_TABLES:
        raise HTTPException(404)
    return await _svc(request).repo.list_rows(
        LIST_TABLES[table], status.split(",") if status else None, limit
    )
