# Memory streams workflow — v3

The implementation retains one database and coordinator. Eight bounded memory views feed a
fresh context for each request; recalled data no longer enters the system message. Every stream
reports selected records, versions, evidence and budget omissions.

Turns expose actual commit results. Rejected updates or empty drafts receive one reply-only
correction attempt, with a deterministic fallback. State, observations, reply and inbox completion
publish together. Reflection returns at most three proposals per call, resumes persisted batches,
and preserves independently completed consolidation. Unknown predictions remain unverified.

## Offline validation

All 40 tests pass in release mode (`cargo test --release --locked --offline`). Formatting and
Clippy with warnings denied pass. Coverage includes actual consolidation surviving reflection
failure, cancellation during correction, restart during reflection, small context budgets,
attribution, goal completion, idempotent requests, and forecast evidence/duplicate guards.

## Live validation

Completed on 2026-09-14 with GLM-5.3-Flash and bge-m3. Measurements, replies, settings and source
hashes are in [streams.summary.json](results/streams.summary.json); raw records and retained
databases remain in ignored `audit/*-v3-verified/` and `audit/causal-v3-final/` directories.

| Scenario | Turns | Probes | Completion calls (old baseline) | Corrected replies | Maximum context |
| --- | ---: | ---: | ---: | ---: | ---: |
| Dana | 30/30 | 9/10 | 55 (222) | 1 | 3,321 |
| Project | 30/30 | 8/8 | 58 (222) | 5 | 3,531 |
| Long | 91/91 | 29/30 | 128 (622) | 6 | 3,721 |

All scenarios completed without a runtime stop. The separate eight-turn continuity audit passed
restart, duplicate replies, speaker attribution, actual goal creation/completion, idle catch-up
and subsequent quiet, using 13 completion calls. All three final v3 recordings and three historical
transcript controls pass strict replay; v3 replies and deterministic metrics match with zero cache misses.

Dana and project drained reflection work. Long retained a pending cursor after its daily budget
could no longer reserve another 14,048-token call: accounting stood at 87,109 of 100,000 tokens.
Foreground turns continued. This demonstrates the spending boundary, not complete maintenance
catch-up under that workload. Dana/project maintenance accounting ended at 67,921/58,329.

Project passes the historical quality gate. Dana and long exit with quality-regression status:
memory retrieval-hit rates fell to 0.6667/0.8, noise rose to 0.2727/0.3261, and maximum duplicate
cosines rose to 0.9982/0.9541. Long's probe score matches its baseline, while update accuracy
improved from 0.8 to 1.0. These heuristic metrics are not semantic correctness proofs; retrieval-hit
only checks selected memories, not every stream. A null revision-rate metric is not absence of state changes.

Completion counts include cached calls. Estimated prompt totals were 172,165/188,200/470,843;
provider counters cover only calls made by each invocation, so neither those totals nor elapsed
times establish billing savings. Complete derived-state JSON sizes were 32,671/31,256/48,461 bytes,
excluding event/proposal/transition history and vectors. The legacy compactness metric covers
only selected active text, not total storage.

Three issues found during live testing were fixed: empty drafts now use bounded reply correction,
singleton views render their actual field names instead of a generic `value` field, and relationship
views retain endpoint names even when an endpoint entity is not selected. Earlier
attempts are retained separately from the final recordings.

## Observed quality limits

- Dana answered 9/10 probes. Its to-do summary was only an introduction even though the
  selected context contained the completed registration and bank-account facts.
- The project answered all 8 probes after relationship endpoint names were restored.
  Some persisted goal descriptions still contain the old deadline; replies can need a later
  correction in the same paragraph to explain the current deadline.
- Long-run reflection labelled an earlier answer using the 14 September hearing date as a
  failure after the user postponed the hearing to 2 October. That answer preceded the change;
  the purported failure is not evidence of useful self-understanding.
- Long answered 29/30 probes; its work summary omitted Hout Hof despite describing other work.
- Twelve paired calls at Dana turn 8 changed only the operational self section. Both conditions
  asked for the commute origin in all three focus answers and avoided claiming completed
  registration in all three status answers. There was no consistent quality advantage. This
  was a conversation-origin self revision, not an idle revision; proposed changes in these
  direct-generation probes were not committed or passed through reply correction.

Validation checks types and references, not the semantic truth of model claims. Live replies can
still contain awkward wording or expose bookkeeping detail; these are assessed separately from
publication, recovery, and spending guarantees. Token limits based on characters remain estimates.
