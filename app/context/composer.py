"""Projects persistent state into a token-budgeted prompt (SPEC §12, §32)."""

import json
from datetime import UTC, datetime
from typing import Any

from app.config import settings
from app.context.ranking import score
from app.services import Services
from app.state.models import LIVE_BELIEF, LIVE_MEMORY

SECTIONS = {  # name: (title, share of budget)
    "working": ("WORKING STATE", 0.10),
    "memories": ("RELEVANT MEMORIES", 0.30),
    "beliefs": ("RELEVANT BELIEFS", 0.15),
    "world": ("WORLD MODEL", 0.10),
    "self": ("SELF MODEL", 0.10),
    "goals": ("ACTIVE GOALS", 0.10),
    "recent": ("RECENT EVENTS", 0.15),
}


def tokens(text: str) -> int:
    return len(text) // 4 + 1


def _fill(lines: list[str], budget: int) -> tuple[list[str], int, int]:
    kept, used, dropped = [], 0, 0
    for line in lines:
        t = tokens(line)
        if used + t <= budget:
            kept.append(line)
            used += t
        else:
            dropped += 1
    return kept, used, dropped


async def _ranked(
    svc: Services, table: str, emb: list[float], k: int, statuses: list[str], now: datetime
) -> list[dict[str, Any]]:
    rows = await svc.repo.similar(table, emb, k=k, statuses=statuses)
    for r in rows:
        r["score"] = score(r, max(r["relevance"], 0.0), now, settings.memory_half_life_days)
    rows.sort(key=lambda r: r["score"], reverse=True)
    return rows


async def compose(
    svc: Services, input_text: str, budget: int | None = None
) -> tuple[str, dict[str, Any]]:
    budget = budget or settings.context_token_budget
    now = datetime.now(UTC)
    (emb,) = await svc.llm.embed([input_text])
    working = (await svc.repo.singleton("working_state"))["data"]
    self_data = (await svc.repo.singleton("self_state"))["data"]
    mems = await _ranked(svc, "memories", emb, 20, LIVE_MEMORY, now)
    beliefs = await _ranked(svc, "beliefs", emb, 10, LIVE_BELIEF, now)
    ents = [e for n in working.get("active_entities", []) if (e := await svc.repo.find_entity(n))]
    rels = await svc.repo.relationships_for([e["id"] for e in ents]) if ents else []
    goals = await svc.repo.list_rows("goals", status=["active", "blocked"])
    goals.sort(key=lambda g: g["priority"], reverse=True)
    recent = await svc.events.recent(10)

    sections: dict[str, list[str]] = {
        "working": [
            f"{k}: {json.dumps(v)}"
            for k, v in working.items()
            if v not in (None, [], "", {}) and k != "recent_events"
        ],
        "memories": [
            f"[{r['id']}] ({r['kind']}, imp={r['importance']:.2f}, conf={r['confidence']:.2f}) "
            f"{r['summary']}"
            for r in mems
        ],
        "beliefs": [
            f"[{r['id']}] ({r['status']}, conf={r['confidence']:.2f}) {r['proposition']}"
            for r in beliefs
        ],
        "world": [f"{e['name']} ({e['kind']}): {json.dumps(e['attributes'])}" for e in ents]
        + [f"{r['src_name']} --{r['rel']}--> {r['dst_name']}" for r in rels],
        "self": [f"{k}: {json.dumps(v)}" for k, v in self_data.items() if v],
        "goals": [
            f"[{g['id']}] ({g['status']}, {g['origin']}, p={g['priority']:.2f}) {g['description']}"
            for g in goals
        ],
        "recent": [
            f"{e['ts']:%Y-%m-%d %H:%M} {e['source']}/{e['type']}: {json.dumps(e['payload'])[:300]}"
            for e in recent
        ],
    }
    out: list[str] = []
    manifest: dict[str, Any] = {"budget": budget, "sections": {}}
    carry = 0
    for name, (title, share) in SECTIONS.items():
        section_budget = int(budget * share) + carry
        kept, used, dropped = _fill(sections[name], section_budget)
        carry = section_budget - used
        manifest["sections"][name] = {"included": len(kept), "dropped": dropped, "tokens": used}
        if kept:
            out.append(f"## {title}\n" + "\n".join(kept))
    for name, rows in (("memories", mems), ("beliefs", beliefs)):
        manifest[name] = [
            {"id": r["id"], "score": round(r["score"], 4)}
            for r in rows[: manifest["sections"][name]["included"]]
        ]
    text = "\n\n".join(out)
    manifest["tokens"] = tokens(text)
    return text, manifest
