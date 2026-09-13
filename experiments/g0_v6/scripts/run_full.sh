#!/usr/bin/env bash
set -uo pipefail

: "${PYTHON:=python3}"
export CUBLAS_WORKSPACE_CONFIG=:4096:8
export OMP_NUM_THREADS=2
export OPENBLAS_NUM_THREADS=2
export MKL_NUM_THREADS=2

"$PYTHON" -m pytest experiments/g0_v6/tests -q || exit "$?"
"$PYTHON" -m experiments.g0_v6.src.run audit || exit "$?"
"$PYTHON" -m experiments.g0_v6.src.run protocol-lock || exit "$?"
"$PYTHON" -m experiments.g0_v6.src.run controls || exit "$?"
"$PYTHON" -m experiments.g0_v6.src.run smoke --device cuda:0 --max-epochs 1 || exit "$?"

mkdir -p experiments/g0_v6/results/logs
run_group() {
    local device="$1"
    shift
    local code=0
    for seed in "$@"; do
        "$PYTHON" -m experiments.g0_v6.src.run train --seed "$seed" --device "$device" \
            >"experiments/g0_v6/results/logs/train_seed${seed}.log" 2>&1 || code=1
    done
    return "$code"
}

run_group cuda:0 0 2 4 & pid0=$!
run_group cuda:1 1 3 & pid1=$!
code=0
wait "$pid0" || code=1
wait "$pid1" || code=1
[ "$code" -eq 0 ] || exit "$code"

for seed in 0 1 2 3 4; do
    "$PYTHON" -m experiments.g0_v6.src.run evaluate --seed "$seed" || exit "$?"
done
"$PYTHON" -m experiments.g0_v6.src.run aggregate || exit "$?"
"$PYTHON" -m experiments.g0_v6.src.report
