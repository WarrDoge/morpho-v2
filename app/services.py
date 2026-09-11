from dataclasses import dataclass

import asyncpg

from app.db import connect
from app.events.store import EventStore
from app.llm import LLM, DeepInfraLLM
from app.state.engine import StateEngine
from app.state.repository import Repository


@dataclass
class Services:
    pool: asyncpg.Pool
    llm: LLM
    events: EventStore
    repo: Repository
    engine: StateEngine

    async def close(self) -> None:
        await self.pool.close()


async def build_services(dsn: str | None = None, llm: LLM | None = None) -> Services:
    pool = await connect(dsn)
    llm = llm or DeepInfraLLM()
    return Services(pool, llm, EventStore(pool), Repository(pool), StateEngine(pool, llm))
