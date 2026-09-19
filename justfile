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
    just eval {{scenario}} --strict --baseline evals/results/{{scenario}}.v8.json --db evals/cache/{{scenario}}.v8.db

replay-control scenario:
    just eval {{scenario}} --control --strict --baseline evals/results/{{scenario}}.control.base.json

record scenario:
    just eval {{scenario}} --label v8 --db evals/cache/{{scenario}}.v8.db

# Context selection against the recorded journal, no model calls (`just replay` writes the db).
recompose scenario *args:
    just eval {{scenario}} --recompose evals/cache/{{scenario}}.v8.db --baseline evals/results/{{scenario}}.v8.json {{args}}

# Zero named context streams, e.g. `just ablate dana recent` (env on the command line only).
ablate scenario streams *args:
    MORPHO_DROP_STREAMS={{streams}} just eval {{scenario}} --label v8-drop-{{replace(streams, ",", "-")}} {{args}}

control-full scenario *args:
    CONTEXT_TOKEN_BUDGET=1000000 MAX_PROMPT_TOKENS=1000000 just eval {{scenario}} --control --label full {{args}}

trial scenario n *args:
    just eval {{scenario}} --trial {{n}} --label v8-t{{n}} {{args}}

# Coding tasks in a sandbox, e.g. `just workshop workshop-memory morphling 1`; arms: transcript
# morphling. Ablate with streams on the command line: `MORPHO_DROP_STREAMS=identity just workshop ...`.
workshop scenario arm trial *args:
    BACKGROUND_DAILY_TOKEN_BUDGET=100000000 just eval {{scenario}} --arm {{arm}} --trial {{trial}} {{args}}

replay-workshop scenario arm trial:
    just workshop {{scenario}} {{arm}} {{trial}} --strict --baseline evals/results/{{scenario}}.{{arm}}.t{{trial}}.json

gate:
    cargo test --release --test replay -- --nocapture

# Local traces and metrics: Grafana on :3000, OTLP on :4317/:4318 (set OTEL_EXPORTER_OTLP_ENDPOINT).
otel:
    docker run --rm -d --name otel-lgtm -p 3000:3000 -p 4317:4317 -p 4318:4318 grafana/otel-lgtm

# LongMemEval, adapted: `just longmem-gen <longmemeval_oracle.json> 20`, then `just longmem`.
# The adapter drops the haystack's assistant turns, so the score is not a published-benchmark
# number; see evals/longmem.py.
longmem-gen dataset n="20" prefix="longmem":
    python3 evals/longmem.py {{dataset}} --n {{n}} --prefix {{prefix}}

longmem prefix="longmem" *args:
    #!/usr/bin/env bash
    set -euo pipefail
    for f in evals/scenarios/{{prefix}}-*.json; do
        name=$(basename "$f" .json)
        [[ $name == *.meta ]] && continue
        just eval "$name" --label lme {{args}}
    done

longmem-report *args:
    python3 evals/longmem.py --report {{args}}
