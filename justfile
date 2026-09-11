set dotenv-load

default: check

check: fmt-check clippy test

fmt:
    cargo fmt

fmt-check:
    cargo fmt --check

clippy:
    cargo clippy --all-targets -- -D warnings

test:
    cargo test --release

run:
    cargo run --release --bin morpho

eval scenario *args:
    cargo run --release --bin eval -- evals/scenarios/{{scenario}}.json {{args}}

replay scenario:
    just eval {{scenario}} --strict --baseline evals/results/{{scenario}}.base.json

replay-control scenario:
    just eval {{scenario}} --control --strict --baseline evals/results/{{scenario}}.control.base.json

record scenario:
    just eval {{scenario}} --label base

gate:
    for s in dana project long; do just replay $s && just replay-control $s || exit 1; done
