"""Background worker: event consumers + periodic consolidation, reflection, snapshot."""

import asyncio
import logging
from collections.abc import Awaitable, Callable
from datetime import UTC, datetime
from typing import Any

from app.agents import beliefs, consolidation, goals, memory, reflection, self_model
from app.config import settings
from app.events.store import Event
from app.services import Services, build_services
from app.state.models import Proposal
from app.state.snapshots import take_snapshot

log = logging.getLogger("morpho.worker")
Consumer = Callable[[Services, list[Event]], Awaitable[list[Proposal]]]
CONSUMERS: dict[str, Consumer] = {
    "memory": memory.run,
    "beliefs": beliefs.run,
    "self_model": self_model.run,
    "goals": goals.run,
}
REFLECT = "reflection"
CYCLE_LOCK = 0x6D6F7270


async def commit_all(
    svc: Services, proposals: list[Proposal], fallback: str | None
) -> dict[str, int]:
    proposals = proposals[: settings.max_proposals_per_cycle]
    results = await svc.engine.commit(proposals, fallback)
    for p, r in zip(proposals, results, strict=True):
        if not r.accepted:
            log.info("rejected %s %s: %s", p.agent, p.operation, r.reason)
    accepted = sum(r.accepted for r in results)
    return {"accepted": accepted, "rejected": len(results) - accepted}


async def cycle(svc: Services, force_reflect: bool = False) -> dict[str, Any]:
    async with svc.pool.acquire() as conn, conn.transaction():
        if not await conn.fetchval("SELECT pg_try_advisory_xact_lock($1)", CYCLE_LOCK):
            return {}
        return await _cycle(svc, force_reflect)


async def _cycle(svc: Services, force_reflect: bool) -> dict[str, Any]:
    stats: dict[str, Any] = {}
    for name, fn in CONSUMERS.items():
        cursor = await svc.repo.cursor(name)
        events = await svc.events.after(cursor, 20)
        if not events:
            continue
        try:
            proposals = await fn(svc, events)
        except Exception:
            failures = await svc.repo.bump_failures(name)
            if failures < settings.consumer_max_failures:
                log.exception("consumer %s failed (%d); will retry", name, failures)
                continue
            log.exception("consumer %s failed %d times; skipping batch", name, failures)
            proposals = []
        stats[name] = await commit_all(svc, proposals, events[-1]["event_id"])
        await svc.repo.set_cursor(name, events[-1]["id"])

    last_id = await svc.events.last_id()
    row = await svc.pool.fetchrow(
        "SELECT last_event_id, updated_at FROM event_cursors WHERE consumer = $1", REFLECT
    )
    new = last_id - (row["last_event_id"] if row else 0)
    idle = (datetime.now(UTC) - row["updated_at"]).total_seconds() if row else 1e9
    if (
        force_reflect
        or new >= settings.reflect_every_n_events
        or (new > 0 and idle >= settings.idle_reflect_seconds)
    ):
        for name, run in (("consolidation", consolidation.run), (REFLECT, reflection.run)):
            try:
                stats[name] = await commit_all(svc, await run(svc), None)
            except Exception:
                log.exception("%s failed", name)
        snap = await take_snapshot(svc)
        stats["snapshot"] = snap["id"]
        await svc.repo.set_cursor(REFLECT, last_id)
    return stats


async def run_forever(svc: Services) -> None:
    while True:
        try:
            stats = await cycle(svc)
            if stats:
                log.info("cycle %s", stats)
        except Exception:
            log.exception("worker cycle failed")
        await asyncio.sleep(settings.worker_poll_seconds)


async def main() -> None:
    logging.basicConfig(level=logging.INFO)
    svc = await build_services()
    try:
        await run_forever(svc)
    finally:
        await svc.close()


if __name__ == "__main__":
    asyncio.run(main())
