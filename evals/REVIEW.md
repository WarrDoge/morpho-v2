# Behavior audit — 2026-09-12

**Verdict: partially achieved.** Durable state, inspection, and automatic idle revision work in the tested cases. Reliable goal tracking and useful improvement from self-revision are not established. All three full scenarios failed during reflection at the configured completion limit.

Evaluated the current working tree based on `1a6e021d454b30d80bf4f0c75766223d6d72004d`, including the pre-existing entity-update changes. Product behavior was not modified during this audit. Added opt-in diagnostic capture, a live probe driver, and exact reply/metric checks for completed v2 replays.

[Machine-readable measurements and assessments](results/audit.summary.json). Raw databases, settings, and step captures are retained locally under `evals/audit/` and ignored by Git. `audit/product.diff` records the pre-existing product changes.

## Verification and scenario results

- **32 offline tests pass**, including FIFO, cancellation/restart, duplicate replies, atomic rollback, migration fixtures, and the new observational-capture test. Formatting and Clippy with warnings denied pass.
- Three historical transcript-control baselines replay successfully. A three-turn cached smoke run reproduced identical replies and deterministic metrics with zero live calls.
- No full v2 recording completed. Strict replay of each partial recording reaches the missing reflection response, at calls 35, 19, and 48 respectively. The historical v1 harness baselines remain excluded because their prompts were replaced.

Each existing scenario was resumed once, sequentially, with existing caches and default provider/token settings. All stopped with `completion reached MAX_COMPLETION_TOKENS` in **Reflection**, not a transport timeout in these attempts.

| Scenario | Completed inputs | Completed keyword probes | Elapsed | Reported live input/output tokens |
|---|---:|---:|---:|---:|
| Dana | 21/30 | 3/3 of 10 total | 341 s | 41,083 / 7,410 |
| Project | 12/30 | 1/1 of 8 total | 303 s | 21,723 / 4,905 |
| Long | 35/91 | None of 30 reached | 1,201 s | 87,948 / 19,501 |

Only four of the 48 scenario probes were reached; three of their replies came from cache. These partial scores are not complete benchmark accuracy. Dana's fresh recall probe correctly named Lisbon and Nimbus but additionally asserted that the job had already started, which the supplied facts did not establish.

For all three failures, the cognitive state before and after failed maintenance was identical. Every completed input retained its stored reply: 21/21, 12/12, and 35/35. Maintenance reservations remained durable. No paid retries followed these failures.

## Core goals

| Goal | Assessment | Evidence and limit |
|---|---|---|
| Durable continuity from state | Achieved in tested cases | Exact state survived reopening; duplicate requests made no extra calls. Dana recalled old facts beyond the recent-event window. Full long-scenario recall remains unverified. |
| Useful idle self-revision | Partial | Automatic reflection identified a real missing-goal-record failure and changed the self-model without another user message. The controlled comparison did not demonstrate consistent response improvement. |
| Traceability and inspection | Achieved mechanically | All accepted proposal-level evidence IDs resolved in the scenario captures; rejection reasons and transitions are inspectable. Real evidence IDs do not guarantee that a claim follows from the evidence. |
| Bounded inference cost | Partial | Completion limits, durable maintenance reservations, and offline budget guards work; idle inference stopped when state was unchanged. The default completion allowance prevented every full scenario from finishing. Cost-effectiveness against transcript controls remains unmeasured. |

## Focused live probes

The probes used **21 fresh completion calls** against the same configured provider: nine for continuity/idle behavior and twelve for the paired comparison, within the 24-call allowance.

**Continuity and goals.** Two speakers had different dogs named Rex. Bob's Madrid/poodle answer and Alice's Berlin/basset-hound answer were correct, including Alice's correction and a new session after database reopening. All eight duplicate requests returned identical replies without additional inference.

However, the structured goal check **failed**. Goal creation supplied the string `medium` where a numeric priority was required. Completion targeted nonexistent `goal_vet_appointment_rex`. Both proposals were rejected, yet the replies said the appointment was marked done and the list was empty. The goals table remained empty. Other rejected memory proposals included a missing summary and importance outside the accepted range. Turn 3 also exposed internal-looking JSON in the response.

Automatic idle reflection then identified that the assistant had reported an empty to-do list without persisted task records. Self-state advanced from version 1 to 2. Four further seconds of worker observation produced no additional inference. This demonstrates useful error detection, not that the error was repaired. [Raw continuity evidence](audit/continuity/steps.jsonl).

**Controlled self-model comparison.** Used Dana's first relevant reflection, after turn 6. Verified that paired prompts differed only in the self-model section; input, other context, model settings, and schema were fixed. Conditions alternated order across three repetitions per probe. These were direct model calls at the production prompt boundary; proposed changes were not committed.

| Probe | Previous self-model | Revised self-model |
|---|---|---|
| “What should I focus on next?” | Anmeldung ranked first in 2/3 responses | Ranked first in 3/3 responses |
| “Have I finished the Anmeldung?” | 2/3 gave a grounded answer; one was whitespace | 2/3 gave a grounded answer; one invented a failed appointment |

The revised condition's unsupported reply began, “the appointment didn't work out then.” No failure had been reported. The whitespace control response would be rejected by the production turn handler. Prioritization changed, but this small experiment does **not** establish improved reasoning. [Paired prompts and all responses](audit/causal/steps.jsonl).

## Other specification goals

- **Consolidation:** Some genuine reduction occurred. Dana's turn-15 maintenance reduced active memories from 14 to 10 and their summary text from 3,163 to 2,027 characters. Nevertheless, complete structured state was not smaller than the transcript in these prefixes.
- **Compactness:** Active state serialized as UTF-8 JSON measured 20,386 / 12,460 / 19,747 bytes for Dana/project/long, versus transcript text of 8,733 / 4,171 / 10,699 bytes: **2.33× / 2.99× / 1.85×**. This includes row metadata and provenance and excludes retired memories/goals, resolved predictions, and expired relationships. Database/journal growth is measured separately. The legacy compactness metric counts only memory summaries, belief propositions, and goal descriptions.
- **Corrections and beliefs:** Rex's corrected breed was recalled, but all three final prefixes contained zero explicit beliefs. Confidence revision is therefore unverified; a null `revision_rate` can evade the old regression comparison.
- **World model:** Project retained zero entities after twelve inputs. Team-member updates and relationships were rejected because the referenced entities did not exist, despite correct early recall from other context.
- **Grounding:** Dana stored a September 15 completion date for “done today” despite the evaluator's fixed September 11 clock. This occurred in cached v2 output. Provenance was retained, but the date was unsupported.
- **Predictions:** The prefixes retained 17/9/15 unverified predictions, including repetitive forecasts. Reflection sees overdue predictions but not the complete outstanding set. The fixed scenario clock never advances to their deadlines; objective calibration and improvement remain unverified.
- **Context:** Observed peak context estimates were 2,505/1,908/2,164 tokens, below the 4,000-token budget. Improving selection over time has not been demonstrated.
- **Recovery:** Fixture migration/replay checks pass. Migration of real legacy data remains unverified because no dataset was supplied.

## Follow-up priorities

1. Make reflection's work and output fit the configured completion allowance, then complete the three scenarios before claiming v2 quality or efficiency gains.
2. Align proposal instructions/schema with validator types and ranges. Make user-visible state-change claims agree with accepted changes; currently rejected proposals can coexist with success wording.
3. Prevent unsupported dates/outcomes from becoming asserted memories. Give reflection visibility of outstanding predictions and assess redundant additions.
4. Repeat the controlled behavior comparison after those changes, grading grounded answers and actual committed goals alongside keyword scores.

## Reproduction and measurement limits

```sh
cargo fmt --check
cargo clippy --all-targets --locked --offline -- -D warnings
cargo test --release --locked --offline
cargo build --release --locked --offline --bin eval --example audit
target/release/eval evals/scenarios/dana.json --label v2 --audit-dir evals/audit/dana-next
IDLE_REFLECT_SECONDS=1 target/release/examples/audit continuity evals/audit/continuity-next
target/release/examples/audit causal evals/audit/causal-next evals/audit/dana-preflight
```

Use new output directories; replace Dana with project/long for the other scenarios. Successful recordings should then be replayed with `--strict` and their v2 baseline.

Scenario token estimates include cached calls; provider counters cover only this invocation's live calls, including reported truncated completions. They are not complete scenario billing totals. Embeddings are counted separately. Supplemental probes bypass the cache. The continuity probe overlapped the tail of long, so elapsed times are diagnostic observations, not controlled performance comparisons. Semantic review covered supplied scenario facts and state-change claims; external advice in responses was not fact-checked. The causal experiment used one checkpoint and two probes, not a statistical demonstration of general improvement.
