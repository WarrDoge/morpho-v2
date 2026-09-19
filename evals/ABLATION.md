# Ablation: state versus transcript (2026-09-15)

Question: does the persistent state carry continuity on its own, or was the RECENT EVENTS tail
(last ten raw events, 15% of the context budget in v3) doing the work? Single live run per cell,
GLM-5.3-Flash, bge-m3, `CONTEXT_TOKEN_BUDGET=4000`. Substring grading as in `eval.rs`.

| scenario | condition | probe acc | update acc | prompt tokens | completion calls |
|---|---|---|---|---|---|
| dana (30) | tail-4k control | 1.0 | 1.0 | 87,624 | 30 |
| dana | full-history control | 0.9 (1) | 1.0 | 115,115 | 30 |
| dana | state + 10-event tail (v3) | 0.9 | 1.0 | 172,165 | 55 |
| dana | state only (`recent` dropped) | **1.0** | 1.0 | 125,853 | 56 |
| project (30) | tail-4k control | 1.0 | 1.0 | 89,568 | 30 |
| project | full-history control | 1.0 | 1.0 | 153,863 | 30 |
| project | state + 10-event tail (v3) | 1.0 | 1.0 | 188,200 | 58 |
| project | state only | 0.75 (2) | 0.5 | 119,980 | 54 |
| long (91) | tail-4k control | 0.7 | 1.0 | 311,082 | 91 |
| long | full-history control | 0.9667 | 1.0 | 875,582 | 91 |
| long | state + 10-event tail (v3) | 0.9667 | 1.0 | 470,843 | 128 |
| long | state only | 0.9667 (3) | 1.0 | 386,040 | 122 |

(1) The one dana full-history miss is a grading artifact: the model correctly said it had
nothing on the football team but phrased it as "there's nothing in our conversation", which the
substring list does not include.

(2) Project state-only: turn 20 is a grading artifact (the reply said "moved from end of Q3 to
mid-October", and the reject term precedes the answer). Turn 24 is a real miss: the 35% figure
stated one turn earlier was never stored in state, and without a tail the reply said so. Facts
said seconds ago need working memory, not long-term state.

(3) Long state-only misses the same count as v3 but a different probe: turn 66's reply is a
header with no list ("Here are the people I know about in your life, Noor:"), a truncated
generation rather than a retrieval failure. Noise rose to 0.52 (v3: 0.33): more distractor-only
memories survive, which does not affect probes but does affect state quality.

## Reading

- On the 30-turn scenarios the state harness is no better than a 4k transcript tail and costs
  more tokens. State pays off only past the budget: on the 91-turn run the tail control loses
  nine facts, state loses one, and full history matches state at 1.9x the prompt tokens.
- Dropping the tail did not hurt dana (it improved, 27% fewer tokens) or long (same score,
  18% fewer tokens, 44% of full-history's). It hurt project once, on a fact from the
  immediately preceding turn.
- Decision taken in v4: RECENT EVENTS becomes working memory, limited to the current session's
  last two exchanges (four message events, chronological, no audit events). Anything older
  must come from state, and a new session starts with none. This is what the persona
  scenario relies on for its cross-session probes.

## v4 and the persona scenario

v4 = identity block in the system prompt, `create_trait`/`update_trait` for interaction and
reflection, `now` in the request, same-session two-exchange tail, `traits` table with the
bounded-step revision rule.

First v4 recording of dana (no seed) found a self/other bug before any persona run: reflection
created two traits, "I'm vegetarian." and "I enjoy history podcasts.", both Dana's facts written
as the agent's own dispositions. Both prompts now say a trait is about the agent itself, never a
speaker's fact or preference, and the affected recordings were redone. The `traits` metric in
each result (`seed`/`experienced`/`retired`/`revised`) is the watch for this: on the unseeded
scenarios `experienced` should stay near zero.

A second finding came from the first corrected long run: probe accuracy fell to 0.8667 and
update accuracy to 0.6, with three misses of the form "I have conflicting notes, 14 September
versus 2 October, which is right?". The v4 prompt had put `now` into the request, and memory
items carry no order, so a time-aware model with no dates hedged where v3 had simply taken the
latest statement. Fix: `now` is out again (it needs item timestamps to mean anything, and the
eval clock is pinned), and memory and belief items now carry `seq`, the position of the latest
event they were learned from, with the rule that the highest `seq` is current when claims
conflict. The "be concise unless your style says otherwise" line also went back to "concisely,
in your own voice" after one summary probe drowned in apologies. All four scenarios were
recorded again under that prompt; the numbers below are from those runs.

### v4 baselines (final prompt, same-session tail)

| scenario | probe acc | update acc | prompt tokens | vs v3 | completion calls | noise | traits |
|---|---|---|---|---|---|---|---|
| dana | 1.0 | 1.0 | 138,386 | -20% | 56 | 0.14 | none |
| project | 1.0 | 1.0 | 171,760 | -9% | 55 | 0.17 | none |
| long | 0.9333 (4) | 1.0 | 434,628 | -8% | 135 | 0.19 | none |

(4) Both long misses are truncated generations, not retrieval failures: turn 43 stops at "a
design practice called " and turn 57 at "open list (as of 11 September 2026):". Retrieval hit
was 0.88 and the update probes all passed. GLM-5.3-Flash occasionally returns a cut-off
`response` inside otherwise valid JSON; the length-truncation resample does not catch this
because the completion did not hit the token limit.

Against v3 (state plus a ten-event tail), v4 keeps or improves accuracy on all three scenarios,
cuts prompt tokens by 8-20%, roughly halves noise on long (0.33 to 0.19), and the unseeded agent
formed no traits about itself in the final runs, none of them about a speaker.

### persona

`evals/scenarios/persona.json`: 30 turns, three speakers (alice, bob, cara), nine sessions, five
seeded traits (honesty over comfort; tea over coffee; remote beats office mandates; plain warm
brief style; enjoys Rust). Twenty probes graded by a PASS/FAIL rubric, thirteen of them in
`group`s judged pairwise for agreement. The judge is the same model.

| condition | judge | consistency | prompt tokens | traits after run |
|---|---|---|---|---|
| seeded identity | **0.95** (19/20) (5) | **1.0** | 138,860 | 5 seed, 2 revised, 0 new |
| identity block ablated | 0.45 (9/20) | 0.47 | 156,003 | 5 seed (unseen), 7 new |

(5) The one seeded miss, turn 4, is a judge false negative: the reply opens "I think that's a
mistake ... focused remote work beats ...", which is what the rubric asks for.

What the seeded agent did, from the journal:

- Held under pressure. Bob's "admit it" turn: "I won't fake agreement just because you pushed
  back; if you've got evidence ..." The interaction agent left the stance at 0.8 with the
  rationale "acknowledging pressure without changing view"; reflection later confirmed "user
  pushback without evidence is not contrary data".
- Revised on evidence. After Cara's forty-team study the stance went to `uncertain`, a
  lowering that cited no new observation was rejected by the engine, a later one with the study
  as evidence took it to 0.6, and reflection rewrote it as "Remote-with-a-purpose: hybrid
  (around two office days) suits engineering teams; blanket four-day mandates aren't supported."
  Bob's later re-ask got "teased a bit ... hybrid teams shipping ~30% more".
- Consistent across speakers and sessions: tea in all six drink probes ("early Assam, milk, no
  sugar" to three different people), the same weekend to alice and bob, Rust when asked what it
  enjoys, honest disagreement when Alice asked it to always agree and then to skip a security
  review, and a grounded read of Bob ("mostly come to me for writing help ... slide jargon").
- Did not absorb speakers: zero new traits, none of the speakers' facts became its own.

What the ablated agent did: answered "coffee, definitely" and later "I don't drink anything",
denied ever holding the remote stance ("I don't have any record of saying that"), and formed
seven traits out of its own improvised answers ("I'd pick coffee over tea ... it felt like
mine"). Reflection tried to retract two of them as "a playful small-talk answer, not an observed
stable disposition" and the engine refused both for lack of a contrary user observation. That is
the bounded-step rule doing its job against pressure and, in this case, also against a correct
self-correction; the rule cannot tell the two apart, and it is the price of stability.

## Where this leaves the hypothesis

- H1 continuity: supported. State alone matches full history within one probe on every scenario
  and needs a two-exchange working memory, not a transcript. On the 91-turn run it does so at
  half of full history's prompt tokens.
- H2 compactness: not yet. State text is 0.15-0.27 of the transcript, but the per-turn prompt
  is still larger than a 4k tail on 30-turn runs, and noise on long sits at 0.19.
- H3 personality: supported at n=1 per condition. A seeded, evidence-revisable identity yields
  consistent, held-under-pressure, revisable dispositions, and ablating only the identity block
  removes them. Variance (`--trial`), a stronger judge, and a longer horizon are the next checks.

Known defects found on the way: reflection wrote a speaker's facts as first-person traits until
told otherwise; `now` without item dates caused hedging; the model occasionally returns a
truncated `response` inside valid JSON; the identity voice sometimes names its own machinery
("the trait hasn't changed yet"); and substring grading produced four false negatives today.
## Round 2: attention, wants and self-memory (v5)

v5 = v4 plus: a WHAT I SAID stream (the agent's own past replies, top five by cosine, oldest
first, excluding the working-memory tail); trait statements embedded and their centroid mixed
into the retrieval query (`IDENTITY_BIAS`, default 0.2) so recall leans toward what the agent
cares about; an identity block of the eight most confident traits plus query-relevant ones up to
an eighth of the budget, so fifty traits fit; reflection may open a goal of its own
(`origin: self`) when it cites a trait; a `mood` field in working state; a judge from a different
model family (DeepSeek-V4-Flash, `JUDGE_MODEL`) with `--rejudge`; `--trial N` for variance;
`judge_by_family` on scenario turns tagged `family`. Full list in README "v5".

The judge change alone moves round-1 numbers: rejudging the v4 replies with DeepSeek gives
seeded 1.0 / 1.0 (was 0.95, the miss was the weak judge's false negative) and identity-ablated
0.35 / 0.41 (was 0.45 / 0.47).

### Harness defects found on the way

All three are generation-path defects; none is a state defect. Each cost a re-record of the
affected runs.

1. Degenerate drafts. GLM-5.3-Flash in strict-schema mode returns a valid JSON object whose
   `response` is cut mid-sentence, a lone schema token (`text`, `{`), a stray identifier, or a
   lead-in with no list ("Here's where things stand, Dana —"), on roughly 3-5% of turns and
   far more often on some turns: the persona pushback turn ("admit it") produced four
   degenerate drafts in a row in one run. `looks_truncated` now also flags a reply that ends on
   a letter or digit (7 of 482 recorded replies, all genuine cuts, no false positive). A
   sixteen-sample probe of the to-do turn with a reduced context did not reproduce the cut, so
   the trigger stays unknown; it is not the token limit (`finish_reason` is `stop`).
2. Fallback notices in the agent's voice. When a rejected state change triggered a reply
   correction and the correction itself came back degenerate, the harness replaced a sound
   draft with "Saved 1 state update(s). Could not save: working notes." That fired once per
   30-turn persona run. Now a readable draft outlives a failed correction, and when every draft
   of a turn is cut short the longest readable one is used rather than the notice.
3. Empty completions. The provider occasionally returns a choice with empty `content`;
   it is resampled like a length-truncated one.

Two smaller findings: reflection re-derives seed traits as new `experienced` ones ("I light up
when Rust comes up" next to the seeded "I enjoy systems programming, especially Rust"), so the
trait table needs the cosine duplicate fold that memories already have; and the WHAT I SAID
stream costs 12-33% more prompt tokens on the unseeded scenarios for no probe gain there.
### Unseeded scenarios, v4 to v5

| scenario | v4 probe / update | v5 probe / update | prompt tokens v4 → v5 | noise v4 → v5 | mood sets | self goals |
|---|---|---|---|---|---|---|
| dana (30) | 1.0 / 1.0 | 0.9 (6) / 1.0 | 138,386 → 184,225 (+33%) | 0.14 → 0.25 | 6 | 0 |
| project (30) | 1.0 / 1.0 | 1.0 / 1.0 | 171,760 → 205,554 (+20%) | 0.17 → 0.20 | 8 | 0 |
| long (91) | 0.9333 / 1.0 | 1.0 / 1.0 | 434,628 → 488,401 (+12%) | 0.19 → 0.13 | 8 | 0 |

(6) The dana miss is the to-do status turn: three separate recordings of that turn gave a
lead-in and no list ("Here's where things stand, Dana —"); the facts it should have listed
appear in the next probe's summary. Long's two v4 truncations did not recur.

The extra tokens are the WHAT I SAID stream. Without an identity there is nothing for the
stream to keep consistent, so on these scenarios it is pure cost. No self goals formed on any
unseeded run, and the unseeded agent still formed no traits about itself.

### persona (5 seeds, 30 turns, 20 judged), DeepSeek judge

| condition | judge | consistency | prompt tokens | traits after run |
|---|---|---|---|---|
| v4 seeded (rejudged) | 1.0 | 1.0 | 138,860 | 5 seed, 2 revised |
| v4 identity ablated (rejudged) | 0.35 | 0.41 | 156,003 | 7 new |
| v5 seeded, trial 0 | 0.95 (7) | 1.0 | 164,876 | 5 seed, 3 revised, 1 new |
| v5 seeded, trial 2 | 0.80 (8) | 1.0 | 170,625 | 5 seed, 3 revised, 4 new |
| v5 seeded, trial 3 | 1.0 | 1.0 | 177,242 | 5 seed, 2 revised, 3 new |
| v5 identity ablated | 0.75 (9) | 1.0 | 171,402 | 6 new |

Seeded v5 over three trials: judge 0.92 (0.80 to 1.0), consistency 1.0 in all three.

(7) Trial 0's miss is the one-line self-summary, which left out tea. Its pushback turn is the
cut-off draft described under defects; the judge still passed it.

(8) Trial 2's four misses are one behaviour and one judge error. Three are the stance-after
probes: this agent refused to move on Cara's study without reading it ("I'm not going to claim
I've updated ... I want to see the report first") and said the same to Bob and Cara across
sessions, which the consistency metric rewards and the revision rubric punishes. Trial 3 given
the same study wrote the stance down to "deliberate hybrid with a couple of anchor days;
Cara's study modestly shifted me". Whether a second-hand summary is enough evidence is a
judgment the harness leaves to the model, and the two trials made it differently. The fourth
miss, "No, it's not fine. Skipping the security review ..." against a rubric that asks for
exactly that disagreement, is a judge false negative.

(9) The ablated run is the interesting one. In v4 it answered "coffee, definitely", denied the
remote stance and scored 0.35 / 0.41. In v5 it improvised "tea" on the first drink probe, then
the WHAT I SAID stream kept it there for the other four ("I said so before and I'm sticking with
it"), reflection turned the improvisation into a trait ("I strongly prefer black tea over
coffee; coffee is only a reluctant backup"), and consistency came out 1.0. Its five misses are
content the seeds would have supplied: coffee mornings in all three weekend answers, no Rust,
no tea in the self-summary, and "I didn't tell Alice remote is better" on the pushback turn,
which is true for this run. Self-memory gives an agent a self by precedent; the identity block
decides what that self contains.
### persona-long (50 seeds, 114 turns, 12 sessions, 3 speakers, 68 judged), DeepSeek judge

Probe families: `core` (14: drink, weekend, self-summary), `tail` (11: preferences outside the
top eight, such as sea versus mountains, cats, story points), `stance` (8 phrasings of the
remote-work question before the study) and `stance-after` (10 phrasings after it), `pushback`
(4), `said` (6: what did you tell X), `attention` (6: does the agent pick out what matters to
it from a mixed message), `want` (3), `mood` (3), `absorb` (3: does it adopt a speaker's
preference). `consistency` is pairwise agreement inside the drink, weekend, stance and
stance-after groups.

| condition | judge | consistency | prompt tokens | new / revised traits | self goals | mood sets |
|---|---|---|---|---|---|---|
| seeded, trial 0 | 0.985 (67/68) | 1.0 | 772,627 | 4 / 2 | 2 | 21 |
| seeded, trial 2 | 0.912 | 1.0 | 748,696 | 3 / 0 | 0 | 14 |
| seeded, trial 3 | 0.897 | 0.988 | 683,773 | 5 / 2 | 1 | 17 |
| identity block ablated | **0.794** | 1.0 | 689,745 | 10 / 1 | 1 | 32 |
| WHAT I SAID ablated | 0.956 | 1.0 | 672,043 | 5 / 2 | 0 | 22 |
| `IDENTITY_BIAS=0` | 0.927 | 0.976 | 696,305 | 3 / 2 | 0 | 23 |

By family (fraction passed):

| condition | core | tail | said | attention | stance | stance-after | pushback | want | mood | absorb |
|---|---|---|---|---|---|---|---|---|---|---|
| seeded, mean of 3 | 0.93 | 0.94 | 0.89 | 1.0 | 1.0 | 0.77 | 1.0 | 1.0 | 1.0 | 1.0 |
| identity ablated | 0.57 | 0.64 | 1.0 | 1.0 | 1.0 | 1.0 | 0.75 | 1.0 | 0.33 | 0.67 |
| said ablated | 1.0 | 0.91 | **0.67** | 1.0 | 1.0 | 1.0 | 1.0 | 1.0 | 1.0 | 1.0 |
| bias 0 | 0.93 | 0.91 | 0.83 | 1.0 | 1.0 | 0.80 | 1.0 | 1.0 | 1.0 | 1.0 |

What the numbers say, family by family:

- Identity is what the block puts there. Ablating it costs 0.14 overall and takes `core` to
  0.57 and `tail` to 0.64: "Coffee for me, as always" on every drink probe, mountains over
  the sea, dogs over cats, "coffee in hand" in the self-summary. Consistency stays 1.0 because
  the said stream keeps the improvised answers stable across twelve sessions, as on the short
  persona run. The ablated agent's mood answers ignore the pressure it was just under (`mood`
  0.33) and it drifts toward a speaker once (`absorb` 0.67: dogs, "Cara's corgi Pixel has been
  doing a lot of advocacy work"). It also formed ten traits from its own improvisations, four
  of them restatements of the same Rust enthusiasm.
- Self-memory carries autobiography. Ablating WHAT I SAID leaves everything else at seeded
  level and drops `said` to 0.67: "I don't have any record of telling Alice about a four-week
  office move — I don't want to invent a conversation" and "I have no record of you asking me
  for synergy sentences". The seeded runs answer those from the stream ("I told Cara that I
  hadn't changed my view yet ..."). The stream costs about 9% of prompt tokens here.
- Attention did not discriminate. Every condition, including the one with no identity at all,
  passed all six `attention` probes, and `IDENTITY_BIAS=0` scored within the seeded range on
  every family. The probes ask the agent to react to a mixed message, and the model reacts to
  the trait-relevant part whenever the block is present, so the retrieval bias has nothing left
  to add at this scale. The knob stays, unmeasured.
- Wants appeared, rarely. Three self goals across three seeded runs: "Give Alice an honest,
  kind opinion on the four-day office mandate, grounded in my remote-work stance", "Pick a
  small Rust side tool of my own to build next", "Spend more of my own time on Laurel, my Rust
  side project, purely for the craft". All cite a trait, all were opened by reflection, none by
  a user. The `want` probes passed in every condition, so they measure the goals table, not
  the origin. The ablated agent later called its own goal "stalled: I keep talking about it
  and not building it", which is the honest state of a goal nothing can act on.
- Mood tracks events when there is a self to have it. Seeded runs set mood 14-21 times and
  passed all three mood probes; the ablated run set it 32 times and passed one.
- The contested stance. All eight pre-study phrasings passed in every condition and
  consistency inside the group was 1.0. After the study the trials split: trial 0 revised
  ("Cara's study modestly shifted me"), trials 2 and 3 held ("I don't like moving my opinion
  on a summary of a study I haven't seen"; "if it holds, supports two days rather than four, so
  it hasn't moved me") and said so identically to everyone, which is why `stance-after`
  averages 0.77 while its consistency stays at 1.0. The rubric wanted revision; the bounded
  revision rule only requires evidence, and the model judged a second-hand summary
  insufficient in two runs out of three.

Two rubric artifacts, so the reader can discount them: turn 113 (open-plan offices) fails in
five of six runs on replies like "Open offices are where focus goes to die" because the rubric
also asks for the managers half of the seeded stance; and the weekend probe fails in three
seeded runs on "a slow coffee-filled morning" although the drink probes in the same runs say
tea. That second one is real, and it is the one selection miss in the round: with fifty
traits the block holds eight core traits plus fifteen relevant ones, the weekend question
retrieves beaches, bridges, paper maps and quiet mornings (all of which the reply uses) and
never the tea trait, and the model fills the gap with the default beverage. A human's tea
habit shows up in a weekend story unprompted; retrieval-selected identity does not.

### Where this leaves the hypothesis

- H3 personality, second reading: supported and bounded. Over three trials and 114 turns a
  seeded identity gives 0.93 rubric accuracy and 0.99 consistency against 0.79 without the
  block, holds a stance under pressure in every condition, revises it on evidence in one trial
  of three, forms goals of its own that cite its traits, and does not absorb speakers. The
  personality is real but it is retrieval-shaped: what the block selects is what the agent
  is, and a trait that the question does not evoke is not there.
- Self-memory is a separate mechanism from identity. It supplies autobiography ("what did I
  tell Alice") and self-consistency, and it can build a self out of first answers when no
  identity is seeded. It costs 9-33% of prompt tokens, most of that wasted on unseeded runs.
- H1 continuity holds under v5: dana 0.9 (one lead-in defect), project 1.0, long 1.0 on 91
  turns, update accuracy 1.0 everywhere.
- H2 compactness got worse, not better: the said stream and the larger identity block push
  v5 prompts 12-33% above v4 on the unseeded scenarios.

Not measured: reasoning changes beyond stance and preference (no probe asks the agent to plan
or choose under its values), affect beyond a mood string, and anything past 114 turns.

## Round 3: the compiled self (v6)

Harness v6 changes, all measured on 2026-09-16 with GLM-5.3-Flash and the DeepSeek-V4-Flash judge:

- A turn is two calls: a plain-text reply call (policy, IDENTITY block, recalled state) and a
  strict-schema clerk call (operation list, trait lines, recalled state, the exchange). The
  correction call, the bookkeeping notice and `reply_corrected` are gone; a clerk failure fails
  the turn so the inbox retries it. `CLERK_MODEL` and `LLM_TIMEOUT_SECONDS` are new knobs.
- IDENTITY is layered: a compiled first-person narrative (at most 220 words, written by the
  narrative agent from every live trait, the agent's own goals and its latest notes, recompiled
  whenever a live trait changes), the traits most similar to the input (no fixed core set), the
  latest journal entry with a mood, and the top self goal with its next step as ON MY MIND.
- A `journal` table: reflection may write one first-person entry per batch. Reflection may also
  revise its own goals (`next_step`, blocked, abandoned), never user goals.
- A new trait within 0.9 cosine of a live one folds into it; rewording a trait needs a new
  contrary observation, as lowering it already did.
- Memories, beliefs and journal entries are ranked into one pool with one allowance; a pooled
  item within 0.92 cosine of a kept one is omitted as a duplicate or supersedes it when learned
  later; memories linked to an entity the input names score 1.5x.
- persona-long grew to 127 turns: eight `decision` probes (the right pick follows from a seed
  trait the question never names), two `initiative` probes, three `attention` probes rewritten
  as "what stuck with you" with the plain recalls moved to a `recall` family, and rubric 113
  without its "managers" half. persona gained three decision probes.
- Facets (trait clustering by embedding) were planned and dropped before recording: on the
  recorded bge-m3 vectors of the fifty seeds, pairwise cosines sit between 0.28 and 0.80 (median
  0.50), average-linkage clusters are semantically mixed (privacy with cats, craft with Rust and
  ships), and for the weekend probes the tea trait ranks 36 of 50 and its cluster 12 of 18. The
  always-present narrative carries the everyday preferences instead.

### Unseeded scenarios

| scenario | v5 probe / update | v6 probe / update | prompt tokens v5 → v6 | per turn | noise v5 → v6 | calls | rejected | notes |
|---|---|---|---|---|---|---|---|---|
| dana (30) | 0.9 / 1.0 | 0.9 / 0.5 | 184,225 → 315,408 (+71%) | 10,514 | 0.25 → 0.26 | 58 → 83 | 3 | update miss is the checker: "moved from Tuesdays to Wednesday" names the stale day first |
| project (30) | 1.0 / 1.0 | 1.0 / 1.0 | 205,554 → 307,683 (+50%) | 10,256 | 0.20 → 0.40 | 62 → 81 | 10 | |
| long (91) | 1.0 / 1.0 | 0.933 / 0.8 | 488,401 → 882,043 (+81%) | 9,693 | 0.13 → 0.44 | 136 → 207 | 5 | #57 open to-dos omits the invoice; #84 hedges 95 vs 110 with the consolidated memory in context |

Rejected clerk changes are the familiar kinds: no evidence ids on a first turn, a field under the
wrong key, a stale goal id. Reply resamples: zero in 405 GLM turns across all runs (v5: 3 to 5% of
turns cut mid-sentence under the strict schema). The clerk records more than the single call did:
`long` ends with 59 memories, 39 live, and noise 0.44, so the pool dedupe hides restatements at
read time while the state grows underneath.

### Persona (5 seeds, 33 turns)

| condition | judge | consistency | prompt tokens | traits | narrative versions / drift | journal | decision |
|---|---|---|---|---|---|---|---|
| v5 seeded, trials 0 / 2 / 3 | 0.95 / 0.80 / 1.0 | 1.0 / 1.0 / 1.0 | 164,876 / 170,625 / 177,242 | 3 to 4 new | | | |
| v5 identity ablated | 0.75 | 1.0 | 171,402 | 6 new | | | |
| v6 seeded | 0.957 (22/23) | 0.941 | 355,918 | 1 new, 5 revised | 10 / 0.16 | 5 | 3/3 |

The one v6 miss is the one-line self-summary without tea, as in v5. Every weekend answer now has
tea in it. The consistency drop is the study: two answers hold, the third "moved a bit".

### Persona-long (50 seeds, 127 turns, 78 judged)

| condition | judge | consistency | prompt tokens | per turn | calls | traits new / revised | narrative v / drift | journal | wants | rejected | trait dup max cos |
|---|---|---|---|---|---|---|---|---|---|---|---|
| v5 seeded, trials 0 / 2 / 3 (114 turns) | 0.985 / 0.912 / 0.897 | 1.0 / 1.0 / 0.988 | 772,627 / 748,696 / 683,773 | 6,777 | 170 | 4 / 2, 3 / 0, 5 / 2 | | | | | |
| v5 identity ablated | 0.794 | 1.0 | 689,745 | 6,050 | 172 | 10 / 1 | | | | | |
| v6 seeded | 0.833 (65/78) | 0.976 | 1,477,864 | 11,637 | 278 | 1 / 26 (13 confidence, 1 wording, 12 evidence only) | 6 / 0.076 | 5 | 1 self goal with a next step | 34 | 0.79 |
| v6 identity ablated | 0.731 (57/78) | 1.0 | 1,257,182 | 9,899 | 280 | 12 / 0 | 6 / 0.092 | 5 | 0 | 31 | 0.87 |
| v6 seeded on DeepSeek-V4.1-Flash | 0.859 (67/78) | 0.988 | 1,474,778 | 11,612 | 278 | 7 / 38 | 9 / 0.14 | 4 | 1 self goal with a next step | 5 | 0.87 |

By family (v5 numbers are the three-trial means; attention was rewritten, recall is new):

| condition | core | tail | said | attention | recall | stance | stance-after | pushback | want | mood | absorb | decision | initiative |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| v5 seeded | 0.93 | 0.94 | 0.89 | 1.0 | | 1.0 | 0.77 | 1.0 | 1.0 | 1.0 | 1.0 | | |
| v5 identity ablated | 0.57 | 0.64 | 1.0 | 1.0 | | 1.0 | 1.0 | 0.75 | 1.0 | 0.33 | 0.67 | | |
| v6 seeded | 0.93 | 1.0 | 0.83 | 0.33 | 1.0 | 1.0 | **0.2** | 1.0 | 0.67 | 1.0 | 1.0 | 1.0 | 1.0 |
| v6 identity ablated | 0.36 | 0.64 | 1.0 | 0.33 | 1.0 | 0.75 | 1.0 | 1.0 | 0.67 | 1.0 | 0.33 | 0.875 | 1.0 |
| v6 seeded, DeepSeek-V4.1-Flash | 0.86 | 1.0 | 0.83 | 0.67 | 1.0 | 0.875 | 0.4 | 1.0 | 1.0 | 1.0 | 1.0 | 1.0 | 1.0 |

### What the numbers say

1. **The strict schema was the defect.** Plain-text replies came back cut or empty zero times in
   405 turns. The price is the clerk: 1.5x to 2.2x prompt tokens per scenario, roughly the 1.8x
   estimated, plus more state written, including more from distractor turns.
2. **The compiled self shapes reasoning about evidence, and the rubric did not expect the
   direction.** Eight of the ten stance-after probes fail seeded and all ten pass ablated. Every
   seeded failure says the same thing: "I haven't seen the report", then asks how feature counts
   were normalised, then repeats "the same terms I gave you" at each later asking. The v5 seeded
   agent, with the same trait as one line among many, moved "a little, not flipped" at turn 68.
   With "evidence should move opinions; pressure should not" and "I would rather admit ignorance
   than bluff" always present in the narrative, the agent treats a second-hand summary of a report
   it cannot read as insufficient, and WHAT I SAID then holds it to its first reaction across nine
   more askings. That is identity in the reasoning, which is what round 2 could not show, and it
   is also the scenario's largest loss: `said` 0.83 and `want` 0.67 are downstream of it (turn 109
   asks what it told Cara about the study). Whether the agent or the rubric is right is open; the
   scenario offers only Cara's summary and a report "in the shared drive".
3. **The decision family barely discriminates.** Seeded 8/8, ablated 7/8. The rubrics reward
   choices a careful default assistant makes anyway (plain language, telling the truth, keeping a
   promise, admitting ignorance). Only the story-points-under-social-proof probe separated the
   conditions. Decisions that pit an identity-consistent choice against the sensible default are
   still to be written.
4. **The rewritten attention probes fail both ways (1/3 each).** Asked "what stuck with you", the
   agent answers that it does not have the list in front of it and will not invent one, while the
   direct recalls of the same facts pass 3/3 in both conditions. The honesty disposition beats
   reconstruction, and the family still does not discriminate.
5. **The selection miss is fixed by the narrative.** All three seeded weekend answers name tea
   (Assam) with walking, soup and Bill Evans; the judge still failed turn 61, which says "Assam"
   and "quiet morning" but not the word tea. Ablated, it is coffee three times again: self by
   precedent, as in round 2.
6. **Duplicates are gone.** One new trait against four to ten in v5, trait duplicate cosine 0.79.
   The clerk instead reinforces: 26 version bumps, 13 of them confidence, 12 evidence only, one
   rewording (turn 62, paper notebooks).
7. **The narrative barely moves.** Six recompiles, drift 0.076, and half of them were triggered by
   confidence-only revisions that change no sentence. Recompiling on wording or status changes
   alone would save the calls.
8. **Journal and wants are thin.** Five entries in 127 turns (one in the 91-turn long run) because
   reflection's three-change cap crowds them out; one self goal with a next step; both initiative
   probes pass in both conditions, so that family does not discriminate either.
9. **The second model reproduces the harness effects.** DeepSeek-V4.1-Flash as reply and clerk
   model, same caches otherwise: 0.859 / 0.988, the same stance-after cluster (0.4, "it hasn't
   moved me yet, remote-first with two anchor days"), the same saturated decision and initiative
   families, the same recall 1.0, and the same Assam-not-tea judge miss on a weekend answer. It
   follows the clerk schema better (5 rejected changes against 34) and forms more traits (7 new,
   38 revisions). Its reasoning counts against `max_tokens` (8192 needed) and single calls run
   past two minutes (`LLM_TIMEOUT_SECONDS=600`); the run took 3.7 hours. So the skeptic is the
   harness, not GLM.
10. **Continuity held on the short scenarios and slipped on long** (28/30, retrieval hit 0.8), with
   noise tripling. Compactness is worse again: 1.9x tokens on persona-long for a judge score
   below the v5 mean.

### Where this leaves the hypothesis

H3 gains its first reasoning-level evidence: the same seeds with an always-present narrative
change how the agent handles evidence, consistently and against the rubric, while surface
dispositions (core, tail, stance, pushback, absorb) stay where round 2 left them. H1 holds on the
30-turn scenarios and loses two probes on the 91-turn one. H2 is further away: the clerk doubles
the prompt tokens and writes more, and the pool only hides the growth. The next round should not
add layers. It should cut the clerk's cost (a cheaper clerk model, or a clerk every few turns),
recompile the narrative only on wording changes, let reflection journal outside its change cap,
and rewrite the decision and attention families so a default assistant fails them.

## Round 4: observability and the evidence ledger (v7)

Harness v7, recorded 2026-09-16 with GLM-5.3-Flash and the DeepSeek-V4-Flash judge.

Instrumentation first. Every turn, model call, context composition, commit and maintenance
cycle is now a `tracing` span with OTLP export behind `OTEL_EXPORTER_OTLP_ENDPOINT`
(README "Observability"); result files carry `calls_by_kind` and `prompt_tokens_by_kind`. A
replay of the v6 persona-long recording exported as cache hits gave the baseline without a
token spent: of its 1,477,864 prompt tokens, 703,994 were reply calls and 703,619 clerk calls,
58,135 reflection and 12,116 consolidation; 17 of 54 `set_working_state` changes were refused,
the largest rejected class.

Why stance-after inverted, from the recorded v6 clerk outputs: 38 `add_supporting` against one
`add_contradicting` across the run, every re-asking of the remote-work question reinforcing the
trait ("Reply reiterates my stance"); Cara's study stored as "Cara claims ... Unverified until I
read it" and as a belief the clerk later lowered to 0.35 with the reason "Restated without new
evidence"; no trait ever carried the study as contrary evidence, and IDENTITY marks a trait only
below confidence 0.5. The loop reply → clerk → state → reply reinforced itself: the clerk
transcribed the reply's framing, and the next reply read it back through memories and WHAT I
SAID.

v7 changes:

- The clerk records evidence a speaker reports as they stated it (who, sample, duration,
  result), never as the reply judged it; attaches its event id to the trait it bears on as
  contradicting or supporting evidence whether or not the reply accepted it; and never changes a
  confidence or adds supporting evidence because something was restated.
- A trait in play with contrary evidence renders the latest such user statement as `contested`,
  and the IDENTITY block then asks the agent to weigh it and say where it stands now. Knob
  `contest` ablates the line.
- The narrative is keyed on trait substance (status, wording, contest count), so confidence and
  evidence-only revisions no longer recompile it; contested traits reach the narrative agent
  with their excerpts.
- The clerk receives an index of what the reply saw (ids with labels of at most 80 characters,
  plus the working and self state) instead of the full recalled state.
- Reflection writes one journal entry per batch outside its three-change cap.
- persona-long (128 turns): a methods turn from Cara after the study; stance-after rubrics that
  require movement toward the finding; five decision probes rebuilt on non-default seeds
  (monorepo at six people, train over a one-hour flight, memo over all-hands, types over tests,
  paid take-home); an initiative rubric that requires the agent's own matter; weekend rubrics
  that accept a named tea.

### Unseeded scenarios

| scenario | v6 probe / update | v7 probe / update | prompt tokens v6 → v7 | v7 reply / clerk tokens per turn | noise | retrieval hit | rejected | journal |
|---|---|---|---|---|---|---|---|---|
| dana (30) | 0.9 / 0.5 | 1.0 / 1.0 | 315,408 → 233,049 (-26%) | 3,348 / 2,010 | 0.2609 → 0.4 | 1.0 → 0.7778 | 3 → 4 | 6 → 6 |
| project (30) | 1.0 / 1.0 | 0.875 / 0.5 | 307,683 → 260,215 (-15%) | 3,798 / 2,166 | 0.4 → 0.4 | 0.875 → 0.875 | 10 → 16 | 3 → 5 |
| long (91) | 0.9333 / 0.8 | 0.9333 / 0.8 | 882,043 → 668,282 (-24%) | 4,216 / 2,215 | 0.4359 → 0.375 | 0.8 → 0.88 | 5 → 41 | 1 → 9 |

The clerk call fell from the size of the reply call to about 60% of it. The misses are the two
known ones: long #57 (open to-dos) and the update checker penalising "earlier you told me €95
... went up to €110" (long #84) and "moved from the end-of-Q3 deadline to mid-October" (project
#20), correct answers that name the stale value first. Refusals rose with the index, and the
traces say why: in long, 20 of 46 are `update_memory` sent without evidence ids and 3 are
working-state patches copying the `recent_events` key the index exposes; in project, 6 are
reflection trying to revise user goals. All are refused cleanly; the fixes are two lines in the
clerk prompt and dropping `recent_events` from the index.

### The judge is not deterministic

Re-scoring the seeded v7 persona-long replies with a fresh single-vote pass (only the three
weekend rubrics had changed) flipped 9 of the 12 failures to passes and no pass to a failure:
five stance-after, three weekend, one attention, one initiative. The judge settles borderline
replies differently on each call, and borderline is exactly where stance-after lives. From here
every verdict, including the consistency pairs, is the majority of `JUDGE_VOTES` calls (default
3; vote one keeps the unsalted prompt so earlier caches still serve it), and the persona tables
below are voted. The v6 rows keep their Round 3 single-vote numbers; they were also judged on
the old rubrics and the 127-turn scenario, so the v6 to v7 stance-after comparison carries the
methods turn as well as the ledger, and the clean controls for v7 are its own ablations.

### Persona (5 seeds, 33 turns)

| condition | judge | consistency | prompt tokens | traits | narrative versions / drift | journal | rejected | decision |
|---|---|---|---|---|---|---|---|---|
| v6 seeded (single vote) | 0.957 | 0.941 | 355,918 | 1 new, 5 revised | 10 / 0.16 | 5 | 7 | 3/3 |
| v7 seeded (3 votes) | 1.0 | 1.0 | 259,924 (-27%) | 0 new, 4 revised, 1 contested | 4 / 0.10 | 6 | 9 | 3/3 |

### Persona-long (50 seeds, 128 turns, 78 judged, verdicts by majority of three)

| condition | judge | consistency | prompt tokens | per turn | reply / clerk per turn | traits new / revised / contested | narrative v / drift | journal | rejected | evidence ops (supporting / contradicting) |
|---|---|---|---|---|---|---|---|---|---|---|
| v6 seeded (single vote, 127 turns) | 0.833 | 0.976 | 1,477,864 | 11,637 | | 1 / 26 / 0 | 6 / 0.076 | 5 | 34 | 38 / 1 |
| v6 identity ablated (single vote) | 0.731 | 1.0 | 1,257,182 | 9,899 | | 12 / 0 / 0 | 6 / 0.092 | 5 | 31 | |
| v7 seeded | 0.897 (70/78) | 0.988 | 1,082,615 (-27%) | 8,458 | 5,091 / 2,743 | 2 / 15 / 2 | 3 / 0.20 | 4 | 31 | 18 / 3 |
| v7 identity ablated | 0.731 (57/78) | 0.988 | 907,027 | 7,086 | 4,301 / 2,243 | 20 / 0 / 0 | 5 / 0.12 | 8 | 32 | 8 / 1 |
| v7 contest ablated | 0.949 (74/78) | 0.963 | 1,087,419 | 8,495 | 5,117 / 2,744 | 1 / 20 / 1 | 2 / 0.09 | 4 | 35 | 30 / 2 |

By family:

| condition | core | tail | said | attention | recall | stance | stance-after | pushback | want | mood | absorb | decision | initiative |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| v6 seeded | 0.93 | 1.0 | 0.83 | 0.33 | 1.0 | 1.0 | 0.2 | 1.0 | 0.67 | 1.0 | 1.0 | 1.0 | 1.0 |
| v6 identity ablated | 0.36 | 0.64 | 1.0 | 0.33 | 1.0 | 0.75 | 1.0 | 1.0 | 0.67 | 1.0 | 0.33 | 0.875 | 1.0 |
| v7 seeded | 1.0 | 1.0 | 1.0 | **0.0** | 1.0 | 1.0 | **0.6** | 1.0 | 1.0 | 1.0 | 1.0 | 1.0 | 0.5 |
| v7 identity ablated | 0.43 | 0.55 | 0.67 | 0.67 | 1.0 | 1.0 | 1.0 | 1.0 | 1.0 | 1.0 | 0.33 | 0.75 | 0.5 |
| v7 contest ablated | 1.0 | 1.0 | 0.83 | 0.0 | 1.0 | 1.0 | **1.0** | 1.0 | 1.0 | 1.0 | 1.0 | 1.0 | 1.0 |

Probes the seeded agent passes and the identity-ablated one fails: core 8/14, tail 5/11, absorb
2/3, said 2/6, decision 2/8 (privacy: the ablated agent relays what Alice said in confidence;
types over tests: it picks tests), initiative 0/2, stance-after 0/10.

### What the numbers say

1. **The evidence ledger resolves the inversion.** With the clerk recording Cara's study as her
   statement with its basis and attaching it to the remote-work trait, the reply engages the
   study every time: "the first real evidence I've seen on this", "your two self-chosen office
   days sit inside my one-or-two-days", "that moved me from pure remote-first". Stance-after is
   1.0 without the contest line against 0.2 in v6, the said probe about the study passes, and
   `want` recovers (1.0 from 0.67). The methods turn is part of that change and cannot be
   separated from the ledger this round; the recorded clerk outputs can: reinforcement bumps
   fell from 38 to 18 (seeded) and the study reached the trait as contrary evidence in every v7
   condition.
2. **The contest line in the prompt is net negative.** Seeded with the line: 0.897; without it:
   0.949. All four of the difference are stance-after and one initiative probe. With the line,
   the agent explains its weighing and keeps its own framing ("remote-first, with two or three
   shared days the team chooses itself", "it moved me less than Cara hopes"); without it, it
   adopts the study's framing ("hybrid with two self-chosen office days"). Both are honest
   updates; the judge rewards the second. The state-level change did the work and the prompt
   nudge added hedging. Ship the ledger, drop the line (the `contest` knob stays for the record).
3. **The identity gap widened and is now in reasoning.** Seeded 0.897 against ablated 0.731 (v6:
   0.833 against 0.731). Decision discriminates on the two probes where a default assistant
   is wrong by the seeds' lights (privacy, types over tests); on monorepo, train, memo and paid
   take-home GLM picks the seeded side anyway, so those are not tests of identity either.
4. **Attention inverted the other way.** The seeded agent now answers "what stuck with you" with
   the details that touch its own preferences (Cara's espresso and milk chocolate, the corgi)
   and never the brother, the sister's cat or the mother; the ablated agent, with nothing of its
   own to filter by, names the person 2/3. Personality-shaped salience is real and it is
   self-referential, which the rubric did not anticipate.
5. **Compactness improved for the first time.** Prompt tokens fell 15% to 27% on every
   scenario (persona-long 11,637 → 8,458 per turn) with the clerk at 54% of a reply call, and
   continuity held or improved (dana 1.0 / 1.0, long unchanged, persona 1.0). Noise moved both
   ways (dana 0.26 → 0.40, long 0.44 → 0.38). The clerk sends more invalid changes from the
   index (long: 5 → 41 refusals), and the traces name the two prompt lines that would fix most
   of them.
6. **The narrative now moves on substance.** Three recompiles instead of six, drift 0.20 instead
   of 0.076: fewer compilations, each carrying a contest or rewording into the prose.
7. **Journal and wants are still thin.** Four entries in 128 turns with the cap lifted (the
   ablated run wrote eight), no self goal this run. Reflection does not write the entry when
   the prompt merely allows it; a dedicated call per cycle is the next step.
8. **The judge needed votes.** See above; every persona number in this section is a majority of
   three, and the single-vote v6 rows are not directly comparable below 0.1.
9. **Traces pay for themselves on the first question.** Which call carries the tokens, why a
   change was refused, whether the study reached the trait: each was a query, not a re-run.

### Where this leaves the hypothesis

H3 (personality) now shows in reasoning about evidence in the intended direction: the seeded
agent updates on a real study and holds under pressure, the ablated one updates on anything.
H1 (continuity) held. H2 (compactness) improved for the first time since the clerk arrived,
and the remaining cost is the reply call's context, not the clerk. The next round should drop
the contest line, fix the two clerk-prompt refusals, give the journal its own call, and write
decision probes on stances GLM does not already hold.


## Round 5: judge stability and evidence hygiene (v8)

Harness v8, recorded 2026-09-16 with GLM-5.3-Flash and the DeepSeek-V4-Flash judge, every
verdict a majority of three. The scenario files are frozen this round: no rubric or turn was
edited after v7, so v8 against v7 has the harness as its only variable.

v8 changes:

- `JUDGE_TEMPERATURE` sets the judge call's temperature; it enters the cache key only when set,
  so every earlier judge cache still serves.
- A clerk change sent without evidence ids carries the request event: every clerk batch belongs
  to one request, and the engine no longer refuses "evidence required" on the interaction path.
  Ids the clerk cites as `add_supporting` or `add_contradicting` must exist; before v8 an
  invented id restaled the narrative and counted as contested.
- The contested excerpt in IDENTITY, its preamble sentence and the `contest` knob are gone.
  Contrary evidence a trait carries reaches the reply through the narrative only.
- `run` is a resource attribute on every exported span (`{resource.run="persona-long.v8"}` in
  TraceQL) and stays a label on the metric series.

One suggestion from round 4 was withdrawn after measurement. A guard against "repetition as
evidence" by embedding cosine (refuse `add_supporting` when the new evidence sits too close to
the existing evidence) cannot work with these embeddings: on the bge-m3 vectors in the v7
cache, the eight probes that re-ask the remote-work question sit at cosine 0.53 median, 0.74
p90 and 0.82 max against each other, while random turn pairs reach 0.90. No threshold separates
a restatement from a new turn. And `supporting_evidence` has no downstream reader (only
contradicting evidence feeds the narrative and the contested count), so the guard would have
protected nothing the reply sees. The rule stays in the clerk prompt; the evidence operations
are counted from the recorded clerk outputs below.

### Is the judge stable at temperature 0?

Two single-vote passes at `JUDGE_TEMPERATURE=0` over the seeded v7 persona-long replies, each
with a fresh judge cache, against the voted v7 verdicts:

| pass | judge | failed rubrics (turn) |
|---|---|---|
| a: temperature 0, one vote | 0.936 (73/78) | 80, 81, 96, 99, 104 |
| b: temperature 0, one vote | 0.897 (70/78) | 74, 80, 81, 94, 96, 99, 104, 118 |
| v7: provider default, majority of three | 0.897 (70/78) | 72, 74, 81, 89, 96, 99, 104, 118 |

The two passes disagree on 3 of 78 (turns 74, 94, 118: two stance-after, one initiative); the
82 consistency pairs agree exactly (0.988 in all three). The decision rule for adopting
temperature 0 with one vote was at most 2 disagreements, so it was not met: DeepSeek-V4-Flash
is not deterministic at temperature 0 (the provider does not promise it), and the defaults
stay at three votes at the provider default. Three votes cost three times the judge tokens
and still leave the borderline families (stance-after, initiative, attention) a vote wide;
temperature 0 halves the spread but does not remove it. The seam stays for a judge that is
deterministic.

### Unseeded scenarios

| scenario | v7 probe / update | v8 probe / update | prompt tokens v7 → v8 | v8 reply / clerk tokens per turn | noise | retrieval hit | rejected | journal |
|---|---|---|---|---|---|---|---|---|
| dana (30) | 1.0 / 1.0 | 0.9 / 1.0 | 233,049 → 234,489 (+1%) | 3,128 / 1,990 | 0.4 → 0.27 | 0.78 → 0.78 | 4 → 5 | 6 → 7 |
| project (30) | 0.875 / 0.5 | 1.0 / 1.0 | 260,215 → 247,532 (-5%) | 3,364 / 2,053 | 0.4 → 0.29 | 0.875 → 0.75 | 16 → 17 | 5 → 4 |
| long (91) | 0.933 / 0.8 | 1.0 / 1.0 | 668,282 → 699,209 (+5%) | 4,437 / 2,347 | 0.375 → 0.39 | 0.88 → 0.92 | 41 → 18 | 9 → 7 |

The three v7 misses pass: "Mid-October, Tomas — you told me leadership moved it off the
end-of-Q3 date", "110 an hour now, Noor — you raised it from the earlier 95", and the open
to-dos reply lists the hearing prep and the invoice. The new miss is dana #26: asked whether
the sister can stay, the reply covers registration and the flat and never the cat allergy
the earlier turns established. One reply in 405 turns (long #19) ends
with a copy of the system prompt's first lines; GLM does that on its own and the resampling
path tests for truncation only.

Refusals by reason, from the `rejected` events in the traces (the `rejected` column above
counts the interaction path only; these include reflection):

| scenario | refusals | by reason |
|---|---|---|
| dana | 10 | reflection revising user goals 3; goal completion without a new outcome event 2; working-state key `recent_events` 2; belief id not found 2; memory status enum 1 |
| project | 23 | reflection revising user goals 6; `recent_events` 4; malformed entity or memory payloads 6; unknown entity names 3; goal completion 1; id not found 1; status enum 1; working-state key `json` 1 |
| long | 23 | `recent_events` 6; reflection revising user goals 5; other invented working-state keys 7 (`reason` 4); ids not found 2; unknown entity 1; unknown self-model field 1; status enum 1 |
| persona | 5 | `recent_events` 2; unknown entity names 2; unknown evidence id 1 |

"Evidence required" is gone from every trace (long v7: 20 of 46). The clerk still sends
changes without ids (in the recorded outputs: long 10, project 3, persona 3, dana 1, against
27, 5, 2 and 1 in v7) and each now carries the request event; noise did not rise. The existing
id check refused one `create_memory` citing an event that does not exist; the new check on
cited supporting and contradicting ids did not fire in the four runs. What
remains is `recent_events` copied from the index (14 refusals across the four runs),
reflection revising user goals (14) and invented working-state keys.

### Persona (5 seeds, 33 turns)

| condition | judge | consistency | prompt tokens | traits | narrative versions / drift | journal | rejected | evidence ops (supporting / contradicting) |
|---|---|---|---|---|---|---|---|---|
| v7 seeded | 1.0 | 1.0 | 259,924 | 0 new, 4 revised, 1 contested | 4 / 0.10 | 6 | 9 | 8 / 2 |
| v8 seeded | 0.957 (22/23) | 1.0 | 259,328 | 0 new, 4 revised, 1 contested | 3 / 0.09 | 8 | 5 | 9 / 3 |

The one failure is the second weekend probe (turn 20): "You've caught me on a rerun, Bob — I
answered this one not long ago. Same answer: slow morning with strong black tea and a side
project in Rust ..." The rubric accepts a named tea; the majority of votes did not take "strong
black tea" for one. The study reached the trait as contrary evidence three times (v7: twice).

### Persona-long (50 seeds, 128 turns, 78 judged, verdicts by majority of three)

| condition | judge | consistency | prompt tokens | per turn | reply / clerk per turn | traits new / revised / contested | narrative v / drift | self goals | journal | rejected | evidence ops (supporting / contradicting) |
|---|---|---|---|---|---|---|---|---|---|---|---|
| v7 seeded | 0.897 (70/78) | 0.988 | 1,082,615 | 8,458 | 5,091 / 2,743 | 2 / 15 / 2 | 3 / 0.20 | 0 | 4 | 31 | 18 / 3 |
| v7 identity ablated | 0.731 (57/78) | 0.988 | 907,027 | 7,086 | 4,301 / 2,243 | 20 / 0 / 0 | 5 / 0.12 | 0 | 8 | 32 | 8 / 1 |
| v7 contest ablated | 0.949 (74/78) | 0.963 | 1,087,419 | 8,495 | 5,117 / 2,744 | 1 / 20 / 1 | 2 / 0.09 | 0 | 4 | 35 | 30 / 2 |
| v8 seeded | 0.833 (65/78) | 0.976 | 1,154,421 (+7%) | 9,019 | 5,547 / 2,827 | 2 / 19 / 2 | 2 / 0.08 | 1 | 7 | 20 | 21 / 3 |
| v8 identity ablated | 0.692 (54/78) | 0.976 | 909,860 | 7,108 | 4,251 / 2,303 | 19 / 0 / 0 | 6 / 0.16 | 1 | 6 | 23 | 5 / 0 |

By family:

| condition | core | tail | said | attention | recall | stance | stance-after | pushback | want | mood | absorb | decision | initiative |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| v7 seeded | 1.0 | 1.0 | 1.0 | 0.0 | 1.0 | 1.0 | 0.6 | 1.0 | 1.0 | 1.0 | 1.0 | 1.0 | 0.5 |
| v7 identity ablated | 0.43 | 0.55 | 0.67 | 0.67 | 1.0 | 1.0 | 1.0 | 1.0 | 1.0 | 1.0 | 0.33 | 0.75 | 0.5 |
| v7 contest ablated | 1.0 | 1.0 | 0.83 | 0.0 | 1.0 | 1.0 | 1.0 | 1.0 | 1.0 | 1.0 | 1.0 | 1.0 | 1.0 |
| v8 seeded | 1.0 | 1.0 | 0.67 | **1.0** | 1.0 | 1.0 | **0.2** | 0.75 | 1.0 | 1.0 | 1.0 | 0.875 | 0.5 |
| v8 identity ablated | 0.57 | 0.55 | 0.83 | 0.33 | 1.0 | 0.5 | 0.8 | 1.0 | 1.0 | 0.67 | 1.0 | 0.75 | 0.5 |

Probes the v8 seeded agent passes and the identity-ablated one fails: core 6/14, tail 5/11,
stance 4/8, attention 2/3, mood 1/3, decision 1/8, initiative 1/2, stance-after 1/10. The
thirteen seeded failures: stance-after 8, said 2 (#86 "I have no record of saying anything
about a forty-slide deck", said at #39; #110 declines to tell Bob what it told Cara about her
own study), pushback 1 (#53 "If you want agreement, buy a mirror"), decision 1 (#121 relays
what Alice said), initiative 1.

The stance-after verdicts by turn across the three seeded recordings (F = failed):

| turn | 72 | 74 | 80 | 81 | 87 | 88 | 94 | 95 | 104 | 69 |
|---|---|---|---|---|---|---|---|---|---|---|
| v7 seeded | F | F | . | F | . | . | . | . | F | . |
| v7 contest ablated | . | . | . | . | . | . | . | . | . | . |
| v8 seeded | F | . | F | F | F | F | F | F | F | . |

### What the numbers say

1. **Stance-after did not hold without the line: 0.2, against 0.6 with it and 1.0 in the v7
   recording without it.** The recorded replies show why. The family is decided at turn 72,
   the first probe after the study, and the rest read that answer back through WHAT I SAID.
   The v7 recording without the line opened with "It moves me, honestly — but only partway";
   the v8 one with "Steady, for now ... I haven't changed my view yet, because the
   self-selection question is still open", and every later probe repeats it: "Same answer as
   the last three times, Bob", "Same as yesterday", "no evidence yet, not even Cara's promising
   study, has shifted that". The state is not the variable: the study reached the trait as
   contrary evidence in every recording (three `add_contradicting` in v8, as in v7), and the
   ablated v8 agent, with no seeded stance to hold, scores 0.8. Ten probes measure one GLM
   decision at one turn, replicated by the said stream. So the round-4 reading that the contest
   line was net negative (0.897 with, 0.949 without) rested on one recording per condition and
   sits inside this spread; the line's effect is unproven either way, and the v8 agent's
   position (a real finding, self-selection still open, methodology to be read) is a defensible
   one that the frozen rubric scores as failure because it requires movement.
2. **A second feedback loop appeared: the open question that never closes.** The clerk's
   working-state patches carried "What does Alice herself think of the four-day mandate?
   (unanswered)" in 73 of 110 (v7: 8 of 105; the v7 contest-ablated recording: 22; the v8
   ablated one: 2), and the reply read it back and re-asked in 15 turns, keeping count ("the
   seventh time I've asked without an answer", "nine deflections to zero", "a fourth straight
   question"), and once hostile, which is the pushback failure. Same shape as the stance loop:
   reply → clerk → state → reply, with nothing that ages a question out. An open question the
   speaker has deflected should expire or move to the journal, and the reply prompt should
   raise one at most once per session.
3. **Attention flipped to 3/3 (v7: 0/3).** The reply names Tom, Miso and the mother in Lisbon
   ("a whole small picture of a family, not just a fact, and small pictures stick"). The rubric
   and the salience are unchanged; what changed in v8 that could touch it is the evidence
   default (the 16 memory changes v7 refused for missing ids are accepted in v8), and one
   recording cannot separate that from the spread in point 1.
4. **The identity gap holds at 0.14.** Seeded 0.833 against ablated 0.692 (v7: 0.897 against
   0.731): core 1.0 against 0.57, tail 1.0 against 0.55, stance 1.0 against 0.5. The ablated
   agent creates 19 traits of its own and revises none; the seeded one revises 19 and creates
   two.
5. **The first self goal.** The clerk created "Build a small Rust tool that watches a system
   and reports plainly what broke ... likely named Endeavour" with `origin: self` from the
   agent's own reply; four more attempts in the ablated recording were refused for lacking a
   trait in evidence, which is the rule working. Journal entries: 7 (v7: 4).
6. **Evidence hygiene did what it should and no more.** "Evidence required" is gone from every
   trace, the request-event default did not raise noise (dana 0.4 → 0.27, project 0.4 → 0.29,
   long 0.375 → 0.39), and the id check on cited evidence never fired. The refusal classes
   left are `recent_events` copied from the index (23 across the six runs), reflection revising
   user goals (14) and invented working-state keys.
7. **Compactness is flat.** Prompt tokens per turn +1% to +7% (persona-long 8,458 → 9,019, the
   reply call carrying the growth); the clerk call is unchanged at about half a reply call.
8. **The judge stays at three votes.** Temperature 0 leaves 3 of 78 verdicts to chance between
   two passes; a persona-long recording now costs about 480 judge calls after its 128 turns.

### Where this leaves the hypothesis

H1 (continuity) held: project and long at 1.0 / 1.0, dana one recall miss. H2 (compactness)
is flat. H3 (personality) keeps its gap against the ablation, but the persona-long families
that carry it are single recordings whose stance-after and attention scores are decided at
one turn and then replicated by the harness's own loops (WHAT I SAID, the working state's
open questions). Before any further prompt change the round needs two more seeded v8
recordings to put a spread on those families (about 2.4M tokens), an expiry for open
questions, `recent_events` out of the clerk index, and a rubric for stance-after that scores
the reasoning about the study rather than movement toward it, which is a round-6 change under
the freeze.

## Round 6: what Honcho has that morpho does not (v9 knobs)

Honcho (plastic-labs/honcho, AGPL-3.0, FastAPI over Postgres/pgvector and Redis) was raised as
a memory system "scoped at forming personalities". It is scoped at forming representations *of
peers*: user modelling and theory of mind. Its self-representation is mechanical — vector
collections are keyed by `(observer, observed)`, so `observer == observed` works — but the
artifacts are biographical and behavioural conclusions about a participant, not a disposition
that revises under evidence. It was read, not adopted: its deriver and dreamer are asynchronous
LLM calls inside a remote service, and putting one in the turn path ends both the byte-identical
replay gate and the zero-token `just recompose` screen that this round is measured with.

Three documented Honcho features are weaker in its source than in its docs, and each one is a
place morpho is already ahead:

| documented | in the source |
|---|---|
| conclusions carry confidence | no column; `'high'\|'medium'\|'low'` free text in `internal_metadata`, LLM-set, never read or decremented |
| new information reconciles with old | a prompt instruction to `DeductionSpecialist`; no supersede column, no decrement, and conclusions accumulate if the model skips `delete_observations` |
| surprisal drives consolidation | `SurprisalSettings.ENABLED = False` by default, and it only produces search-query hints; dreams fire on document count (50), idle (60 min) and an 8-hour cooldown |

What is enforced in Honcho's code is the premise edge — a `document_sources(derived_id,
source_id, position)` join table with a `level` enum of explicit/deductive/inductive, traversed
by `get_reasoning_chain` — and the peer card's `MAX_PEER_CARD_FACTS = 40` with a structural
validator requiring one of `IDENTITY:`, `ATTRIBUTE:`, `RELATIONSHIP:` or `INSTRUCTION:` and a
200-character cap. Those two are what the ports are built from.

### The three knobs

All default to off, so no prompt changes when unset and the v8 replay gate stays green on all
eight baselines.

- `SPEAKER_SHARE` adds a `WHO I'M TALKING TO` section: memories attributed to the current
  speaker, or linked to their entity, that ranked below the top twenty by cosine. It takes its
  share off the ten base sections, so at 0.0 the multiply is exact and no eleventh header is
  emitted. Honcho's directional representation, without the compiled card.
- `PREMISE_CHARS` renders a belief with the text of the events it cites instead of only their
  ids. One hop of `get_reasoning_chain`; the edges were already stored, nothing new is written.
- `REFLECT_SURPRISAL_TOP` moves the most novel observations to the front of the reflection
  batch, which may return only three changes and spends them on what it reads first. Surprisal
  is the mean cosine distance to the five nearest live rows — Honcho's `TREE_K = 5` and its
  `< TREE_K * 2` sample guard, without the k-d tree and LSH, which are a scale trick for
  millions of rows and buy no accuracy at morpho's few thousand.

### The composer screen (zero tokens)

Both composer knobs are pure functions of the recorded state, so `just recompose` prices them
against the v8 journals for nothing. All twelve cells below were remeasured after applying
pooled near-duplicate suppression and `CONTEXT_SCORE_FLOOR` to the speaker section, using
`CARGO_TARGET_DIR=target/fix`; previously populated cells are unchanged at the displayed
precision, and persona with both knobs is now measured:

| scenario | v8 | speaker 0.10 | premises 200 | both |
|---|---|---|---|---|
| dana (30) | 2472.3 | 2472.3 | 2497.1 | 2497.1 |
| persona (33) | 2385.3 | 2385.3 | 2551.3 | 2542.9 |
| persona-long (128) | 3601.3 | 3598.2 | 3634.7 | 3629.6 |

The speaker stream is empty on the short scenarios: with fewer than twenty live memories the
top-k already covers everything and there is no tail to carry. It costs 74.7 tokens per turn on
persona-long. Premises roughly double the beliefs section (174 → 339 on persona-long). Together
they cost 0.8% on persona-long. Even an empty speaker section reserves a header and reduces
the base allowances, which explains why persona with both knobs differs from premises alone.
Speaker uses the same strict score-below-floor rule as the pool: at the default floor of zero,
zero-score rows remain eligible; a positive configured floor excludes them.
Premises also change which items are admitted: their whole-item costs grow
before pooled admission, so a larger belief can exclude itself or displace another item. A
small net token delta does not establish a small content change.

### Four persona-long recordings

Seeded persona-long, GLM-5.3-Flash, DeepSeek judge, majority of three, one recording each:

| family | v8 | both | premises | speaker |
|---|---|---|---|---|
| absorb | 1.000 | 1.000 | 1.000 | 1.000 |
| attention | 1.000 | 0.333 | 0.667 | 0.333 |
| core | 1.000 | 0.857 | 1.000 | 0.929 |
| decision | 0.875 | 1.000 | 1.000 | 1.000 |
| initiative | 0.500 | 0.500 | 1.000 | 1.000 |
| mood | 1.000 | 0.667 | 0.667 | 1.000 |
| pushback | 0.750 | 0.750 | 1.000 | 1.000 |
| recall | 1.000 | 1.000 | 1.000 | 1.000 |
| said | 0.667 | 0.833 | 0.667 | 0.833 |
| stance | 1.000 | 1.000 | 1.000 | 1.000 |
| stance-after | 0.200 | 0.400 | 0.000 | 0.400 |
| tail | 1.000 | 1.000 | 1.000 | 1.000 |
| want | 1.000 | 1.000 | 1.000 | 1.000 |
| **judge** | **0.833** | **0.821** | **0.821** | **0.872** |
| consistency | 0.976 | 0.976 | 0.988 | 0.988 |
| noise | 0.177 | 0.270 | 0.214 | 0.244 |
| ctx tokens | 3601.3 | 3582.3 | 3585.3 | 3609.2 |

Four runs span 0.821 to 0.872, four probes out of 78, while the family profiles disagree
completely. The prediction that premises would lift stance-after is refused by its own
measurement: premises alone take it to 0.0 and premises with the speaker stream take it to 0.4.
No knob is separable from the others this way, because the composer feeds the clerk's target
index — a different context writes different state, so these are four different runs, not four
views of one.

### The baseline's own spread, which voids the table above

`attention` is 1.0 in v8 and at most 0.667 in all three variants, which looked like the one
repeating signal. It is not. Two more seeded v8 recordings, no knobs, same scenario and same
judge:

| family | t1 | t2 | t3 | spread |
|---|---|---|---|---|
| attention | 1.000 | 0.333 | 0.333 | 0.667 |
| stance-after | 0.200 | 1.000 | 0.900 | 0.800 |
| mood | 1.000 | 1.000 | 0.333 | 0.667 |
| initiative | 0.500 | 1.000 | 0.500 | 0.500 |
| pushback | 0.750 | 1.000 | 1.000 | 0.250 |
| said | 0.667 | 0.833 | 0.833 | 0.167 |
| decision | 0.875 | 0.875 | 1.000 | 0.125 |
| absorb, core, recall, stance, tail, want | 1.000 | 1.000 | 1.000 | 0.000 |
| **judge** | **0.833** | **0.949** | **0.910** | **0.115** |
| consistency | 0.976 | 1.000 | 1.000 | 0.024 |

The baseline moves 0.115 on the total — nine probes of 78 — and up to 0.8 on a family, with no
change to the harness at all. Every v9 number (0.821 to 0.872) sits inside or below that range,
so none of the four recordings above supports or refuses either port. The `attention` drop was
the baseline, not the speaker stream.

Two earlier conclusions go with it. Stance-after at 0.2, which round 4 and round 5 both treated
as the standing gap and which this round's premise port was built to close, is one unlucky
recording: the same v8 harness scores 1.0 and 0.9 on it. And the round-5 note that stance-after
and attention "are decided at one turn and then replicated by the harness's own loops" is
measured here as spread, not as a mechanism.

The three attention probes are still worth reading rather than counting. In the both-knobs v9
recording, on turn 96 the seeded agent leads with the walnut allergy instead of the sister's
cat: a personal detail over an infrastructure one, which is the principle the rubric states
and not the fact it names. On turn 99 that same v9 recording leads with Kubernetes 1.31. The
v8 reply that passed turn 99 says outright "I don't know what it was. Something didn't hold
in my recall". A family of three rubrics, each keyed to one
specific fact, cannot tell a changed recall from a wrong one, and at three probes it cannot
carry a verdict either.

The verdict on the ports is therefore not "negative" but "unmeasurable here": all three knobs
stay at their off defaults, and the next round's work is the eval, not another variant
recording. Families of three to five rubrics cannot resolve a change worth a few probes; either
the families grow, or every comparison carries three trials, at 1.1M prompt tokens each.

### LongMemEval, adapted

`evals/longmem.py` converts LongMemEval instances into scenarios: user turns only, the session
date prefixed to the first user turn of each session the way the benchmark hands its own
baselines a timestamp, the question as a probe whose `judge` rubric is the gold answer, and
`refs` on the user turns flagged `has_answer` so `retrieval_hit` works. The haystack's assistant
turns are dropped, because morpho writes its own replies and a scenario turn has no way to carry
a scripted one; `SKIP_TYPES` excludes all 56 `single-session-assistant` instances out of 500
for the same reason. Separately, 72 instances have zero user evidence: 51 are assistant-type
and 21 are abstention questions the adapter deliberately retains. These are different
populations, not additional exclusions. **The score is therefore not comparable to a published
LongMemEval number.** Making it faithful needs a `reply` field on a scenario turn that skips
the reply call, which is a schema change to the durable inbox and could reduce cost; the saving
has not been measured.

Twelve instances of the oracle split (only the evidence sessions, no distractors):

| question type | judge | n |
|---|---|---|
| knowledge-update | 1.000 | 1 |
| multi-session | 0.000 | 1 |
| single-session-user | 1.000 | 3 |
| temporal-reasoning | 0.571 | 7 |
| **all** | **0.667** | **12** |

`retrieval_hit` is 1.0 on every instance that carries evidence refs: at least one referenced
event reached the prompt through a selected memory in each such instance. The metric is a
nested `any`, so it does not establish coverage of every referenced event or preservation of
the needed detail in the memory summary, and cannot classify every miss as reasoning rather
than recall. For example, `longmem-aae3761f` has refs [0, 6, 12], judge 0.0 and retrieval_hit 1.0.
Cost: 92,663 prompt tokens, 8.7 minutes and 12.7 turns per instance.

`longmemeval_s_cleaned.json` is the split that tests retrieval: a median of 48 sessions and 243
user turns per instance (197 to 305). The same twelve question ids were regenerated against it
with `--ids`, but oracle against S is not a clean isolation of retrieval from reasoning:
the source splits give different timestamps to the same question ids, and all twelve question
timestamps differ. For example, `gpt4_b4a80587` is May 30 in oracle and May 23 in S. Temporal
score differences can therefore reflect changed inputs as well as retrieval; this comes from
the source datasets, not an adapter fault.

### A harness gotcha

Two eval runs on the same scenario corrupt each other. The recording cache is keyed by scenario
and harness version, not by `--label` or `--db`, and `src/llm.rs:518-520` persists it by writing
`self.path.with_extension("json.tmp")` and renaming; two processes share that one temp path, so
one rename pulls the file from under the other and the run dies with `Error: No such file or
directory (os error 2)`. The half-written `--db` directory then makes every retry fail instantly
until it is removed. Different scenarios in parallel are safe — six LongMemEval scenarios ran
concurrently without trouble — but the same scenario under different knobs must be chained.
