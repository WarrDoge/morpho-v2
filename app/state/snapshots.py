"""State snapshots (SPEC §18) and deterministic replay from transitions (SPEC §24)."""

from datetime import datetime
from typing import Any

from app.services import Services
from app.state.models import LIVE_GOAL, LIVE_MEMORY
from app.state.repository import Repository

STATE_TABLES = [
    "entity_relationships",
    "entities",
    "memories",
    "beliefs",
    "goals",
    "predictions",
]
DT_KEYS = {
    "created_at",
    "updated_at",
    "last_reinforced_at",
    "last_reviewed",
    "valid_from",
    "valid_until",
    "deadline",
    "verified_at",
    "ts",
}
EMBEDDED = {"memories": "summary", "beliefs": "proposition"}


async def take_snapshot(svc: Services) -> dict[str, Any]:
    repo = svc.repo
    data = {
        "working_state": (await repo.singleton("working_state"))["data"],
        "self_state": (await repo.singleton("self_state"))["data"],
        "memories": await repo.list_rows("memories", LIVE_MEMORY, 1000),
        "beliefs": await repo.list_rows("beliefs", ["hypothesis", "active", "uncertain"], 1000),
        "goals": await repo.list_rows("goals", LIVE_GOAL, 1000),
        "predictions": [
            p for p in await repo.list_rows("predictions", limit=1000) if p["verified"] is None
        ],
        "entities": await repo.list_rows("entities", limit=1000),
    }
    row = await svc.pool.fetchrow(
        "INSERT INTO state_snapshots (last_event_id, data) VALUES ($1, $2) RETURNING *",
        await svc.events.last_id(),
        data,
    )
    assert row is not None
    return dict(row)


async def replay(svc: Services, up_to: int | None = None) -> int:
    """Rebuild derived state by re-applying transition after-images. Events untouched."""
    applied = 0
    async with svc.pool.acquire() as conn, conn.transaction():
        repo = Repository(conn)
        for t in STATE_TABLES:
            await conn.execute(f"DELETE FROM {t}")
        rows = await conn.fetch(
            "SELECT * FROM state_transitions WHERE id <= $1 ORDER BY id", up_to or 2**62
        )
        for r in rows:
            table, after = r["table_name"], r["after"]
            if table in ("working_state", "self_state"):
                await conn.execute(
                    f"UPDATE {table} SET data = $1, version = $2 WHERE id = 1",
                    after["data"],
                    after["version"],
                )
                applied += 1
                continue
            if after is None:
                continue
            fields: dict[str, Any] = {k: (_dt(v) if k in DT_KEYS else v) for k, v in after.items()}
            oid = str(after["id"])
            if table in EMBEDDED:
                (fields["embedding"],) = await svc.llm.embed([str(after[EMBEDDED[table]])])
            if await repo.get(table, oid):
                await repo.update(table, oid, fields)
            else:
                await repo.insert(table, fields)
            applied += 1
    return applied


def _dt(v: Any) -> datetime | None:
    return datetime.fromisoformat(v) if isinstance(v, str) else v
