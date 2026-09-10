#!/usr/bin/env bash
# Smoke D: one genome -> one evaluation row.
set -u
PY="${MIOBA_PYTHON:-/home/ubuntu/mioba-venv/bin/python}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$REPO" || exit 2
RUNS="$(mktemp -d /tmp/mioba-d-XXXX)"
CFG=experiments/mioba/configs/smoke_mock.yaml
"$PY" -m experiments.mioba.cli --config "$CFG" --runs-dir "$RUNS" \
  start --port 8872 >"$RUNS/coord.log" 2>&1 &
COORD=$!
"$PY" -m experiments.mioba.workers.worker --coordinator http://127.0.0.1:8872 \
  --backend mock --batch 1 --worker-id w1 --heartbeat-s 1 >"$RUNS/w1.log" 2>&1 &
W1=$!
for i in $(seq 1 20); do
  N=$("$PY" - <<'EOF'
import httpx
try:
    n = httpx.get("http://127.0.0.1:8872/api/evaluations").json()["evaluations"]
    print(len(n))
except Exception:
    print(0)
EOF
)
  [ "${N:-0}" -ge 1 ] && break
  sleep 1
done
"$PY" -m experiments.mioba.cli --config "$CFG" --runs-dir "$RUNS" stop --timeout 5 >/dev/null 2>&1
kill $W1 $COORD 2>/dev/null
if [ "${N:-0}" -ge 1 ]; then echo "PASS smoke_d ($N evaluations)"; exit 0
else echo "FAIL smoke_d; logs in $RUNS"; exit 1; fi
