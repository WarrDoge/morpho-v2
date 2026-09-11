"""Scenario runner: metrics for one scenario, optional baseline comparison.

Usage: uv run python -m evals.run evals/scenarios/dana.json [--label L] [--strict]
       [--baseline evals/results/dana.base.json] [--control]
"""

import argparse
import asyncio
import json
import os
import sys
import time
from pathlib import Path
from typing import Any

from httpx import ASGITransport, AsyncClient

from app.config import settings
from app.context.composer import tokens
from app.db import ensure_database, reset_state, seed_ids
from app.llm import DeepInfraLLM
from app.services import Services, build_services
from app.state.models import LIVE_BELIEF, LIVE_GOAL, LIVE_MEMORY
from app.workers.run import cycle
from evals.llm import CacheMiss, RecordingLLM

ROOT = Path(__file__).resolve().parent
EVAL_DSN = os.environ.get(
    "EVAL_DATABASE_URL", settings.database_url.rsplit("/", 1)[0] + "/morpho_eval"
)
CONTROL_SYSTEM = """You are a helpful assistant. Below is the transcript of your conversation so far
with the user (older turns may have been cut off). Answer the user's latest message concisely."""
LOWER_IS_WORSE = ["probe_accuracy", "update_accuracy", "retrieval_hit", "revision_rate"]
HIGHER_IS_WORSE = ["noise", "dup_max_cos", "llm_calls", "embed_calls", "prompt_tokens"]
INFO = ["compactness", "ctx_tokens", "seconds", "cache_misses", "probes", "revisions", "turns"]

Turn = dict[str, Any]


def judge(reply: str, turn: Turn) -> bool:
    """Every expect group present; a rejected (stale) value may only appear after the answer."""
    r = reply.lower()
    groups = [[a.lower() for a in g] for g in turn.get("expect", [])]
    if not all(any(a in r for a in g) for g in groups):
        return False
    rejects = [x.lower() for x in turn.get("reject", []) if x.lower() in r]
    if not rejects:
        return True
    first_answer = min(r.find(a) for g in groups for a in g if a in r)
    return first_answer < min(r.find(x) for x in rejects)


def _mean(xs: list[float]) -> float | None:
    return round(sum(xs) / len(xs), 4) if xs else None


async def run_harness(
    svc: Services, turns: list[Turn], cycle_every: int
) -> tuple[list[str], list[dict[str, Any]], list[str]]:
    from app.api.main import app

    app.state.svc = svc
    replies, manifests, event_ids = [], [], []
    async with AsyncClient(transport=ASGITransport(app=app), base_url="http://eval") as c:
        for i, t in enumerate(turns, 1):
            r = await c.post("/interact", json={"text": t["text"]})
            r.raise_for_status()
            body = r.json()
            replies.append(body["response"])
            manifests.append(body["context"])
            event_ids.append(body["event_id"])
            if i % cycle_every == 0:
                await cycle(svc, force_reflect=True)
        await cycle(svc, force_reflect=True)
    return replies, manifests, event_ids


async def run_control(llm: RecordingLLM, turns: list[Turn]) -> list[str]:
    replies: list[str] = []
    transcript: list[str] = []
    for t in turns:
        tail: list[str] = []
        used = 0
        for line in reversed(transcript):
            if used + tokens(line) > settings.context_token_budget:
                break
            tail.insert(0, line)
            used += tokens(line)
        prompt = f"{CONTROL_SYSTEM}\n\n# TRANSCRIPT\n" + "\n".join(tail)
        reply = await llm.complete_text(prompt, t["text"])
        replies.append(reply)
        transcript += [f"user: {t['text']}", f"assistant: {reply}"]
    return replies


def behaviour(turns: list[Turn], replies: list[str]) -> dict[str, Any]:
    probes = [(t, r) for t, r in zip(turns, replies, strict=True) if t.get("tag") == "probe"]
    verdicts = [judge(r, t) for t, r in probes]
    updates = [judge(r, t) for t, r in probes if t.get("reject")]
    return {
        "probe_accuracy": _mean([float(v) for v in verdicts]),
        "update_accuracy": _mean([float(v) for v in updates]),
        "probes": len(probes),
    }


async def state_metrics(
    svc: Services, turns: list[Turn], replies: list[str], manifests: list[dict], ids: list[str]
) -> dict[str, Any]:
    repo = svc.repo
    hits: list[float] = []
    for t, m in zip(turns, manifests, strict=True):
        if t.get("tag") != "probe" or not t.get("refs"):
            continue
        refs = {ids[i] for i in t["refs"]}
        hit = False
        for entry in m.get("memories", []):
            row = await repo.get("memories", entry["id"])
            if row and refs & set(row["source_events"]):
                hit = True
                break
        hits.append(float(hit))

    live_mem = await repo.list_rows("memories", LIVE_MEMORY, 10_000)
    distractors = {ids[i] for i, t in enumerate(turns) if t.get("tag") == "distractor"}
    noisy = [m for m in live_mem if m["source_events"] and set(m["source_events"]) <= distractors]

    dup = await svc.pool.fetchval(
        "SELECT max(1 - (a.embedding <=> b.embedding)) FROM memories a JOIN memories b "
        "ON a.id < b.id WHERE a.status = ANY($1) AND b.status = ANY($1)",
        LIVE_MEMORY,
    )

    beliefs = await repo.list_rows("beliefs", limit=10_000)
    revised: list[float] = []
    for t in turns:
        if t.get("tag") != "update" or not t.get("contradicts"):
            continue
        for b in beliefs:
            prop = b["proposition"].lower()
            if all(k.lower() in prop for k in t["contradicts"]) and not any(
                k.lower() in prop for k in t.get("asserts", [])
            ):
                revised.append(float(b["status"] != "active" or bool(b["contradicting_evidence"])))

    live_beliefs = await repo.list_rows("beliefs", LIVE_BELIEF, 10_000)
    goals = await repo.list_rows("goals", LIVE_GOAL, 10_000)
    state_text = " ".join(
        [m["summary"] for m in live_mem]
        + [b["proposition"] for b in live_beliefs]
        + [g["description"] for g in goals]
    )
    transcript = " ".join(t["text"] for t in turns) + " ".join(replies)
    return {
        "retrieval_hit": _mean(hits),
        "noise": round(len(noisy) / len(live_mem), 4) if live_mem else None,
        "dup_max_cos": round(float(dup), 4) if dup is not None else None,
        "revision_rate": _mean(revised),
        "revisions": len(revised),
        "compactness": round(tokens(state_text) / tokens(transcript), 4),
        "ctx_tokens": _mean([float(m["tokens"]) for m in manifests]),
    }


def compare(metrics: dict[str, Any], baseline: dict[str, Any] | None) -> list[str]:
    """Return regression descriptions (empty = pass)."""
    if not baseline:
        return []
    bad = []
    for k in LOWER_IS_WORSE + HIGHER_IS_WORSE:
        a, b = metrics.get(k), baseline.get(k)
        if a is None or b is None:
            continue
        if (k in LOWER_IS_WORSE and a < b) or (k in HIGHER_IS_WORSE and a > b):
            bad.append(f"{k}: {a} vs baseline {b}")
    return bad


def print_table(metrics: dict[str, Any], baseline: dict[str, Any] | None) -> None:
    keys = [k for k in LOWER_IS_WORSE + HIGHER_IS_WORSE + INFO if k in metrics]
    print(f"{'metric':16} {'value':>10} {'baseline':>10}")
    for k in keys:
        b = baseline.get(k) if baseline else None
        print(f"{k:16} {str(metrics[k]):>10} {str(b) if b is not None else '':>10}")


async def run_scenario(
    scenario: Path,
    label: str | None = None,
    strict: bool = False,
    baseline: Path | None = None,
    control: bool = False,
) -> int:
    data = json.loads(scenario.read_text())
    turns: list[Turn] = data["turns"]
    name = scenario.stem + (".control" if control else "")
    cache = ROOT / "cache" / f"{name}.json"
    inner = DeepInfraLLM() if settings.deepinfra_api_key and not strict else None
    llm = RecordingLLM(inner, cache, strict)
    seed_ids(name)
    base = json.loads(baseline.read_text())["metrics"] if baseline else None
    t0 = time.monotonic()
    metrics: dict[str, Any] = {}
    try:
        if control:
            replies = await run_control(llm, turns)
            metrics |= behaviour(turns, replies)
        else:
            await ensure_database(EVAL_DSN)
            svc = await build_services(EVAL_DSN, llm)
            try:
                await reset_state(svc.pool)
                replies, manifests, ids = await run_harness(svc, turns, data.get("cycle_every", 3))
                metrics |= behaviour(turns, replies)
                metrics |= await state_metrics(svc, turns, replies, manifests, ids)
            finally:
                await svc.close()
    except CacheMiss as e:
        head = str(e).splitlines()[0]
        print(f"cache miss (prompts changed; re-record without --strict): {head}")
        print(e, file=sys.stderr)
        return 1
    finally:
        llm.save()
    metrics |= dict(llm.counts)
    metrics.setdefault("cache_misses", 0)
    metrics["seconds"] = round(time.monotonic() - t0, 1)
    metrics["turns"] = len(turns)

    print_table(metrics, base)
    bad = compare(metrics, base)
    for line in bad:
        print("REGRESSION", line)
    if label:
        out = ROOT / "results" / f"{name}.{label}.json"
        out.write_text(
            json.dumps(
                {
                    "scenario": scenario.stem,
                    "control": control,
                    "metrics": metrics,
                    "replies": [
                        {
                            "text": t["text"],
                            "reply": r,
                            "ok": judge(r, t) if t.get("tag") == "probe" else None,
                        }
                        for t, r in zip(turns, replies, strict=True)
                    ],
                },
                indent=1,
                ensure_ascii=False,
            )
        )
        print("wrote", out)
    return 1 if bad or (strict and metrics["cache_misses"]) else 0


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("scenario", type=Path)
    ap.add_argument("--label", help="write evals/results/<scenario>.<label>.json")
    ap.add_argument("--strict", action="store_true", help="fail on any LLM cache miss")
    ap.add_argument("--baseline", type=Path)
    ap.add_argument("--control", action="store_true", help="stateless transcript-stuffing bot")
    a = ap.parse_args()
    sys.exit(asyncio.run(run_scenario(a.scenario, a.label, a.strict, a.baseline, a.control)))


if __name__ == "__main__":
    main()
