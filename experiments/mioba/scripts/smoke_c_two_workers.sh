#!/usr/bin/env bash
# Smoke C: coordinator + two workers; each claims a different job.
set -u
PY="${MIOBA_PYTHON:-/home/ubuntu/mioba-venv/bin/python}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$REPO" || exit 2
RUNS="$(mktemp -d /tmp/mioba-c-XXXX)"
CFG=experiments/mioba/configs/smoke_mock.yaml
"$PY" -m experiments.mioba.cli --config "$CFG" --runs-dir "$RUNS" \
  start --port 8871 >"$RUNS/coord.log" 2>&1 &
COORD=$!
"$PY" -m experiments.mioba.workers.worker --coordinator http://127.0.0.1:8871 \
  --backend mock --batch 1 --worker-id w1 --heartbeat-s 1 >"$RUNS/w1.log" 2>&1 &
W1=$!
"$PY" -m experiments.mioba.workers.worker --coordinator http://127.0.0.1:8871 \
  --backend mock --batch 1 --worker-id w2 --heartbeat-s 1 >"$RUNS/w2.log" 2>&1 &
W2=$!
sleep 15
RES=$("$PY" - <<EOF
import httpx, json
s = httpx.get("http://127.0.0.1:8871/api/status").json()
w = httpx.get("http://127.0.0.1:8871/api/workers").json()["workers"]
jobs = httpx.get("http://127.0.0.1:8871/api/jobs").json()["jobs"]
ok = len({x["worker_id"] for x in w}) >= 2 and \
     len({j["claimed_by_worker"] for j in jobs if j["claimed_by_worker"]}) >= 2
print("PASS" if ok else "FAIL", json.dumps(s["counters"]))
EOF
)
echo "$RES"
"$PY" -m experiments.mioba.cli --config "$CFG" --runs-dir "$RUNS" stop --timeout 5 >/dev/null 2>&1
kill $W1 $W2 $COORD 2>/dev/null
case "$RES" in PASS*) exit 0;; *) echo "logs in $RUNS"; exit 1;; esac
