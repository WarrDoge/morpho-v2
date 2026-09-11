"""Opt-in: uv run pytest -m live. Hits DeepInfra with the real model."""

import pytest

from app.agents.interaction import Interpretation
from app.config import settings
from app.llm import DeepInfraLLM
from app.services import Services, build_services
from tests.conftest import TEST_DSN

pytestmark = pytest.mark.live


@pytest.mark.skipif(not settings.deepinfra_api_key, reason="no DEEPINFRA_API_KEY")
async def test_structured_output_and_embeddings() -> None:
    llm = DeepInfraLLM()
    out = await llm.complete_json(
        "Interpret the input. Return JSON.",
        'WORKING STATE: {}\nEXISTING GOALS: (none)\nINPUT: "I moved to Berlin, plan my move"',
        Interpretation,
    )
    assert any("berlin" in e.name.lower() for e in out.entities)
    assert out.new_explicit_requests
    (v,) = await llm.embed(["hello"])
    assert len(v) == settings.embed_dim
    assert await llm.complete_text("Reply with one word.", "ping")


TURNS = [
    "Hi, I'm Dana. I just moved from Lisbon to Berlin for a new job at a startup called Nimbus.",
    "I have a dog named Rex, a beagle. I need to find a vet near Prenzlauer Berg.",
    "I'm vegetarian and I'm learning German; classes are on Tuesday evenings.",
    "Help me make a short checklist for registering my address (Anmeldung).",
    "The Anmeldung appointment is booked for next Thursday.",
    "Correction: Rex is not a beagle, he is a basset hound.",
    "Nimbus wants me to start on the 1st. I'm nervous about the commute.",
    "What kind of restaurants should I look for near the office?",
    "Remind me, what breed is Rex?",
    "What do you remember about where I moved from and where I work?",
]


@pytest.mark.skipif(not settings.deepinfra_api_key, reason="no DEEPINFRA_API_KEY")
async def test_ten_turn_scenario(fresh: Services) -> None:
    from httpx import ASGITransport, AsyncClient

    from app.api.main import app
    from app.workers.run import cycle

    live = await build_services(TEST_DSN, DeepInfraLLM())
    app.state.svc = live
    replies: list[str] = []
    tokens: list[int] = []
    try:
        async with AsyncClient(transport=ASGITransport(app=app), base_url="http://t") as c:
            for i, text in enumerate(TURNS, 1):
                r = await c.post("/interact", json={"text": text})
                assert r.status_code == 200, r.text
                replies.append(r.json()["response"])
                tokens.append(r.json()["context"]["tokens"])
                if i % 3 == 0:
                    await cycle(live, force_reflect=True)
            await cycle(live, force_reflect=True)

            counts = {
                t: await live.pool.fetchval(f"SELECT count(*) FROM {t}")
                for t in ("memories", "beliefs", "goals", "predictions", "entities")
            }
            folded = await live.pool.fetchval(
                "SELECT count(*) FROM agent_proposals WHERE reason LIKE 'folded%'"
            )
            rejected = await live.pool.fetchval(
                "SELECT count(*) FROM agent_proposals WHERE decision = 'rejected'"
            )
            print(f"\n{counts} folded={folded} rejected={rejected} ctx_tokens={tokens}")
            assert "lisbon" in replies[9].lower() and "nimbus" in replies[9].lower()
            assert "basset" in replies[8].lower()
            assert "vegetarian" in replies[7].lower() or "vegan" in replies[7].lower()

            for table in ("memories", "beliefs"):
                worst = await live.pool.fetchval(
                    f"SELECT max(1 - (a.embedding <=> b.embedding)) FROM {table} a "
                    f"JOIN {table} b ON a.id < b.id WHERE a.valid_until IS NULL "
                    "AND b.valid_until IS NULL AND a.status <> 'archived' "
                    "AND b.status <> 'archived'"
                )
                assert worst is None or worst < settings.dedupe_threshold, (table, worst)

            beliefs = await live.repo.list_rows("beliefs", limit=500)
            cited = {
                e
                for b in beliefs
                for e in b["supporting_evidence"] + b["contradicting_evidence"]
                if e.startswith("evt_")
            }
            known = {e["event_id"] for e in await live.events.get_many(list(cited))}
            assert cited <= known, cited - known

            beagle = [
                b
                for b in beliefs
                if "beagle" in b["proposition"].lower() and "basset" not in b["proposition"].lower()
            ]
            assert beagle
            for b in beagle:
                why = (await c.get(f"/why/{b['id']}")).json()
                assert b["contradicting_evidence"] or b["status"] != "active", why["history"]

            print("beagle beliefs:", [(b["status"], b["confidence"]) for b in beagle])
    finally:
        await live.close()
