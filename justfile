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
    just eval {{scenario}} --strict --baseline evals/results/{{scenario}}.v8.json

replay-control scenario:
    just eval {{scenario}} --control --strict --baseline evals/results/{{scenario}}.control.base.json

record scenario:
    just eval {{scenario}} --label v8

# Zero named context streams, e.g. `just ablate dana recent` (env on the command line only).
ablate scenario streams *args:
    MORPHO_DROP_STREAMS={{streams}} just eval {{scenario}} --label v8-drop-{{replace(streams, ",", "-")}} {{args}}

control-full scenario *args:
    CONTEXT_TOKEN_BUDGET=1000000 MAX_PROMPT_TOKENS=1000000 just eval {{scenario}} --control --label full {{args}}

trial scenario n *args:
    just eval {{scenario}} --trial {{n}} --label v8-t{{n}} {{args}}

gate:
    cargo test --release --test replay -- --nocapture

# Local traces and metrics: Grafana on :3000, OTLP on :4317/:4318 (set OTEL_EXPORTER_OTLP_ENDPOINT).
otel:
    docker run --rm -d --name otel-lgtm -p 3000:3000 -p 4317:4317 -p 4318:4318 grafana/otel-lgtm
