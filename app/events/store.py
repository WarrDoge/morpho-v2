from typing import Any

import asyncpg

from app.db import new_id

Event = dict[str, Any]


class EventStore:
    def __init__(self, pool: asyncpg.Pool) -> None:
        self.pool = pool

    async def append(
        self, type: str, source: str, payload: dict[str, Any], session_id: str | None = None
    ) -> Event:
        row = await self.pool.fetchrow(
            "INSERT INTO events (event_id, type, source, session_id, payload) "
            "VALUES ($1, $2, $3, $4, $5) RETURNING *",
            new_id("evt"),
            type,
            source,
            session_id,
            payload,
        )
        assert row is not None
        return dict(row)

    async def get(self, event_id: str) -> Event | None:
        row = await self.pool.fetchrow("SELECT * FROM events WHERE event_id = $1", event_id)
        return dict(row) if row else None

    async def get_many(self, event_ids: list[str]) -> list[Event]:
        rows = await self.pool.fetch(
            "SELECT * FROM events WHERE event_id = ANY($1) ORDER BY id", event_ids
        )
        return [dict(r) for r in rows]

    async def after(self, cursor: int, limit: int = 20) -> list[Event]:
        rows = await self.pool.fetch(
            "SELECT * FROM events WHERE id > $1 ORDER BY id LIMIT $2", cursor, limit
        )
        return [dict(r) for r in rows]

    async def recent(self, n: int = 10) -> list[Event]:
        rows = await self.pool.fetch("SELECT * FROM events ORDER BY id DESC LIMIT $1", n)
        return [dict(r) for r in reversed(rows)]

    async def last_id(self) -> int:
        return await self.pool.fetchval("SELECT coalesce(max(id), 0) FROM events") or 0
