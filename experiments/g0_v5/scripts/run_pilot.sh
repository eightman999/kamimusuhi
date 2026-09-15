#!/usr/bin/env bash
set -uo pipefail
: "${PYTHON:=python3}"
export CUBLAS_WORKSPACE_CONFIG=:4096:8
export OMP_NUM_THREADS=2
export OPENBLAS_NUM_THREADS=2
export MKL_NUM_THREADS=2
"$PYTHON" -m experiments.g0_v5.src.run controls || exit "$?"
run_gpu() {
  local device="$1"; shift
  local code=0
  for method in "$@"; do
    "$PYTHON" -m experiments.g0_v5.src.run pilot --method "$method" --device "$device" || code=1
  done
  return "$code"
}
run_gpu cuda:0 cpc jepa & pid0=$!
run_gpu cuda:1 gru vicreg & pid1=$!
code=0
wait "$pid0" || code=1
wait "$pid1" || code=1
"$PYTHON" -m experiments.g0_v5.src.report || code=1
exit "$code"
