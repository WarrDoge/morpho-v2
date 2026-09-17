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
