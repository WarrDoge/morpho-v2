import os
from collections.abc import AsyncIterator

import pytest
from httpx import ASGITransport, AsyncClient

from app.db import ensure_database, reset_state
from app.services import Services, build_services
from tests.fake_llm import FakeLLM

TEST_DSN = os.environ.get(
    "TEST_DATABASE_URL", "postgresql://morpho:morpho@localhost:5432/morpho_test"
)


@pytest.fixture(scope="session")
async def svc() -> AsyncIterator[Services]:
    await ensure_database(TEST_DSN)
    s = await build_services(TEST_DSN, FakeLLM())
    yield s
    await s.close()


@pytest.fixture
async def fresh(svc: Services) -> Services:
    await reset_state(svc.pool)
    llm = svc.llm
    assert isinstance(llm, FakeLLM)
    llm.json.clear()
    llm.text.clear()
    llm.errors.clear()
    llm.calls.clear()
    return svc


@pytest.fixture
def llm(fresh: Services) -> FakeLLM:
    assert isinstance(fresh.llm, FakeLLM)
    return fresh.llm


@pytest.fixture
async def client(fresh: Services) -> AsyncIterator[AsyncClient]:
    from app.api.main import app

    app.state.svc = fresh
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://t") as c:
        yield c
