# Resume checkpoint

Updated 2026-09-14. Final v3 validation is complete; no live evaluation is running.
See [the streams report](evals/STREAMS.md) and [the earlier audit](evals/REVIEW.md).

## Intent and decisions

One evolving agent, one durable FIFO, one embedded libSQL database and coordinator.
Multiple speakers share knowledge with attribution. Eight memory views compose fresh context
for each request; fixed policy stays separate from recalled data. Privately staged turns
publish state, reply and completion atomically. No new infrastructure or dependencies.

`CONVERSATION.md` and `SPEC.md` stay local and ignored; neither belongs on the remote branch.

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

## Validation and follow-up

- [x] Complete continuity, all 151 scenario turns, and twelve paired causal calls; strictly
  replay three v3 recordings plus three historical controls. Probe scores: 9/10, 8/8, 29/30.
  Long maintenance paused within its daily budget with a durable backlog. Quality/cost
  limitations are recorded in `evals/STREAMS.md`.
- [ ] Improve semantic answer completeness, unsupported temporal/outcome claims and retrieval
  quality based on the recorded misses. Structural validation does not establish truth.
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
target/release/eval evals/scenarios/dana.json --label v3 --baseline evals/results/dana.base.json --audit-dir evals/audit/dana-next
```

Strict replay after its result exists:

```sh
target/release/eval evals/scenarios/dana.json --strict --baseline evals/results/dana.v3.json
```
