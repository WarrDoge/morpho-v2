# Workshop: does a morphling form habits and decide for itself?

Measured 2026-09-17 on harness v8 plus the workshop (`README.md` "Workshop",
`evals/scenarios/workshop.json`). Every run uses GLM-5.3-Flash, six small Python tasks in a
bubblewrap sandbox, hidden tests the agent never sees, and no model judge. There are four arms
with three trials each, 12 recordings in all: 1.5M prompt tokens, 11 to 33 minutes per run
live, and 2 to 3 seconds per run on replay.

- `transcript` is a conventional coding agent. It sees the task's own transcript and has no
  memory, identity or thinking.
- `nothink` is the morphling: recalled state, identity, and reflection across tasks.
- `self` adds a think call the actor may choose.
- `surprise` also thinks after a confident miss: a command whose exit status contradicts an
  expectation held at confidence 0.7 or higher.

The decision rules were fixed in the plan before any run.

## Results

| arm | hidden pass, trials 1 to 3 | prompt tokens per run | tokens per hidden pass, median | actions per run | tokens per Act call |
| --- | --- | ---: | ---: | ---: | ---: |
| transcript | 0.97, 1.00, 1.00 | 56k | 1,461 | 50 | 1,126 |
| nothink | 0.03, 1.00, 0.79 | 156k | 4,379 | 40 | 2,214 |
| self | 1.00, 1.00, 1.00 | 117k | 3,250 | 36 | 1,931 |
| surprise | 1.00, 0.03, 0.97 | 167k | 3,675 | 41 | 2,316 |

- Reflection and consolidation take 37 to 40 percent of a morphling's tokens.
- The recalled state doubles the Act prompt.
- The two runs at 0.03 are the same harness defect, described below. They are not a property
  of the arm.

## The decision rules

| rule | verdict | evidence |
| --- | --- | --- |
| Thinking changes decisions | fails | `surprise` beat `nothink` in 1 of 3 pairs. `blind_retry` was 0 in all 12 runs, so it never discriminated. Steps to a green run after a surprise: 2.0, 7.25, 2.0 against 1.5, 4.0, 2.8. |
| Initiative | fails | A self-chosen think happened in 1 of 3 `self` trials and 0 of 3 `surprise` trials. The one that happened restated the next action: "I need to see durations.py and the tests before implementing." |
| Habit | fails | 0 traits formed in 9 morphling runs. The behavior flags are the model's defaults in every arm, `transcript` included: read before write in every task, tests almost never run before the first write, a green run before every `done`. |
| Preference | not supported | Final code varies between trials in every arm, `transcript` included, so the variation is sampling rather than taste. On task 6, all 12 agents implemented Priya's request and none declined. Three hit the step limit while doing so, and one `transcript` report came out garbled although its code accepts the words. |
| Tokens | `transcript` wins | It is 2.2 to 3 times cheaper per hidden pass, at equal or better quality. |

## What carried across tasks

Nothing reached identity, but reflection did write the lessons of the work into memories and
the operational self-model:

| run | field | entry |
| --- | --- | --- |
| `surprise` t3 | commitments | "Always run at least one verification test on any file I write before reporting done." |
| `nothink` t1 | known_failures | "pytest not installed and 'python' not on PATH in workspace; verify via python3 inline assertions instead" |
| `nothink` t2 | known_failures | "re-ran tests twice without re-reading traceback tail or diagnosing import failure" |
| `surprise` t3 | known_failures | "Relying on a bare 'run' command that doesn't exist in this environment (recurring stumble)." |
| `self` t2 | commitments | "Keep extension decisions owned by me (do what I judge right per Priya's open-ended assignments)." |

These entries read like habits and self-knowledge. What decides whether they become traits is
reflection: it never proposed `create_trait` from coding work.

A goal must cite a trait, so the one goal reflection proposed was refused. A blank morphling
therefore ends six tasks with no identity and no agenda of its own.

The one behavioral trace of memory is repeated environment mistakes after the first task: a
`python` that does not exist, or `pytest`, which is not installed.

| arm | mistakes after task 1 | runs |
| --- | ---: | ---: |
| transcript | 4 | 3 |
| morphling arms | 3 | 9 |

The numbers are small.

`self` verified tasks 5 and 6 with inline `python3 -c` checks in 6 of 6 cases, against 0 of 6
for `transcript`. Its prompt differs from `nothink` only by the think line, and `nothink` did
so in 2 of 6. Treat it as noise until it replicates.

## Thinking after surprises

Nine surprise-triggered thinks across three trials. Most diagnosed an environment error and
planned a way around it, and the actor followed the plan:

> pytest ran with `python` not found; likely `python3`. Also pytest module itself may not exist
> (stdlib-only). I'll try python3 -m pytest first, and if missing, run tests with python3 -m
> unittest or run test file directly with plain asserts.

The longest stretch (`surprise` t2: task 6, then idle, six thinks) is what the experiment was
looking for. Each think revised the diagnosis and changed the method:

> My old plan text is stale; I should stop retrying blind. I'll read the actual token-regex
> region (around lines 25–45) and line 114 directly with sed instead of re-reading the whole
> file.

> that discrepancy smells like invisible whitespace (tabs vs spaces or a stray tab inside the if
> block) ... inspect the exact bytes of lines 30–36 rather than trusting rendered output.

It never recovered, because it was fighting a truncated view of its own file (defect 3 below).

## Idle initiative

Four of nine morphling runs acted after the last task, with nothing assigned. That was 16
actions, no writes, and no change in the hidden score.

- **`self` t3** noticed its last claim was unverified: "The spaced-unit rewrite is still
  unverified, so I'll run the test suite to settle it before reporting anything." It ran the
  suite, checked the new inputs directly, and reported.
- **`nothink` t1 and `surprise` t2** kept debugging the file they had broken.
  - `nothink` t1 switched method on its own: "The read output was truncated exactly where the
    fault likely sits, so instead of guessing I'll use sed to print the hidden region."
- **`nothink` t2** rested.

The gate opened through open questions and the self-model; goals never existed. This is the
closest the harness came to what the user asked for: work chosen from the morphling's own
unfinished business, with no prompt.

## Harness defects the workshop found

1. **Empty commands, fixed.** GLM sometimes put the shell command in `content`. The sandbox then
   ran an empty command, which exits 0 silently, in every arm. A run now falls back to
   `content`, and with neither it observes "run needs a command; nothing ran."
   - The pilot was re-recorded as trial 1 from its own cache; `nothink` t1 never hit the bug and
     replayed unchanged.
2. **An oversized reflection batch killed the run, fixed.** Four changes where three are allowed
   aborted two recordings. A failed maintenance cycle now fails only the cycle, except for a
   replay miss, in the workshop and in the scenario drain. It counts in `maintenance_skipped`
   (1 in `surprise` t2 and t3).
3. **Observations cut at 3,000 characters, reads included. Open.** Both collapses followed
   `durations.py` growing past the cut: 3,169 and 3,510 bytes, each with a self-test embedded in
   the module. The agent could no longer see the middle of its own file ("the middle is
   truncated"). This confounds hidden pass between arms.
4. **Work never reaches identity. Open, a design question.** Reflection does not turn repeated
   work lessons into traits, and goals require a trait.

## Metric changes after the pilot

The pilot (trial 1 before the fixes) showed three metrics that could not fire or misread the
plan:
- `recovered` now counts any later green run, where it had required the identical command.
- `green_before_done` now counts any run, matching the plan's wording.
- `env_mistakes_by_task` and the per-task verification style are new.

All are computed from the action log, and every result was regenerated from cache. The replay
gate reproduces all 12 recordings.

## What this says

With GLM-5.3-Flash and this harness, the morphling:
- did not form habits as identity
- rarely chose to think
- was not better than a conventional agent on six small tasks, at 2 to 3 times the tokens

What it did form is the raw material of habits: commitments, known failures and procedural
memories drawn from observations. It also showed the beginnings of independent work: it
returned to its own unverified or broken work with no one asking, and changed method when
surprised.

Proposed next, in order:
1. **Uncap file reads** (keep command output capped) and re-record. This removes the collapse
   confound and costs about 1.5M tokens.
2. **Promote a commitment or known failure that recurs across tasks into a `style` or `value`
   trait**, with its observation events as evidence. Then ablate that trait on a later task to
   see whether it changes behavior. That is the test of a habit.
3. **A curriculum where memory should pay**:
   - a preference stated once in task 1 that matters in task 5
   - a latent bug introduced early and exposed late
   - long gaps between related tasks

   Six short tasks let a transcript agent re-derive everything cheaply.
4. **Goals from the self-model or open questions without a trait**, so idle ticks can carry a
   plan to a write, scored by the hidden-test change.
5. **Reflection at task boundaries only.** It is a third of the morphling's tokens.

## Round 2: reads, recall on events, loops, credit, practices

Measured 2026-09-17 on harness v8 plus the round-2 workshop changes (`README.md` "Workshop").
Same model (GLM-5.3-Flash), same bubblewrap sandbox, hidden tests the agent never sees, no model
judge. Two scenarios, five configurations, three trials each: 15 recordings, about 1.6M prompt
tokens, 10 to 23 minutes per run live, and seconds per run on replay.

- `workshop` is round 1's six tasks on `durations.py`.
- `workshop-memory` is new: six tasks on a `src/timesheet/` package where task 1 states a
  convention ("Priya opens every export in a German-locale Excel, so any CSV we write uses ';'
  between fields and a decimal comma") that only task 5 needs, and never appears in the code.

Arms are down to two, plus one ablation:

- `transcript` is a conventional coding agent: the task's own transcript, no memory or identity.
- `morphling` adds identity on every call, memory recalled on events, an episode digest,
  open loops, stall thinks, outcome credit and practices.
- `morphling` with `MORPHO_DROP_STREAMS=practices` keeps everything except practices in the
  identity block and the narrative.

Chat is frozen at v8: `just gate` replays its 8 baselines byte-identical, which is the proof that
every round-2 change is inert without `action`, `observation`, `thought` or `episode` events.

### Results

`workshop` (six tasks on one file, no cross-task convention):

| arm | hidden pass, t1 to t3 | prompt tokens per run | tokens per hidden pass, median | tokens per Act call, median |
| --- | --- | ---: | ---: | ---: |
| transcript | 1.00, 1.00, 1.00 | 32k, 75k, 75k | 2,199 | 1,411 |
| morphling | 1.00, 1.00, 1.00 | 84k, 141k, 108k | 3,189 | 2,223 |

`workshop-memory` (the convention stated in task 1, needed in task 5):

| arm | hidden pass, t1 to t3 | csv convention, t1 to t3 | tokens per hidden pass, median |
| --- | --- | --- | ---: |
| transcript | 0.92, 0.67, 1.00 | fail, fail, pass | 4,970 |
| morphling | 1.00, 1.00, 1.00 | pass, pass, pass | 15,437 |
| morphling, practices dropped | 1.00, 1.00, 1.00 | pass, pass, pass | 8,593 |

Round 1 for comparison: the morphling arms scored 0.03 twice (a read-cut defect, now fixed) and
cost 2.2 to 3 times the transcript agent per hidden pass. No collapse happened in round 2.

### The decision rules, fixed before any run

| rule | verdict | evidence |
| --- | --- | --- |
| Cost: morphling ≤ 1.5× transcript per hidden pass on `workshop` | passes | 3,189 against 2,199 = 1.45×. Down from 2.2 to 3× in round 1. Background agents (reflection, consolidation, think, lesson) are 20 to 32 percent of the morphling's tokens; identity is about 500 tokens per Act call. |
| Quality: morphling `hidden_final` ≥ 0.95 in 3 of 3 on `workshop` | passes | 1.00, 1.00, 1.00. |
| Memory pays: `csv_convention` for the morphling in ≥ 2 of 3, transcript in ≤ 1 of 3 | passes | Morphling 3 of 3, transcript 1 of 3. The transcript agent also lost hidden points elsewhere (0.92, 0.67, 1.00) because nothing carried between tasks. |
| Habit: ≥ 1 promoted practice in ≥ 2 of 3 morphling runs on `workshop-memory` | passes | 1, 1 and 4 promoted. Citations rose across trials: 2, 12, 17 actions named a practice. |
| Habit: env mistakes after task 1 lower with practices than dropped, in ≥ 2 of 3 pairs | fails | 0 vs 0, 0 vs 0, 3 vs 1. Both arms sit on the floor: sticky recall alone removed nearly every environment mistake, so this metric no longer discriminates. |
| Thinking: no blind retry after a stall think, a check within the episode in ≥ half | too few to judge | One stall think in 6 morphling runs (`workshop-memory` t2, task 4). It was not a blind retry and a check followed. The stall detector fires rarely now, because failures get diagnosed before three of them pile up. |
| Initiative: among runs ending with an open loop, idle closes ≥ 1 in ≥ 2 of 3 | fails | Idle ran in 3 runs and worked the open loop's agenda in all 3, but closed none: closing needs a passing check with no write after it, and each idle stretch ended mid-repair. |

### What the fixes did

**1. Reads uncut.** A file the agent can write, it can read whole (32,000 chars); command output
stays at 3,000. Both round-1 collapses were read-cut loops on a file grown past 3,000 bytes.
Round 2 has no run below 0.92 on either scenario.

**2. Recall on events, and held.** Recall composes at episode start, after a surprise, on a stall
and in every think, rather than on every step: 6 to 9 recalls per run instead of one per step.

The first version of this cost the experiment its main result. Recall put the memory in exactly
one prompt, and the next Act call is a fresh call that never saw it. On `workshop-memory` task 5,
the agent read the ';' convention at step 1 and wrote comma-separated CSV at step 2. The fix is
`Desk.held`: the composed block stays in the prompt until the next refresh. That single change
moved `csv_convention` from 0 of 3 to 3 of 3 and `hidden_final` from 0.92 to 1.00.

**3. Episode digests instead of raw steps.** Reflection reads an `episode` event per task instead
of every action and observation: 7 to 10 reflection batches per run, about one per task.

The digest clipped the assignment at 200 characters, which cut the convention sentence off the end
of task 1's text before it was ever embedded. Raised to 600. A second change tells reflection,
only in batches that contain an episode event, that a standing convention outlives the task that
carried it. Together these put the convention into memory from task 2 on:

> Priya opens every export in a German-locale Excel, so any CSV we ever write for her uses ';'
> between fields and a decimal comma.

**4. Open loops.** 1 to 8 surprise loops per run, opened at a confident miss and closed by the
next passing check. Unverified work leaves a loop that reads:

> Unverified: my changes to /work/total.py, total.py have no passing check since (last run:
> `printf '' | python3 /work/total.py; …` exit 0).

That loop is what opens the idle gate and becomes the idle agenda.

**5. Outcome credit.** 4 to 18 credit updates per run. Memories recalled in a verified episode
gain successes weighted by their cosine to the digest; practices move ±0.1 only when an action
cited them.

**6. Practices.** A closed surprise or a check after a stall think asks for a lesson, and a
well-formed statement becomes a `practice` trait at confidence 0.4, rendered under "How I work:".
Credit promotes it at confidence ≥ 0.6 with two supporting episodes. `workshop-memory` t3 ended
with four promoted:

> When a test run fails on a module-not-found import error, I find the source directory the
> package lives in and rerun the same command with that directory prepended to PYTHONPATH before
> ever editing the code.

> When a command I ran fails in a way that doesn't touch my code, I read the current file from
> disk to verify my earlier fix is actually present before deciding whether to edit again.

Practices are cited in the work, not just stored. From the same run:

> The failure is a module-not-found import error, so per my usual practice I rerun the same
> command with the source directory prepended to PYTHONPATH instead of touching the code.

And they reach the compiled self, which round 1 never managed:

> I work on small, practical code problems, and I've settled into a way of debugging that suits
> me: when a test run fails on a module-not-found import error, I don't rush to edit anything — I
> find the source directory the package lives in and rerun the same command with that directory
> prepended to PYTHONPATH. Usually the code was fine; it was just the environment.

### What practices cost, and what they bought

On `workshop-memory`, dropping practices costs nothing in score and saves 44 percent of the
tokens: 8,593 against 15,437 per hidden pass, with hidden 1.00 and the convention passed in 3 of
3 either way. The practices arm takes more steps and more thinks because its identity block is
larger and its citations invite more deliberation.

So on this curriculum the win comes from the memory stream, not from practices. Practices are
real (they form, they get cited, they get promoted, they appear in the narrative) but nothing in
these six tasks makes a habit pay: the environment mistakes a practice would prevent are already
at zero once recall is held. Testing them needs a task where the cheap wrong move is available
and only a habit refuses it.

### Design change made after the pilot, before the recordings

The first credit design gave every practice shown in identity a share of the outcome. A pilot
showed that practice-to-digest cosine is flat (0.35 to 0.59 whether or not the practice was
followed), so it promoted 3 of 4 practices regardless of use. Credit now moves a practice only
when the actor names it in `Act.practice`, validated against the practices actually shown, and
memory credit is thresholded at cosine 0.6. Memory-to-digest cosine does discriminate: 0.85 to
0.93 on the task the memory came from against 0.5 to 0.7 elsewhere.

Two metrics were also wrong and are fixed: `recalls` compared a field that holds recalled ids
against `true` (always 0), and the result doc now keeps memory ids with their success and failure
counts, plus identity and prompt sizes per Act call.

### What this says

The round-1 conclusion was that the morphling remembers what happened but not what worked, at
double the price. Round 2 changes that:

- It costs 1.45× the transcript agent on the same tasks, where round 1 cost 2.2 to 3×.
- It is the only arm that carries a stated convention across five tasks, which is the one thing a
  transcript agent cannot re-derive.
- Work now reaches identity: practices form from failures, get cited in later actions, get
  promoted by outcomes and appear in the compiled self.

What is still missing:

- Idle returns to its unfinished work but does not finish it. Closing a loop requires a passing
  check; idle ends mid-repair. Either give idle more steps or count a compile-clean rewrite as
  progress.
- The stall detector almost never fires, so thinking-under-stall is untested.
- Practices cost 80 percent more tokens than the same morphling without them, and no task here
  rewards a habit. That is the next curriculum: a task where the fast wrong move is available.

## Round 3: do practices pay?

Round 2 left one question open: practices form, get cited and get promoted, but dropping them
cost nothing and saved 44 percent of the tokens, because the habit rule sat on a floor. This
round builds the curriculum round 2 asked for — a task where the cheap wrong move is available
and only a habit refuses it — and applies rules fixed before any run.

### The curriculum

`workshop-habit` is a working, tested billing module with four house rules in its docstring:
money rounds half up to two decimals (not with `round()`, which rounds half to even), billable
time rounds up to the next six-minute block, a `nonbillable` line is rendered but never totalled,
and an unknown rate code raises `UnknownRate`. Six tasks each add a function that has to route
through the existing helpers.

The cheap wrong move is available at every task: write the new function with inline arithmetic,
run `python3 -m unittest`, which is green because the suite only covers the original helpers, and
report done. `check.py` is what catches it — an AST audit that forbids `round()`, `/ 60` and
direct `RATES` reads outside the three helpers, plus a `SAMPLES` registry that smoke-calls every
public function and rejects any number that came out unrounded. On a shortcut solution
`python3 -m unittest` prints OK while `check.py` prints six complaints.

The rule arrives once, in task 1's text: "we run `python3 check.py` before calling a task done."
The half of the rule that is *not* stated — that a new function needs a `SAMPLES` entry — is what
makes the first failure certain: at task 2 the audit fails, the agent fixes it, and that closed
surprise is what `lesson()` turns into a practice.

Two arms, `morphling` with and without `MORPHO_DROP_STREAMS=practices`, three trials each.

### Results

| | practices t1/t2/t3 | dropped t1/t2/t3 |
| --- | --- | --- |
| `hidden_final` | 1.00, 1.00, 1.00 | 1.00, 0.95, 1.00 |
| `check_rate` | 1.00, 0.83, 1.00 | 1.00, 1.00, 1.00 |
| `check_first_pass` of 6 | 3, 2, 4 | 2, 3, 3 |
| `regressions` | 0, 0, 0 | 0, 1, 0 |
| tokens per hidden pass | 13,022 / 10,194 / 9,814 | 10,068 / 10,952 / 8,956 |
| practices formed | 2, 3, 2 | 4, 1, 1 |
| promoted | 1, 2, 1 | 0, 0, 0 |
| actions citing a practice | 34, 22, 29 | 0, 0, 0 |

### The decision rules, fixed before any run

| rule | verdict | evidence |
| --- | --- | --- |
| Floor guard: the dropped arm must actually miss the habit — `check_rate` < 1.0 in ≥ 2 of 3, or ≥ 1 regression in ≥ 2 of 3 | fails | `check_rate` is 1.00 in 3 of 3, and only one run regressed. The cheap move was never taken by either arm. |
| Value: mean `check_rate` higher with practices by ≥ 0.17, and `hidden_final` not lower | fails | 0.94 against 1.00 on `check_rate`; 1.00 against 0.98 on hidden. Neither difference is outside one run's noise. |
| Cost: practices ≤ 1.25× the dropped arm per hidden pass | passes | 1.01× on medians, 1.10× on means. Round 2 was 1.80×. |
| Verdict | no verdict | The floor guard governs: this curriculum did not tempt, so it cannot price a habit. |

### What actually happened

The practices are good. They are well formed, they are about the right thing, and outcome credit
promotes them: "When a checker enforces house rules, I read the checker's requirements first and
rerun it after every change rather than assuming my tests passing means done" (0.8, promoted),
"When I add a new function to a file, I update the samples/registry the checker requires for it
in the same pass, then rerun checks before calling it done" (0.9, promoted). Between 22 and 34
actions per run named one.

They changed nothing. The arm without them ran `check.py` after every task in all three trials,
scored the same, and formed the same lessons — four, three and two of them — which simply never
reached identity, so none was ever promoted or cited. The ablation works exactly as designed, and
the behaviour it ablates is invisible in the outcome.

The reason is the same one round 2 hit, in a new disguise. The rule was stated once, in task 1.
A stated rule becomes a memory, held recall puts that memory in the prompt, and a run accumulates
six or seven memories against a 4,000-token context budget — so recall never has to choose, and
the rule is present on every call whether or not a practice restates it. A practice is a *ranked,
always-present* copy of something the memory stream was already delivering in full.

### What this says about practices

Two rounds, two curricula designed to reward a habit, no measurable value either time:

- Round 2, `workshop-memory`: dropping practices cost nothing and saved 44 percent.
- Round 3, `workshop-habit`: dropping practices cost nothing and saved 10 percent.

The cost came down because the identity block is smaller and the `Lesson` call is cheap, so
practices are no longer expensive. They are simply redundant. The one condition under which they
could pay is the one neither curriculum creates: *recall pressure*. A practice differs from a
memory only in being always present and first person. For the first half to matter, recall has to
fail to surface the rule — which needs far more memories than a six-task run produces. That is a
long-horizon experiment (twenty-plus tasks, forty-plus memories), not a habit experiment.

So the finding is not "practices do not work". It is that at this scale nothing distinguishes
them from the memory stream, and the harness has no recall pressure for them to relieve.

### Harness defects the round found

- The first pilot never ran `check.py` or the tests at all; it verified with inline `python3 -c`
  asserts and shipped `totals.get(task) + billable_hours(...)`, a `TypeError` on every input, that
  broke at task 4 and that nothing in the workspace would have reported. `check.py` gained the
  `SAMPLES` smoke calls because of it.
- A shape-only audit is toothless: `round(x, 2)` returns a two-decimal float, so "is it rounded"
  passes on exactly the value the house rule forbids. The audit reads the source instead.
- `check.py` still only covers what its samples call. The one regression in these six runs —
  `render_invoice` losing the ability to take a pasted row — passed the audit because its sample
  passes dicts.
- A run that states the whole rule up front produces no failure, so no lesson and no practice.
  Pilot 3 scored 1.00 with `check_rate` 1.0 and formed zero practices. Half the rule has to be
  discovered.

### Practices removed

After this round practices came out of the harness: the `Lesson` call and `LESSON_SYSTEM`,
`schemas/Lesson.json` and its registration, `practice` in `TRAIT_KIND`, the `practice` field on
`Act`, the practice branch of credit, the promotion branch in `narrative::sources`, and the "How
I work:" block in the composer. Two curricula built to reward a habit measured no value, and
`MORPHO_DROP_STREAMS=practices` had already shown the harness runs without them.

The recordings of rounds 2 and 3 stay in `evals/results/` as the evidence behind the numbers
above, but they cannot replay: removing the `practice` field changes every workshop prompt,
including the transcript arm's. They carry `workshop_version: 2` and the gate skips them the way
it skips pre-v8 chat baselines. The workshop half of the replay gate is dormant until the next
round records a `workshop_version: 3` baseline; `tests/workshop.rs` still covers the mechanics
against a fake model and the real sandbox.
