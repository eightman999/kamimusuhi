#!/usr/bin/env bash
# Smoke A/B: backend initialise + run + summary on a chosen device/backend.
# On the GPU host: ./smoke_a_b_backend.sh --device cuda:0 --backend torch
set -u
DEVICE="cpu"; BACKEND="mock"
while [ $# -gt 0 ]; do
  case "$1" in
    --device) DEVICE="$2"; shift 2;;
    --backend) BACKEND="$2"; shift 2;;
    *) echo "unknown arg $1"; exit 2;;
  esac
done
PY="${MIOBA_PYTHON:-/home/ubuntu/mioba-venv/bin/python}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$REPO" || exit 2
"$PY" - <<EOF
from experiments.mioba.development.phenotype import develop
from experiments.mioba.fba.registry import get_backend
from experiments.mioba.genome.schema import fba0_genome

b = get_backend("$BACKEND", synthetic=True, synthetic_neurons=500)
b.initialize(develop(fba0_genome()), batch_size=2, seed=0, device="$DEVICE")
stats = b.run(100)
s = b.get_state_summary()
assert stats["simulated_ms"] >= 99.9, stats
assert s["t_ms"] > 0 and "mean_rate_hz" in s, s
print("PASS backend=$BACKEND device=$DEVICE", stats)
EOF
status=$?
[ $status -eq 0 ] && echo "PASS smoke_a_b" || echo "FAIL smoke_a_b"
exit $status
