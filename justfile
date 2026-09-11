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
    just eval {{scenario}} --strict --baseline evals/results/{{scenario}}.v2.json

replay-control scenario:
    just eval {{scenario}} --control --strict --baseline evals/results/{{scenario}}.control.base.json

record scenario:
    just eval {{scenario}} --label v2

gate:
    cargo test --release --test replay -- --nocapture
