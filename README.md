# morpho

One persistent agent shared across users. Memories, beliefs, goals, world state, predictions, and
an operational self-model influence each new response. Idle revision can change that state without
a new user message; the aim is observable causal continuity, with bounded inference cost. The
harness is morpho; the personality it grows from seeds and evidence is a morphling, a bot with a
durable disposition, not a person.

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
one message per turn. A turn stages its state privately and makes two model calls: a plain-text
reply call (policy, IDENTITY, recalled state) and a strict-schema clerk call that receives the
request, the reply and an index of what the reply saw (every id with a short label, plus the
working and self state) and returns the state changes the exchange warrants. The clerk records
evidence a speaker reports as they stated it, attaches it to the trait it bears on, and never
changes a confidence because something was repeated. A cut or empty draft is resampled up to
twice; if every draft is unusable, a short notice is returned and no clerk call is made. The
records, vectors, outcomes, reply, and completion commit together. Interaction results include `state_changes`. Readers see committed state; future queued input never leaks into prompts.

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

Recent observations are only the current session's last two exchanges, in order; anything older
must come from state. WHAT I SAID recalls the agent's own earlier replies by similarity across
sessions, and MY NOTES its own reflection entries. Memories, beliefs and notes are ranked into one
pool with one shared allowance: a pooled item whose embedding lies within 0.92 cosine of a kept one
is omitted as a duplicate, or supersedes it when it was learned later, and memories linked to an
entity the input names score higher. When traits exist, the retrieval query for memories and
beliefs leans toward the identity centroid (`IDENTITY_BIAS`, default 0.2). Selection uses input
relevance, speaker attribution, active entities, and recent outcomes. The context manifest explains
selected IDs, versions, evidence, scores, and every omission with its reason. Unused shares are
redistributed, with goals first. `GET /context/preview?text=...&speaker=alice` uses the same
selector without appending input.

A turn is two model calls. The reply call gets the fixed policy, the IDENTITY block and the
recalled state and answers in plain text; it does not claim to save anything. The clerk call then
gets the same recalled state, the trait lines and the exchange and returns the state changes under
a strict schema (`CLERK_MODEL` may point it at a different model; empty means `LLM_MODEL`). A
reply that comes back cut mid-sentence, as a schema token or empty is resampled under a salted
request up to twice and the longest readable draft is kept; when none is readable the turn
returns a short notice and skips the clerk. A clerk failure fails the turn, so the inbox retries it.

## Identity

Dispositions live in a `traits` table: values, preferences, stances, style rules and per-speaker
relationship stances, each with confidence, evidence, origin (`seed` or `experienced`) and versions.
The IDENTITY block in the system prompt is normative for the reply, unlike recalled state, and has
four layers at three speeds. The narrative is a first-person self-description of at most 220 words
that the narrative agent compiles from every live trait, the agent's own goals and its latest notes;
it is a singleton keyed on the substance of the traits it was compiled from (status, wording,
contest), and maintenance recompiles it when a live trait was formed, reworded, contested or
retired, never for a confidence change alone. Traits in play are the live traits most similar to
the input, up to an eighth of the context budget; contrary evidence a trait carries reaches the
reply only through the narrative, which says where the agent is reconsidering (a prompt line that
named the contested trait measured worse and was removed). Lately is the latest journal entry,
one first-person note per reflection batch with a mood, written outside the batch's three-change cap. On my mind is the highest-priority goal of
origin `self` with the next step reflection gave it. `GET /traits`, `/why/{id}` and `/state`
expose them, and the context manifest lists the traits rendered. Seeds come from
`MORPHO_SEED_FILE` (a JSON array of `create_trait` payloads) or a scenario's `seed` array, applied
once to an empty store through a `seed` event. The clerk and reflection may create traits about
the agent itself (never a speaker's facts) and revise them; a new trait within 0.9 cosine of a
live one folds into it, rewording or lowering a trait needs a new contrary user observation, the
engine moves confidence down by at most 0.25 per observation and refuses to retire a trait above
0.3. Reflection may create goals of origin `self` when they cite a trait and may revise only
those. Ablate with `MORPHO_DROP_STREAMS=identity` (the whole block) or `narrative`, `journal`,
`wants` (one layer each).

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

## Observability

Every turn, model call, context composition, commit and maintenance cycle is a `tracing` span.
With `OTEL_EXPORTER_OTLP_ENDPOINT` set (`.env` or the command line) the spans and four metrics
are exported over OTLP/HTTP; unset, nothing leaves the process. `just otel` starts a local
`grafana/otel-lgtm` container (Grafana on :3000, OTLP on :4318).

Spans: `turn` (`request_id`, `speaker`, `session`, `seq`, `resamples`, `reply_chars`,
`changes.proposed/accepted/rejected`), `compose` (`tokens`, `included`, `omitted`,
`identity_traits`), `llm.call` (`kind` = `text` or the schema name, `model`, `prompt_tokens`,
`completion_tokens`, `cache` = `live`/`hit`/`fake`), `embed`, `commit` (`proposals`, `accepted`,
`rejected`, one `rejected` event per refused change with its reason), `cycle` (`stats`),
`reflection`, `consolidation`, `narrative`. The evaluator adds a root `eval` span, one
`eval.turn` span per scenario turn (`turn`, `tag`, `family`), a `probe` event per judged reply
(`ok`) and a `result` event with the metrics. Metrics, all carrying `run` = scenario and label:
`llm.calls`, `llm.prompt_tokens`, `llm.completion_tokens`, `llm.duration` (by `kind`, `model`,
`cache`) and `changes` (by `agent`, `operation`, `accepted`). Result files carry
`calls_by_kind` and `prompt_tokens_by_kind` for the same split without a backend.

```
{name="llm.call" && span.kind="Changes"}                              # TraceQL: clerk calls
{resource.run="persona-long.v8"}                                      # TraceQL: one run, live or replayed
sum by (kind, run) (increase(llm_prompt_tokens_sum[1h]))              # tokens per call kind
sum by (operation, run) (increase(changes_total{accepted="false"}[1h]))  # refused changes
```

A replay (`just replay persona-long`) exports the recorded run as cache hits, so a baseline
trace costs no tokens.

## Inspect and verify

`GET /state /events /memories /beliefs /goals /predictions /entities /relationships /proposals
/transitions /snapshots /usage`, `GET /why/{id}`, `GET /context/preview?text=...`.

```sh
just check
just record dana                    # paid/live v6 recording
just eval dana --label v6
just eval dana --control --label base
```

Experiment knobs: `just ablate dana recent` zeroes named context streams via `MORPHO_DROP_STREAMS`
(set it on the command line only, never in `.env`); `just control-full dana` gives the transcript
control the whole history; `just trial dana 2` samples a separate cache file. Scenario turns may
carry `speaker`, `session`, a `judge` rubric (graded PASS/FAIL by the same model) and a `group`
(replies sharing a group are judged pairwise for agreement, reported as `consistency`), and a
`family` for per-family accuracy. `JUDGE_MODEL` grades with a separate model and cache;
`--rejudge RESULT.json` re-scores an earlier result with it. Verdicts are a majority of
`JUDGE_VOTES`, voting stops once the majority is settled, and judge calls run sixteen at a time.

Context selection is a pure function of the recorded state, so it is tuned without the model:
`just replay dana` keeps the run's journal in `evals/cache/dana.v8.db` (`--db DIR`), and
`just recompose dana` (`--recompose DB`) recomposes every turn from that journal with the
current composer and prints `retrieval_hit`, `ctx_tokens`, tokens per section and the probes
whose memories missed the prompt, in seconds and for no tokens. Ranking, budgets, shares and
`MORPHO_DROP_STREAMS` are compared this way; only the winner is re-recorded.

For diagnostics, pass `--audit-dir evals/audit/dana-run` to the evaluator. The directory must
be new; it retains the database, per-turn/cycle state, context manifests, usage, and any failure.
Capture does not modify prompts or state. Raw audit directories are ignored by Git.
Completed v6 recordings also require identical replies and deterministic metrics in the replay gate.

Opt-in live probes (billable; never run by `cargo test`):

```sh
IDLE_REFLECT_SECONDS=1 cargo run --release --example audit -- continuity evals/audit/continuity-run
cargo run --release --example audit -- causal evals/audit/causal-run evals/audit/dana-run
```

The continuity probe reopens its database, checks duplicate replies, and observes automatic idle
maintenance. The causal probe uses the first captured Dana operational self revision,
changes only the self-model section of paired prompts, and makes twelve fresh completion calls.
Continuity caps completion calls at 32 (reply and clerk calls and multiple idle batches); the
causal probe caps them at twelve. Both stop on provider failure.

Tests cover FIFO/concurrent callers, cancellation and restart, duplicate replies, atomic rollback,
legacy migration, historical replay, context manifests, maintenance limits, and a deterministic
causal test: changing only the self-model changes the next response to the same input.
The causal fixture tests the harness, not live-model reasoning quality.

The three historical transcript-control baselines retain their strict replay gate. The old v1 to v4
harness baselines and caches remain historical comparisons; their prompts were replaced.
Harness caches use `.v6.json`, include model/settings in their keys, and preserve literal
dates. The scenario runner uses fixed request IDs and a fixed `start_time` (default
`2026-09-11T12:00:00Z`) so state and deadlines replay exactly. Live re-recording is required to compare v6 answer quality and
billed token cost against those baselines. `cargo test` never makes paid model calls.
