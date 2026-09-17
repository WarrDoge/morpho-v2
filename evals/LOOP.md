# The iteration loop: where a recording spends its time and tokens

Measured on the v8 recordings (2026-09-17) from the OpenTelemetry metrics (`llm.calls`,
`llm.duration`, `llm.prompt_tokens` by `run`, `kind`, `model`) and the result files. morpho is
the harness; a morphling is the personality it grows. Every number below is one recording.

## Anatomy of persona-long.v8 (128 turns, 169 min, 1.35M prompt tokens)

| call | calls | wall | p50 / p90 per call | prompt tokens | completion |
| --- | ---: | ---: | ---: | ---: | ---: |
| reply (`text`) | 130 | 30 min | 9.5 s / 36 s | 800k | 16k |
| clerk (`Changes`) | 128 | 44 min | 17.8 s / 47 s | 333k | 36k |
| reflection | 11 | 9 min | 42 s / 74 s | 75k | 5.5k |
| consolidation | 11 | 4 min | 23 s / 45 s | 10k | 3k |
| judge (DeepSeek, 3 votes) | 480 | 74 min | 7.9 s / 21 s | 131k | 99k |
| embeddings | 319 | ~7 min | | | |

Model calls account for 9,693 of the 10,131 seconds: the run is provider latency, sequential.
The turn phase is 87 minutes, the judge phase 74. The other v8 runs have the same shape (dana
35 min, project 46, persona 62 with its judge, long 71).

A reply prompt is about 282 tokens of policy, 380 of IDENTITY (22 traits), 300 of narrative
and a 4,000-token context that fills to about 3,600: memories 1,315, journal 553, WHAT I SAID
461, working state 238, world 237, self 207, recent events 180, beliefs 174, predictions 156,
goals 57. The clerk prompt is about 2,600 tokens: its instructions, an index of what the reply
saw and the exchange.

Provider facts that close three doors: DeepInfra reports `cached_tokens: 0` on an identical
repeated prompt, so prompt ordering for prefix caching buys nothing; the same strict-schema
clerk call took 6 s and 34 s on consecutive tries, so variance, not decoding, sets the wall
clock; reasoning tokens are under 30 per call at `reasoning_effort: low`, so thinking and
schema settings are not levers either. At DeepInfra's GLM-5.3-Flash price a full v8 round
(3.4M tokens) costs well under a dollar; the cost of a round is hours, not money.

## Levers landed

1. **Judge calls run sixteen at a time and voting stops at a majority** (`src/bin/eval.rs`,
   `judged` and `verdict`). A live rejudge of persona.v8 made 82 calls in 37 s with the same
   verdicts the recording reached with 120 sequential calls in 18.5 min. For persona-long that
   is about 330 calls in 3 to 4 minutes instead of 480 in 74. Existing judge caches still hit:
   early stopping only asks for fewer of the votes they hold.
2. **Recompose: context selection without the model.** `compose` is a pure function of the
   state at a turn and the turn's embedding, and the journal holds every state.
   `just replay <scenario>` keeps the journal (`--db evals/cache/<scenario>.v8.db`);
   `just recompose <scenario>` folds it turn by turn, recomposes with the current composer and
   prints `retrieval_hit`, `ctx_tokens`, tokens per section and the probes whose memories
   missed the prompt. It reproduces the recorded numbers exactly on all five scenarios and runs
   in 1 to 4 seconds for no tokens. Ranking, budgets, shares and `MORPHO_DROP_STREAMS` are now
   compared this way; only the winner is re-recorded. `CONTEXT_SCORE_FLOOR` (default 0) was
   added as the first knob to sweep.

## What the free sweeps say about spending tokens rationally

`retrieval_hit` on long (25 probes with source references), context tokens per turn in
parentheses:

| knob | long | dana | project |
| --- | --- | --- | --- |
| budget 4000 (recorded) | 0.92 (3,447) | 0.78 (2,472) | 0.75 (2,727) |
| budget 3000 | 0.84 (2,718) | 0.78 (2,338) | |
| budget 2000 | 0.68 (1,861) | 0.78 (1,728) | |
| budget 1000 | 0.12 (925) | 0.56 (896) | |
| floor 0.10 | 0.92 (3,410) | 0.78 (2,373) | 0.75 (2,680) |
| floor 0.15 | 0.88 (3,303) | 0.78 (2,261) | 0.75 (2,598) |
| floor 0.20 | 0.84 (3,020) | 0.78 (2,164) | 0.75 (2,533) |
| floor 0.30 | 0.56 (2,431) | 0.44 (1,836) | 0.50 (2,257) |
| drop `recent` | 0.92 (3,253) | 0.78 (2,064) | 0.75 (2,325) |

- The budget and a score floor are not free: long loses probes at 3,000 tokens or a 0.15
  floor for a 4 to 21 percent saving. The memories that carry the hits score 0.14 to 0.44
  while the included tail sits near 0.08, so a floor cuts hits before it cuts filler. The
  ranker (relevance × importance × recency × confidence × reinforcement; recency is inert
  under the pinned eval clock) is what decides the tail, and it is now tunable for free.
- Long's two misses (turns 59 and 83) have no memory at all: the clerk wrote none from those
  events. Every hit was the top or near-top candidate. Retrieval is clerk-bound, not
  composer-bound; the composer cannot fix it and the clerk change needs a recording.
- `recent` (the last raw events) is the one cut that costs no retrieval here, saves 6 to 17
  percent of the context, and was recorded live once before: the v3 `drop-recent` runs kept
  probe accuracy (dana 0.9 → 1.0, long 0.97 → 0.97) with the two-exchange tail in place. It
  is the first v9 candidate.

## Proposed, not built

- **Hedged requests.** p90 is 3.8× p50 on the reply and 2.6× on the clerk. A second request
  after about twice the p50, first answer wins, should take a quarter off the turn phase for
  about a tenth more tokens; the recording keeps whichever answer was used, so replay is
  unaffected. Measure on one dana recording before adopting.
- **Trial spread is now affordable.** With the judge at minutes, a persona-long recording is
  about 90 minutes and 1.2M tokens; three seeded trials run in parallel finish in under two
  hours, which is what the stance-after and attention families need before any prompt change.
- **Clerk recall.** The retrieval ceiling is what the clerk chooses to keep. A recording with
  a clerk line that asks for one memory per new user fact, judged by `retrieval_hit` and
  `noise` together, is the experiment; recompose cannot run it.
- **Ranker experiments through recompose.** Relevance-only ranking, importance as a
  tie-break, and a per-section cap are each a one-line change and a four-second measurement.
