import functools
import json
import random
import uuid
from datetime import datetime
from pathlib import Path
from typing import Any

import asyncpg

from app.config import settings

MIGRATIONS = Path(__file__).resolve().parent.parent / "migrations"
STATE_TABLES = [
    "state_transitions",
    "agent_proposals",
    "state_snapshots",
    "event_cursors",
    "entity_relationships",
    "entities",
    "memories",
    "beliefs",
    "goals",
    "predictions",
    "events",
]
_rng = random.Random()


def seed_ids(seed: str) -> None:
    """Deterministic ids for record/replay evals."""
    _rng.seed(seed)


def new_id(prefix: str) -> str:
    return f"{prefix}_{uuid.UUID(int=_rng.getrandbits(128)).hex[:12]}"


def _json_default(o: Any) -> str:
    if isinstance(o, datetime):
        return o.isoformat()
    raise TypeError(f"not JSON serializable: {type(o).__name__}")


_dumps = functools.partial(json.dumps, default=_json_default)


async def _init_conn(conn: asyncpg.Connection) -> None:
    for t in ("jsonb", "json"):
        await conn.set_type_codec(t, encoder=_dumps, decoder=json.loads, schema="pg_catalog")


async def ensure_database(dsn: str) -> None:
    base, name = dsn.rsplit("/", 1)
    admin = await asyncpg.connect(base + "/morpho")
    try:
        if not await admin.fetchval("SELECT 1 FROM pg_database WHERE datname = $1", name):
            await admin.execute(f'CREATE DATABASE "{name}"')
    finally:
        await admin.close()


async def connect(dsn: str | None = None) -> asyncpg.Pool:
    pool = await asyncpg.create_pool(dsn or settings.database_url, init=_init_conn, min_size=1)
    async with pool.acquire() as conn:
        for path in sorted(MIGRATIONS.glob("*.sql")):
            await conn.execute(path.read_text())
    return pool


async def reset_state(pool: asyncpg.Pool) -> None:
    """Wipe everything and re-run migrations (tests and evals)."""
    await pool.execute(f"TRUNCATE {', '.join(STATE_TABLES)} RESTART IDENTITY CASCADE")
    await pool.execute("DELETE FROM self_state; DELETE FROM working_state")
    for path in sorted(MIGRATIONS.glob("*.sql")):
        await pool.execute(path.read_text())


def vec(values: list[float] | None) -> str | None:
    return None if values is None else "[" + ",".join(f"{v:.7g}" for v in values) + "]"


def row_dict(row: asyncpg.Record | None) -> dict[str, Any] | None:
    if row is None:
        return None
    d = dict(row)
    d.pop("embedding", None)
    return d
