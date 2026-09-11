# morpho

Continuous-state agent harness: the LLM reasons from a persistent, versioned cognitive state
(memories, beliefs, goals, world model, self model, predictions, working state) instead of a
growing transcript. See `SPEC.md` for the architecture.

## Run

```
cp .env.example .env            # set DEEPINFRA_API_KEY
docker compose up -d db          # Postgres 17 + pgvector
uv sync
WORKER_INPROCESS=true uv run uvicorn app.api.main:app
uv run python scripts/chat.py    # REPL; "/cycle" forces consolidation + reflection
```

Separate worker: `uv run python -m app.workers.run` (drop `WORKER_INPROCESS`).

## Inspect

`GET /state /events /memories /beliefs /goals /predictions /entities /relationships /proposals
/transitions /snapshots`, `GET /why/{id}` (provenance chain), `GET /context/preview?text=...`,
`POST /admin/cycle?reflect=true`. Rebuild derived state from the transition log:
`uv run python -m scripts.replay`.

## Guards in the state engine

- Near-duplicate `create_memory` / `create_belief` (cosine >= `DEDUPE_THRESHOLD`, default 0.95)
  is folded into the existing row as a reinforcement; the proposal is recorded with reason
  `folded into <id>`. A `create_goal` whose description matches a live goal is rejected.
- `set_working_state` and `update_self_state` are patch merges with optional `expected_version`.
- Worker cycles are serialised with a Postgres advisory lock. A consumer batch that raises is
  retried on the next cycle and skipped after `CONSUMER_MAX_FAILURES` (default 3).

## Evaluate

```
uv run python -m evals.run evals/scenarios/dana.json --label base          # live, records LLM cache
uv run python -m evals.run evals/scenarios/dana.json --control --label base
uv run python -m evals.run evals/scenarios/dana.json --strict --baseline evals/results/dana.base.json
uv run python -m evals.gen_long                                            # regenerates long.json
```

Scenarios are JSON turns tagged `fact | update | distractor | goal | probe`; probes carry `expect`
(AND of OR keyword groups), `reject`, and `refs` (planting turns). Every LLM call is cached in
`evals/cache/` keyed by prompt, and ids are seeded, so a replay is free and deterministic. A structural
refactor that leaves prompts unchanged must pass `--strict --baseline` (zero cache misses, no metric
worse); a prompt change misses the cache and needs a live re-record. `--control` runs a stateless
transcript-stuffing bot on the same probes under the same token budget: the harness should match it on
the 30-turn scenarios and beat it on `long`. `uv run pytest` replays every `evals/results/*.base.json`.

## Develop

```
uv run ruff check && uv run ruff format --check && uv run ty check
uv run pytest            # needs the compose db; uses a fake LLM
uv run pytest -m live    # real DeepInfra: schema smoke + 10-turn scenario (minutes)
```
