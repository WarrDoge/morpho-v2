# morpho

One persistent agent shared across users. Memories, beliefs, goals, world state, predictions, and
an operational self-model influence each new response. Idle revision can change that state without
a new user message; the aim is observable causal continuity, with bounded inference cost.

## Run

```sh
cp .env.example .env     # set DEEPINFRA_API_KEY
just run
curl localhost:8000/interact -H 'content-type: application/json' \
  -d '{"request_id":"alice-1","speaker":"alice","text":"I moved to Berlin."}'
```

`MORPHO_DATA_DIR` defaults to `./data`; `BIND` defaults to `127.0.0.1:8000`.
All speakers share one agent and its knowledge. `speaker` provides attribution, not authentication
or data isolation. `speaker`, `session_id`, and `request_id` are optional for existing clients;
use a stable request ID when retrying. Reusing an ID with different contents returns HTTP 409.

## Durable turns

Inputs enter a libSQL inbox before any model call. One coordinator consumes them in arrival order,
one message per turn. A turn stages its state privately, generates the reply and state proposals
in one structured model call, validates the proposals, and records their outcomes. Rejections or empty drafts trigger one extra reply-only call;
if that call fails, a deterministic response reports saved and failed updates. The records, vectors,
outcomes, reply, and completion commit together. Interaction results include `state_changes`. Readers see committed state; future queued input never leaks into prompts.

A duplicate request returns its stored reply. A disconnected client does not remove its input;
the coordinator resumes pending work. Crashes may repeat model calls, but cannot publish half a
turn. The database directory has an exclusive process lock; run one service instance per directory.

`POST /interact` normally waits for the reply. HTTP 202 means the request remains queued;
`GET /requests/{request_id}` returns its status, error, attempts, and stored reply. Failed input
retries after 30 seconds, up to `CONSUMER_MAX_FAILURES` (default 3), then pauses the FIFO.
Resubmit the same request ID and contents to retry it. Later requests remain durable while paused.

## Storage and upgrades

`morpho.db` is an embedded libSQL database with WAL and full synchronous commits. It stores ordered
state records, native vectors, inbox results, and maintenance budgets. Derived state is folded
into memory; private turns currently clone that projection. Exact cosine scans are sufficient for
this version. Add indexed retrieval or incremental projections only when measured latency or
memory use warrants them.

Stop the old service before upgrading. On first open, Morpho imports complete `journal.jsonl`
records and `vectors.f32` slots transactionally. It leaves both source files intact. An incomplete
final journal/vector tail is ignored; malformed complete records or missing referenced vectors
abort the import. Subsequent starts use the database. The embedding model and dimensions are
recorded and must match configuration; changing models requires explicit re-embedding.

## Memory streams

Each request receives a fresh projection of observations, memories, beliefs, world knowledge,
goals, predictions, operational self state, and working state. Observations are immutable;
derived objects retain versions and provenance. Goals persist through completion, while working
fields can be replaced. System instructions stay constant; the serialized user message carries
`request` and fallible `recalled_state` data.

Selection uses input relevance, speaker attribution, active entities, and recent outcomes. The
context manifest explains selected IDs, versions, evidence, scores, and budget omissions in every
stream. Predictions receive 5% of the context budget; unused shares are redistributed, with goals
first. `GET /context/preview?text=...&speaker=alice` uses the same selector without appending input.

## Idle revision and limits

Reflection reads state, observed transitions, and runtime failures. It can revise beliefs,
predictions, open questions, and the operational self-model. Consolidation merges actual redundancy
and archives stale memories. Both run through the same coordinator, behind ready input, when
new evidence, internal state changes, or due predictions justify work. Unchanged work is not
repeated automatically. Reflection reads at most ten events and ten transitions per batch, sends compact changes rather
than full historical rows, and returns at most three proposals plus a continuation flag. Batch
selection is persisted before inference and resumes after restart. Consolidation checkpoints
separately, so a reflection failure preserves completed consolidation. Failed maintenance retains
its batch for retry; rejections and continuations without progress obey the same retry limits. Paused input does not
block revision of already committed state; its unconsumed text remains outside the context.

Self-revision cannot grant capabilities or permissions. Non-harness updates require existing
evidence IDs. Goal completion and prediction verification require a new user observation beyond
the original intent/forecast; this structural check does not establish semantic truth. A passed
deadline alone leaves predictions unknown. Exact unresolved forecast duplicates are rejected. Text-identical
memories/beliefs can reinforce existing objects; vector similarity alone never establishes that two
claims are equivalent. Every proposal records its decision and any supplied rationale.

Defaults: `CONTEXT_TOKEN_BUDGET=4000`, `MAX_PROMPT_TOKENS=12000`,
`MAX_COMPLETION_TOKENS=2048`, `BACKGROUND_DAILY_TOKEN_BUDGET=100000`,
`IDLE_REFLECT_SECONDS=300`, `REFLECT_EVERY_N_EVENTS=10`. Context/input estimates use characters;
completion requests also carry the provider's output limit. Maintenance reserves its maximum
estimated call cost durably before inference, retaining reservations after failures or crashes.
`GET /usage` reports process-lifetime counts, including provider-reported input/output tokens
separately from estimates. Embedding usage is represented by call count, not included in this text-token budget.

`WORKER_INPROCESS=false` disables automatic maintenance; durable inbox processing still runs.
`POST /admin/cycle?reflect=true` requests maintenance immediately, subject to coordinator ownership
and its daily budget. External actions are deferred.

## Inspect and verify

`GET /state /events /memories /beliefs /goals /predictions /entities /relationships /proposals
/transitions /snapshots /usage`, `GET /why/{id}`, `GET /context/preview?text=...`.

```sh
just check
just record dana                    # paid/live v3 recording
just eval dana --label v3
just eval dana --control --label base
```

For diagnostics, pass `--audit-dir evals/audit/dana-run` to the evaluator. The directory must
be new; it retains the database, per-turn/cycle state, context manifests, usage, and any failure.
Capture does not modify prompts or state. Raw audit directories are ignored by Git.
Completed v3 recordings also require identical replies and deterministic metrics in the replay gate.

Opt-in live probes (billable; never run by `cargo test`):

```sh
IDLE_REFLECT_SECONDS=1 cargo run --release --example audit -- continuity evals/audit/continuity-run
cargo run --release --example audit -- causal evals/audit/causal-run evals/audit/dana-run
```

The continuity probe reopens its database, checks duplicate replies, and observes automatic idle
maintenance. The causal probe uses the first captured Dana operational self revision,
changes only the self-model section of paired prompts, and makes twelve fresh completion calls.
Continuity caps completion calls at 32 (including correction calls and multiple idle batches); the
causal probe caps them at twelve. Both stop on provider failure.

Tests cover FIFO/concurrent callers, cancellation and restart, duplicate replies, atomic rollback,
legacy migration, historical replay, context manifests, maintenance limits, and a deterministic
causal test: changing only the self-model changes the next response to the same input.
The causal fixture tests the harness, not live-model reasoning quality.

The three historical transcript-control baselines retain their strict replay gate. The old v1/v2
harness baselines and caches remain historical comparisons; their extraction prompts were replaced.
New harness caches use `.v3.json`, include model/settings in their keys, and preserve literal
dates. The scenario runner uses fixed request IDs and a fixed `start_time` (default
`2026-09-11T12:00:00Z`) so state and deadlines replay exactly. Live re-recording is required to compare v3 answer quality and
billed token cost against those baselines. `cargo test` never makes paid model calls.
