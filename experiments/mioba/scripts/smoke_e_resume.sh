#!/usr/bin/env bash
# Smoke E: checkpoint, SIGKILL, resume; genomes/evaluations preserved.
set -u
PY="${MIOBA_PYTHON:-/home/ubuntu/mioba-venv/bin/python}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$REPO" || exit 2
RUNS="$(mktemp -d /tmp/mioba-e-XXXX)"
CFG=experiments/mioba/configs/smoke_mock.yaml
"$PY" -m experiments.mioba.cli --config "$CFG" --runs-dir "$RUNS" \
  start --port 8873 >"$RUNS/coord1.log" 2>&1 &
COORD=$!
sleep 3
"$PY" -m experiments.mioba.workers.worker --coordinator http://127.0.0.1:8873 \
  --backend mock --batch 1 --worker-id w1 --heartbeat-s 1 >"$RUNS/w1.log" 2>&1 &
W1=$!
sleep 8
"$PY" -m experiments.mioba.cli --config "$CFG" --runs-dir "$RUNS" checkpoint
kill -9 $W1 $COORD 2>/dev/null; sleep 1
EXP=$(ls "$RUNS" | head -1)
"$PY" -m experiments.mioba.cli --config "$CFG" --runs-dir "$RUNS" \
  start --resume "$EXP" --port 8874 >"$RUNS/coord2.log" 2>&1 &
COORD2=$!
sleep 4
RES=$("$PY" - <<EOF
import httpx
s = httpx.get("http://127.0.0.1:8874/api/status").json()
g = httpx.get("http://127.0.0.1:8874/api/genomes").json()["genomes"]
print("PASS" if s["status"]=="running" and len(g)>=4 else "FAIL", s["status"], len(g))
EOF
)
echo "$RES"
kill $COORD2 2>/dev/null
case "$RES" in PASS*) exit 0;; *) echo "logs in $RUNS"; exit 1;; esac
