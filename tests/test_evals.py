"""Replays each recorded eval baseline in strict mode; a structural refactor must reproduce it."""

import json
from pathlib import Path

import pytest

import evals.run as run
from evals.run import ROOT, run_scenario

BASELINES = sorted((ROOT / "results").glob("*.base.json"))


@pytest.mark.parametrize("baseline", BASELINES, ids=[b.name for b in BASELINES])
async def test_replay_matches_baseline(baseline: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(run, "EVAL_DSN", run.EVAL_DSN + "_test")  # never the shared eval db
    data = json.loads(baseline.read_text())
    scenario = ROOT / "scenarios" / f"{data['scenario']}.json"
    cache = ROOT / "cache" / f"{data['scenario']}{'.control' if data['control'] else ''}.json"
    if not cache.exists():
        pytest.skip(f"no cache for {baseline.name}")
    assert await run_scenario(scenario, None, True, baseline, data["control"]) == 0
