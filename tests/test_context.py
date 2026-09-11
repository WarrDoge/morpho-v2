from app.context.composer import compose
from app.services import Services
from tests.helpers import event, memory


async def test_relevant_state_enters_prompt_irrelevant_stays_out(fresh: Services) -> None:
    e = await event(fresh, "seed")
    rel = await memory(fresh, "User is moving to Berlin next month", e["event_id"], importance=0.8)
    irr = await memory(
        fresh, "Lecture notes on quantum chromodynamics", e["event_id"], importance=0.8
    )
    text, manifest = await compose(fresh, "help me with moving to Berlin")
    ids = [m["id"] for m in manifest["memories"]]
    assert ids[0] == rel and "moving to Berlin" in text
    assert manifest["memories"][0]["score"] > next(
        m["score"] for m in manifest["memories"] if m["id"] == irr
    )
    text, manifest = await compose(fresh, "help me with moving to Berlin", budget=100)
    assert manifest["tokens"] <= 100
    assert rel in text and irr not in text


async def test_context_stays_within_budget(fresh: Services) -> None:
    e = await event(fresh, "seed")
    for i in range(40):
        await memory(
            fresh, f"Fact number {i} about the user's long project history " * 3, e["event_id"]
        )
    for _ in range(15):
        await event(fresh, "a fairly long message " * 20)
    text, manifest = await compose(fresh, "project history", budget=600)
    assert manifest["tokens"] <= 600
    assert manifest["sections"]["memories"]["dropped"] > 0
    assert manifest["sections"]["recent"]["included"] > 0
