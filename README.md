# morpho

Continuous-state agent harness: the LLM reasons from a persistent, versioned cognitive state
(memories, beliefs, goals, world model, self model, predictions, working state) instead of a
growing transcript. See `SPEC.md` for the architecture.

## Run

```
cp .env.example .env     # set DEEPINFRA_API_KEY
just run                 # serves http://127.0.0.1:8000, state in ./data (MORPHO_DATA_DIR)
curl -s localhost:8000/interact -H 'content-type: application/json' -d '{"text": "Hi, I am Dana."}'
```

The background worker runs in-process (`WORKER_INPROCESS=true` by default) and polls every
`WORKER_POLL_SECONDS`. `POST /admin/cycle?reflect=true` forces consolidation + reflection now.

## Storage

One data directory, two files. `journal.jsonl` is append-only and is the only source of truth:
events, commits (proposal + transitions with before/after images + vector slots), cursor moves and
snapshots, one JSON record per line, synced on every append. `vectors.f32` holds embeddings at a
fixed stride and is memory-mapped for the cosine scans. All derived state is folded from the journal
into memory at startup; the same `apply` runs for a live commit and for the fold, so the journal is
the audit log, the crash-recovery source and the time-travel replay (`snapshots::replay`) at once.
A torn final line is dropped on open.

## Inspect

`GET /state /events /memories /beliefs /goals /predictions /entities /relationships /proposals
/transitions /snapshots`, `GET /why/{id}` (provenance chain), `GET /context/preview?text=...`,
`POST /admin/cycle?reflect=true`.

## Guards in the state engine

- Near-duplicate `create_memory` / `create_belief` (cosine >= `DEDUPE_THRESHOLD`, default 0.95)
  is folded into the existing row as a reinforcement; the proposal is recorded with reason
  `folded into <id>`. A `create_goal` whose description matches a live goal is rejected.
- `set_working_state` and `update_self_state` are patch merges with optional `expected_version`.
- Worker cycles are single-flight (a cycle that finds one running returns `{}`). A consumer batch
  that fails is retried on the next cycle and skipped after `CONSUMER_MAX_FAILURES` (default 3).

## Evaluate

```
just record dana                     # live: runs the scenario and records every LLM call
just eval dana --control --label base
just replay dana                     # strict replay against evals/results/dana.base.json
just gate                            # all six baselines
```

Scenarios are JSON turns tagged `fact | update | distractor | goal | probe`; probes carry `expect`
(AND of OR keyword groups), `reject`, and `refs` (planting turns). Every LLM call is cached in
`evals/cache/` (git-ignored, ~17 MB) keyed by prompt, and ids are seeded, so a replay is free and
deterministic. A structural refactor that leaves prompts unchanged must pass `--strict --baseline`
(zero cache misses, no metric worse); a prompt change misses the cache and needs a live re-record.
`--control` runs a stateless transcript-stuffing bot on the same probes under the same token budget:
the harness should match it on the 30-turn scenarios and beat it on `long`. `cargo test` replays
every `evals/results/*.base.json` whose cache is present.

## Develop

```
just check     # fmt --check, clippy -D warnings, unit + integration tests, replay gate
```

The toolchain is pinned in `rust-toolchain.toml`; `rustup` installs it on first use.
