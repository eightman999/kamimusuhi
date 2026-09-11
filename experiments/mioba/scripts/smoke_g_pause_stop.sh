#!/usr/bin/env bash
# Smoke G: pause blocks claims; stop transitions to stopped + checkpoint row.
set -u
PY="${MIOBA_PYTHON:-/home/ubuntu/mioba-venv/bin/python}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$REPO" || exit 2
RUNS="$(mktemp -d /tmp/mioba-g-XXXX)"
CFG=experiments/mioba/configs/smoke_mock.yaml
"$PY" -m experiments.mioba.cli --config "$CFG" --runs-dir "$RUNS" \
  start --port 8875 >"$RUNS/coord.log" 2>&1 &
COORD=$!
sleep 3
"$PY" -m experiments.mioba.cli --config "$CFG" --runs-dir "$RUNS" pause
RES1=$("$PY" - <<'EOF'
import httpx
r = httpx.post("http://127.0.0.1:8875/api/worker/claim",
               json={"worker_id":"wX","batch_size":1})
print("PASS" if r.status_code == 204 else "FAIL", r.status_code)
EOF
)
echo "pause-claim: $RES1"
"$PY" -m experiments.mioba.cli --config "$CFG" --runs-dir "$RUNS" resume
"$PY" -m experiments.mioba.cli --config "$CFG" --runs-dir "$RUNS" stop --timeout 5
sleep 6
RES2=$("$PY" - "$RUNS" <<'EOF'
import sqlite3, sys, glob
db = glob.glob(sys.argv[1] + "/*/lineage.sqlite")[0]
c = sqlite3.connect(db)
st = c.execute("select status from experiments").fetchone()[0]
ck = c.execute("select count(*) from checkpoints").fetchone()[0]
print("PASS" if st == "stopped" and ck >= 1 else "FAIL", st, ck)
EOF
)
echo "stop: $RES2"
kill $COORD 2>/dev/null
case "$RES1$RES2" in *FAIL*) echo "logs in $RUNS"; exit 1;; esac
echo "PASS smoke_g"; exit 0
