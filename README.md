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
in one structured model call, validates the proposals, then commits the records, vectors, reply,
and completion together. Readers see committed state; future queued input never leaks into prompts.

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

## Idle revision and limits

Reflection reads state, observed transitions, and runtime failures. It can revise beliefs,
predictions, open questions, and the operational self-model. Consolidation merges actual redundancy
and archives stale memories. Both run through the same coordinator, behind ready input, when
new evidence, internal state changes, or due predictions justify work. Unchanged work is not
repeated automatically. Failed maintenance preserves its progress for retry. Paused input does not
block revision of already committed state; its unconsumed text remains outside the context.

Self-revision cannot grant capabilities or permissions. Evidence IDs must exist. Text-identical
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
just record dana                    # paid/live v2 recording
just eval dana --label v2
just eval dana --control --label base
```

Tests cover FIFO/concurrent callers, cancellation and restart, duplicate replies, atomic rollback,
legacy migration, historical replay, context manifests, maintenance limits, and a deterministic
causal test: changing only the self-model changes the next response to the same input.
The causal fixture tests the harness, not live-model reasoning quality.

The three historical transcript-control baselines retain their strict replay gate. The old v1
harness baselines and caches remain historical comparisons; their extraction prompts were replaced.
New harness caches use `.v2.json`, include model/settings in their keys, and preserve literal
dates. The scenario runner uses fixed request IDs and a fixed `start_time` (default
`2026-09-11T12:00:00Z`) so state and deadlines replay exactly. Live re-recording is required to compare v2 answer quality and
billed token cost against those baselines. `cargo test` never makes paid model calls.
