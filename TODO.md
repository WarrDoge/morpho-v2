# Resume checkpoint

Work parked on 2026-09-11. Implementation is in progress; live validation is unfinished.

## Intent and decisions

Preserve one evolving agent whose state causally influences later reasoning, including idle
self-revision. Multiple users feed one durable FIFO; speaker attribution does not create separate
agents or isolate knowledge. Optimize simplicity, token usage, reliability, and autonomy.

Chosen storage: local embedded libSQL, native exact cosine search, one coordinator process,
privately staged turns, atomic state/reply/completion publication. Kafka, distributed writers,
approximate indexing, cloud replication, and external actions remain deferred.

## Implemented

- libSQL records, vectors, inbox, retry status, embedding metadata, and maintenance budgets.
- Exclusive process ownership, serial turns, idempotent request IDs, restart recovery, and
  HTTP request-status/usage endpoints. Failed inputs remain durable and pause after bounded retries.
- One structured interaction call returns a reply plus native JSON state proposals; overlapping
  extraction agents were removed. Flat and nested state patches share validation and permission guards.
- Bounded idle reflection sees transitions and runtime failures; paused input does not prevent
  revision of committed state or expose its unconsumed text.
- Read-only legacy journal/vector import, historical replay fixes, correct context manifests,
  provider usage counters, model/settings-aware v2 cache keys, and deterministic evaluation time/IDs.
- The full original conversation is saved locally in `CONVERSATION.md`. It and `SPEC.md` are ignored;
  `SPEC.md` is untracked but retained locally. These files intentionally will not be on the remote branch.

## Checkpoint validation

Formatting, Clippy with warnings denied, and all 30 offline tests passed after the final flat-patch
fix. The test suite includes the historical control replay gate. The live Dana run was explicitly
stopped when work was parked; no live benchmark is intentionally left running.

## Pending work

- [ ] Finish the live Dana evaluation. The last run was stopped at 9/30 completed inputs;
  no `evals/results/dana.v2.json` was produced. Partial recordings remain in the ignored
  `evals/cache/dana.v2.json`; reuse matching entries rather than deleting the cache.
  Earlier attempts exposed malformed JSON-encoded payload strings and missing `patch` wrappers;
  both were fixed. The final run used native JSON objects and the shared patch-normalization fix.
- [ ] Investigate the remaining live rejection: one `upsert_entity` proposal lacked `name`.
  Inspect its target/payload and decide whether it is an invalid proposal or a legitimate update
  of an existing entity. Keep validation strict; do not silently invent a name. Add a regression
  test for any resulting fix. Consider exposing rejected proposal decisions to reflection if needed.
- [ ] Verify completion-limit handling in a live run: truncated provider responses now fail explicitly.
  If it triggers, measure the output requirement before adjusting `MAX_COMPLETION_TOKENS` (2048).
- [ ] Record and compare v2 answer quality and token use against the historical baselines.
  Start with Dana; then evaluate project/long for broader coverage. These runs make billable calls.
  Distinguish estimated prompt tokens from provider-reported input/output tokens; historical
  baselines do not contain equivalent billed completion-token measurements.
- [ ] Strictly replay completed v2 recordings and retain their result artifacts. The current replay
  gate preserves the three historical transcript-control baselines but deliberately skips v1
  harness baselines whose prompts were replaced. Do not describe this as all six old baselines passing.
- [ ] Validate useful idle self-revision with a live model. The deterministic causal test proves that
  revised state affects the next response, not that a live model reliably improves its reasoning.
- [ ] Finish the implementation review after live findings: retry/publication boundaries, API
  compatibility, maintenance spending and no-op behavior, and migration on a copy of real data.
  Production data has not been migrated or a service deployed. State projection cloning and exact
  scans remain deliberate scaling limits; optimize only after measurement.

## Commands

Offline checks (no paid calls):

```sh
cargo fmt --check
cargo clippy --all-targets --locked --offline -- -D warnings
cargo test --release --locked --offline
```

Resume Dana after building the current code (billable cache misses):

```sh
cargo build --release --locked --offline --bin eval
target/release/eval evals/scenarios/dana.json --label v2 --baseline evals/results/dana.base.json
```

Once its v2 result exists:

```sh
target/release/eval evals/scenarios/dana.json --strict --baseline evals/results/dana.v2.json
```

Historical baselines and caches should remain intact. A prompt or state-behavior change can
legitimately invalidate subsequent cached calls; do not weaken strict replay to hide misses.
