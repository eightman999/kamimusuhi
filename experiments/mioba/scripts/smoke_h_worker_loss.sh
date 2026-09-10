#!/usr/bin/env bash
# Smoke H: worker dies mid-run -> its job goes UNKNOWN -> requeued.
set -u
PY="${MIOBA_PYTHON:-/home/ubuntu/mioba-venv/bin/python}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$REPO" || exit 2
RUNS="$(mktemp -d /tmp/mioba-h-XXXX)"
CFG=experiments/mioba/configs/smoke_mock.yaml
"$PY" -m experiments.mioba.cli --config "$CFG" --runs-dir "$RUNS" \
  start --port 8876 >"$RUNS/coord.log" 2>&1 &
COORD=$!
sleep 3
# worker claims a job then is killed before finishing
"$PY" - <<'EOF'
import httpx, sqlite3, glob, time, os
base = "http://127.0.0.1:8876"
httpx.post(base + "/api/worker/register",
           json={"worker_id": "doomed", "hostname": "h", "gpu": [],
                 "runtime_info": {}, "bench": []})
r = httpx.post(base + "/api/worker/claim",
               json={"worker_id": "doomed", "batch_size": 1})
assert r.status_code == 200, r.status_code
job = r.json()["job_id"]
print("claimed", job)
time.sleep(12)   # lost_after_s = 8 in smoke config
jobs = httpx.get(base + "/api/jobs").json()["jobs"]
j = [x for x in jobs if x["job_id"] == job][0]
print("status now:", j["status"], "attempt:", j["attempt"])
assert j["status"] in ("QUEUED", "UNKNOWN"), j["status"]
print("PASS smoke_h")
EOF
RES=$?
"$PY" -m experiments.mioba.cli --config "$CFG" --runs-dir "$RUNS" stop --timeout 5 >/dev/null 2>&1
kill $COORD 2>/dev/null
[ $RES -eq 0 ] && echo "PASS smoke_h" || { echo "FAIL smoke_h; logs in $RUNS"; exit 1; }
