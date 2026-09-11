from typing import Any

import asyncpg

from app.db import row_dict, vec

Row = dict[str, Any]
DB = asyncpg.Pool | asyncpg.Connection


def _marshal(fields: Row, start: int) -> tuple[list[str], list[Any]]:
    ph = [f"${i}::vector" if k == "embedding" else f"${i}" for i, k in enumerate(fields, start)]
    return ph, [vec(v) if k == "embedding" else v for k, v in fields.items()]


def _one(row: asyncpg.Record | None) -> Row:
    out = row_dict(row)
    assert out is not None
    return out


class Repository:
    """Works on a pool or on a connection inside a transaction."""

    def __init__(self, db: DB) -> None:
        self.db = db

    async def get(self, table: str, id: str) -> Row | None:
        return row_dict(await self.db.fetchrow(f"SELECT * FROM {table} WHERE id = $1", id))

    async def list_rows(
        self, table: str, status: str | list[str] | None = None, limit: int = 100
    ) -> list[Row]:
        where, args = "", []
        if status:
            where, args = (
                "WHERE status = ANY($2)",
                [[status] if isinstance(status, str) else status],
            )
        rows = await self.db.fetch(
            f"SELECT * FROM {table} {where} ORDER BY created_at DESC LIMIT $1", limit, *args
        )
        return [r for r in map(row_dict, rows) if r]

    async def insert(self, table: str, fields: Row) -> Row:
        ph, args = _marshal(fields, 1)
        return _one(
            await self.db.fetchrow(
                f"INSERT INTO {table} ({', '.join(fields)}) VALUES ({', '.join(ph)}) RETURNING *",
                *args,
            )
        )

    async def update(self, table: str, id: str | int, fields: Row) -> Row:
        ph, args = _marshal(fields, 2)
        sets = ", ".join(f"{k} = {p}" for k, p in zip(fields, ph, strict=True))
        return _one(
            await self.db.fetchrow(
                f"UPDATE {table} SET {sets} WHERE id = $1 RETURNING *", id, *args
            )
        )

    async def similar(
        self, table: str, embedding: list[float], k: int = 10, statuses: list[str] | None = None
    ) -> list[Row]:
        where = "embedding IS NOT NULL"
        args: list[Any] = [vec(embedding), k]
        if statuses:
            where += " AND status = ANY($3)"
            args.append(statuses)
        rows = await self.db.fetch(
            f"SELECT *, 1 - (embedding <=> $1::vector) AS relevance FROM {table} "
            f"WHERE {where} ORDER BY embedding <=> $1::vector LIMIT $2",
            *args,
        )
        return [r for r in map(row_dict, rows) if r]

    async def singleton(self, table: str) -> Row:
        return _one(await self.db.fetchrow(f"SELECT * FROM {table} WHERE id = 1"))

    async def find_entity(self, name: str, kind: str | None = None) -> Row | None:
        if name.startswith("ent_"):
            return await self.get("entities", name)
        if kind:
            row = await self.db.fetchrow(
                "SELECT * FROM entities WHERE lower(name) = lower($1) AND kind = $2", name, kind
            )
        else:
            row = await self.db.fetchrow(
                "SELECT * FROM entities WHERE lower(name) = lower($1) ORDER BY created_at LIMIT 1",
                name,
            )
        return row_dict(row)

    async def relationships_for(self, entity_ids: list[str]) -> list[Row]:
        rows = await self.db.fetch(
            "SELECT r.*, s.name AS src_name, d.name AS dst_name FROM entity_relationships r "
            "JOIN entities s ON s.id = r.src JOIN entities d ON d.id = r.dst "
            "WHERE (r.src = ANY($1) OR r.dst = ANY($1)) AND r.valid_until IS NULL "
            "ORDER BY r.created_at, r.id",
            entity_ids,
        )
        return [dict(r) for r in rows]

    async def transitions_for(self, object_id: str, limit: int = 50) -> list[Row]:
        rows = await self.db.fetch(
            "SELECT t.*, p.agent AS proposal_agent, p.operation, p.evidence, p.confidence, "
            "p.decision, p.reason FROM state_transitions t "
            "JOIN agent_proposals p ON p.id = t.proposal_id "
            "WHERE t.object_id = $1 ORDER BY t.id LIMIT $2",
            object_id,
            limit,
        )
        return [dict(r) for r in rows]

    async def transitions(self, limit: int = 100, after: int = 0) -> list[Row]:
        rows = await self.db.fetch(
            "SELECT * FROM state_transitions WHERE id > $2 ORDER BY id LIMIT $1", limit, after
        )
        return [dict(r) for r in rows]

    async def cursor(self, consumer: str) -> int:
        v = await self.db.fetchval(
            "SELECT last_event_id FROM event_cursors WHERE consumer = $1", consumer
        )
        return int(v or 0)

    async def set_cursor(self, consumer: str, last_event_id: int) -> None:
        await self.db.execute(
            "INSERT INTO event_cursors (consumer, last_event_id) VALUES ($1, $2) "
            "ON CONFLICT (consumer) DO UPDATE SET last_event_id = $2, failures = 0, "
            "updated_at = now()",
            consumer,
            last_event_id,
        )

    async def bump_failures(self, consumer: str) -> int:
        n = await self.db.fetchval(
            "INSERT INTO event_cursors (consumer, failures) VALUES ($1, 1) "
            "ON CONFLICT (consumer) DO UPDATE SET failures = event_cursors.failures + 1 "
            "RETURNING failures",
            consumer,
        )
        return int(n)

    async def snapshots(self, limit: int = 20) -> list[Row]:
        rows = await self.db.fetch("SELECT * FROM state_snapshots ORDER BY id DESC LIMIT $1", limit)
        return [dict(r) for r in rows]
