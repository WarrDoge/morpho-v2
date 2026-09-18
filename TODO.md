# Resume checkpoint

Updated 2026-09-18 (round 3 of the workshop). v8 (judge temperature seam, clerk changes default their evidence to the
request event, cited evidence ids validated, contested line and `contest` knob removed, `run` on
every span) is recorded on five scenarios plus the `identity` ablation of persona-long; judge
verdicts are majority-of-three. `just otel` starts the local Grafana stack; set
`OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318` to export. The harness is morpho; a
personality it grows is a morphling.
Judge calls run sixteen at a time and stop voting at a majority; `just recompose <scenario>`
tunes context selection against a kept journal without model calls.
The workshop (`just workshop <scenario> <arm> <trial>`) gives the morphling sandboxed coding
tasks. Round 2 recorded two scenarios (`workshop`, `workshop-memory`) across `transcript`,
`morphling` and `morphling` with `MORPHO_DROP_STREAMS=practices`, three trials each: 15
recordings, all gated. Round 3 added `workshop-habit`, a curriculum built to make a habit pay:
6 more recordings, `morphling` with and without practices. Its floor guard tripped — the arm
without practices ran the audit after every task too — so practices still have no measured
value, at 1.10x the tokens, and were removed after it. Chat stays frozen at v8 and replays
byte-identical.
See [the ablation and persona report](evals/ABLATION.md), [the workshop report](evals/WORKSHOP.md), [the loop report](evals/LOOP.md),
[the streams report](evals/STREAMS.md) and [the earlier audit](evals/REVIEW.md).

## Intent and decisions

One evolving agent, one durable FIFO, one embedded libSQL database and coordinator.
Multiple speakers share knowledge with attribution. Eight memory views compose fresh context
for each request; fixed policy stays separate from recalled data. Privately staged turns
publish state, reply and completion atomically. No new infrastructure or dependencies.

`CONVERSATION.md` and `SPEC.md` stay local and ignored; neither belongs on the remote branch.
Artificial personality is now a goal (SPEC §40 non-goal struck 2026-09-15): dispositions are
`traits` rows rendered as a normative IDENTITY block; recalled state stays data.

## Completed

- [x] Preserve entity update-by-name/ID fixes and durable inbox/restart behavior.
- [x] Compose eight budgeted streams with IDs, versions, evidence, attribution and omission metadata.
- [x] Record actual proposal outcomes and correct rejected-update acknowledgments with one
  reply-only attempt and deterministic fallback; handle empty drafts through the same path.
- [x] Page reflection into persisted batches with at most three proposals, independent
  consolidation checkpoints, bounded retries and the existing daily spending limit.
- [x] Guard operational self patches, duplicate forecasts and outcome evidence.
- [x] Fix missing relationship endpoint names discovered by the live ownership probe.
- [x] Pass 40 offline tests, formatting and Clippy with warnings denied.
- [x] Harness knobs: `MORPHO_DROP_STREAMS`, `--trial`, per-turn `speaker`/`session`, `judge`
  rubrics and `group` consistency, full-history control. Tier-1 ablation ran: state carries
  continuity without the transcript once a two-exchange same-session tail is kept.
- [x] `traits` table, seeds, bounded-step revision rule, identity block, `seq` on memory and
  belief items; v4 baselines re-recorded; persona scenario: judge 0.95 / consistency 1.0 seeded
  versus 0.45 / 0.47 with identity ablated. 44 offline tests.
- [x] v5: WHAT I SAID stream, trait embeddings and `IDENTITY_BIAS`, core+relevant identity
  block, `origin: self` goals from reflection, `mood`, `JUDGE_MODEL` with `--rejudge` and
  `judge_by_family`, degenerate-draft resampling. persona-long (50 seeds, 114 turns): seeded
  0.93 / 0.99 over three trials, 0.79 / 1.0 with identity ablated, `said` family 0.67 with the
  stream ablated. Report in `evals/ABLATION.md` "Round 2".
- [x] v7: OpenTelemetry through `tracing` (spans on turn, compose, llm.call, embed, commit,
  cycle, eval; metrics by run and call kind; `calls_by_kind` and `prompt_tokens_by_kind` in
  result files); the evidence ledger. Report in `evals/ABLATION.md` "Round 4".
- [x] v8: `JUDGE_TEMPERATURE`, evidence default and id validation in the clerk path, contested
  rendering removed, `run` as a resource attribute. Scenarios frozen; persona-long seeded 0.83
  against 0.69 ablated, stance-after back to 0.2 / 0.8. Report in `evals/ABLATION.md` "Round 5".
- [x] Workshop: bubblewrap sandbox tools, `Act`/`Think` calls, surprise from stated expectations,
  observations as outcome evidence, idle ticks gated on the morphling's own agenda; four arms
  times three trials, hidden-test grading, no judge. No traits formed from work, a self-chosen
  think in 1 of 6 trials, idle work in 4 of 9 morphling runs without a write, 2 to 3 times the
  transcript agent's tokens per hidden pass. Report in `evals/WORKSHOP.md`.
- [x] v6: reply call plus clerk call (correction path removed), narrative singleton compiled by
  its own agent, `journal` table, `update_goal.next_step` and ON MY MIND, trait cosine fold and
  rewording gate, pooled memories/beliefs/journal with dedupe and entity boost, `decision`,
  `initiative`, `recall` families. persona-long: seeded 0.83 / 0.98 (decision 8/8), identity
  ablated 0.73 / 1.0; zero reply resamples in 405 turns; prompt tokens 1.9x. Report in
  `evals/ABLATION.md` "Round 3".

## Validation and follow-up

- [x] Complete continuity, all 151 scenario turns, and twelve paired causal calls; strictly
  replay three v3 recordings plus three historical controls. Probe scores: 9/10, 8/8, 29/30.
  Long maintenance paused within its daily budget with a durable backlog. Quality/cost
  limitations are recorded in `evals/STREAMS.md`.
- [ ] Improve semantic answer completeness, unsupported temporal/outcome claims and retrieval
  quality based on the recorded misses. Structural validation does not establish truth.
- [x] Trait cosine fold (0.9), always-present narrative for the unevoked-trait miss, plain-text
  reply contract (zero resamples in 405 turns), rubric 113 trimmed. Done in v6.
- [x] v7: the clerk sees an id index instead of the projection (prompt tokens down 15% to 27%
  per scenario, the clerk call at 60% of a reply call); narrative keyed on trait substance;
  journal outside the reflection cap; weekend rubric accepts a named tea; decision, initiative
  and stance-after probes rewritten with a methods turn after the study.
- [ ] Clerk refusals from the index (long: 20 of 46 are `update_memory` without evidence ids, 3
  are working-state patches copying `recent_events`): v8 defaults missing evidence ids to the
  request event; `recent_events` is still in the index. Reflection still tries to revise user
  goals (5 to 6 refusals per run).
- [x] The judge is not deterministic (9 of 12 failures flipped on a fresh pass; 3 of 78 at
  temperature 0), so verdicts stay a majority of `JUDGE_VOTES`; calls run sixteen at a time
  and stop at a majority (persona: 82 calls in 37 s, same verdicts as 120 sequential in 18 min).
- [x] Recompose: `--db` keeps a run's journal, `--recompose DB` recomposes every turn with the
  current composer for no tokens and reproduces the recorded `retrieval_hit` and `ctx_tokens`.
  Sweeps in `evals/LOOP.md`: budget and score floor cost long's probes, `recent` does not.
- [ ] v9 candidates from the loop report: drop `recent` from the reply context (context −6% to
  −17%, retrieval unchanged, v3 live runs kept probe accuracy); hedged requests for the p90
  tail; a clerk recall rule (long's two misses are memories the clerk never wrote).
- [ ] Trial spread before any prompt change: two more seeded v8 persona-long recordings (about
  2.4M tokens); the round-4 "contest line net negative" and the v8 stance-after 0.2 are single
  recordings each.
- [ ] Two harness loops replicate one turn's answer: the first stance-after reply (turn 72) is
  copied through WHAT I SAID, and working-state `open_questions` never expire, so the reply
  re-asks and counts deflections (73 of 110 patches, 15 nagging turns). Add an expiry and score
  stance-after on reasoning about the study, not movement (round 6, rubric freeze).
- [ ] The resampling path tests truncation only; one system-prompt leak (long #19) passed through.
- [x] Drop the `contested` prompt line: the ledger alone gives stance-after 1.0, the line gives
  0.6 (the agent hedges and keeps its framing). Removed in v8 with the `contest` knob; contrary
  evidence reaches the reply through the narrative only.
- [x] Workshop reads are cut at 3,000 characters like command output; both collapses (hidden
  0.03) followed `durations.py` passing that size. Reads now use the 32,000-char write cap and
  output stays at 3,000; no round-2 run fell below 0.92.
- [x] Work never reaches identity. A closed surprise or a check after a stall think asks for a
  lesson; a well-formed statement becomes a `practice` trait at 0.4, credit promotes it at 0.6
  with two supporting episodes, and promoted practices enter the narrative. Round 2 promoted 1,
  1 and 4 per trial on `workshop-memory`, with 2 to 17 cited actions per run.
- [x] A workshop curriculum where memory should pay: `workshop-memory` states a CSV convention in
  task 1 that only task 5 needs. Morphling passes it 3 of 3, the transcript agent 1 of 3.
- [ ] Recall is held between refreshes (`Desk.held`) because a memory shown in one Act prompt is
  gone from the next call. That one change moved `csv_convention` from 0 of 3 to 3 of 3; the
  round-1 "recall every step" cost is gone but the held block is still in every prompt. Measure
  whether a shorter held block (memories only, no identity) keeps the win.
- [x] A task where the fast wrong move is available and only a habit refuses it:
  `workshop-habit`, six tasks over a billing module whose house rules a `check.py` audit enforces
  and whose unit tests do not. Verified: a shortcut solution passes `python3 -m unittest` and
  fails `check.py` in six places.
- [x] Practices removed. Two curricula built to reward a habit measured no value: round 2 cost
  1.80x and round 3 cost 1.10x for the same score, and round 3's floor guard tripped because the
  arm without practices ran the audit after every task in 3 of 3. A stated rule becomes a memory,
  held recall carries it, and 6 or 7 memories never make recall choose, so a practice was a copy
  of what the memory stream already delivered.
- [ ] The workshop half of the replay gate is dormant: removing the `practice` field changed
  every workshop prompt, so the 21 round-2 and round-3 recordings cannot replay. They stay as the
  evidence behind the reports, carry `workshop_version: 2` and are skipped like pre-v8 chat
  baselines. The next workshop round records a `workshop_version: 3` baseline and relights it;
  until then `tests/workshop.rs` is the only cover for the workshop path.
- [ ] Idle returns to its open loop in 3 of 3 runs that have one, but closes none: closing needs a
  passing check and idle ends mid-repair. Give idle more steps, or count a compile-clean rewrite
  as progress.
- [ ] The stall detector fires once in 6 morphling runs, so thinking-under-stall is untested.
  Either loosen it (two failures, or a repeated command) or drop it.
- [ ] Next round on chat: expire open questions, then re-record v9. Round 2 deliberately left
  chat frozen, so the v8 baselines still replay byte-identical.
- [ ] Attention inverted: the seeded agent recalls the details that touch its own preferences
  (espresso, chocolate, the corgi), never the brother or the mother (0/3 vs 2/3 ablated). Decide
  whether the rubric or the salience is right before touching retrieval.
- [ ] Decision probes discriminate on 2 of 8 (privacy, types over tests); GLM already picks
  monorepo, train, memo and paid take-home unseeded. Write probes on stances it does not hold.
- [ ] Journal: 4 entries per 128 turns with the cap lifted (8 ablated); reflection skips it when
  merely allowed. Give it a dedicated call per cycle, and let it carry what stood out.
- [ ] Second-model check for v7 (DeepSeek-V4.1-Flash, 3.7 h, 1.5M tokens) not run; the contest
  and identity ablations are the controls this round.
- [x] Second-model check: DeepSeek-V4.1-Flash reproduces the stance-after cluster (0.4) and the
  saturated families, so they are harness effects; it needs `MAX_COMPLETION_TOKENS=8192` and
  `LLM_TIMEOUT_SECONDS=600` and runs about one turn a minute.
- [ ] Clock: `now` only helps once items carry dates; the eval clock is pinned, so add both together.
- [ ] Run `contradict` scenario for belief revision; probe reasoning under values (plan or
  choose, not only state a preference).
- [ ] Demonstrate useful live self-revision across multiple checkpoints and targeted probes.
- [ ] Test migration on a copy of real legacy data when a dataset becomes available.

## Commands

```sh
cargo fmt --check
cargo clippy --all-targets --locked --offline -- -D warnings
cargo test --release --locked --offline
cargo build --release --locked --offline --bin eval --example audit
```

Record a scenario (billable cache misses; use a fresh audit directory):

```sh
target/release/eval evals/scenarios/dana.json --label v4 --baseline evals/results/dana.v4.json --audit-dir evals/audit/dana-next
```

Strict replay after its result exists:

```sh
target/release/eval evals/scenarios/dana.json --strict --baseline evals/results/dana.v4.json
```
