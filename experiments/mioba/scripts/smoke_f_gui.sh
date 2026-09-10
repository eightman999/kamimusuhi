#!/usr/bin/env bash
# Smoke F: Observatory GUI endpoints on a live mock run.
set -u
PY="${MIOBA_PYTHON:-/home/ubuntu/mioba-venv/bin/python}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$REPO" || exit 2
RUNS="$(mktemp -d /tmp/mioba-f-XXXX)"
CFG=experiments/mioba/configs/smoke_mock.yaml
"$PY" -m experiments.mioba.cli --config "$CFG" --runs-dir "$RUNS" \
  start --port 8872 >"$RUNS/coord.log" 2>&1 &
COORD=$!
"$PY" -m experiments.mioba.workers.worker --coordinator http://127.0.0.1:8872 \
  --backend mock --batch 1 --worker-id wf1 --heartbeat-s 1 >"$RUNS/wf1.log" 2>&1 &
W1=$!
"$PY" -m experiments.mioba.workers.worker --coordinator http://127.0.0.1:8872 \
  --backend mock --batch 1 --worker-id wf2 --heartbeat-s 1 >"$RUNS/wf2.log" 2>&1 &
W2=$!
sleep 12
RES=$("$PY" - <<'EOF'
import httpx
base = "http://127.0.0.1:8872"
try:
    r = httpx.get(base + "/")
    assert r.status_code == 200 and "MIOBA Observatory" in r.text
    d = httpx.get(base + "/api/gui/dashboard").json()
    assert len(d["workers"]) >= 2, d["workers"]
    assert all(w["kind"] == "LIVE" for w in d["workers"])
    g = httpx.get(base + "/api/gui/genomes").json()["genomes"]
    assert len(g) >= 1
    t = httpx.get(base + "/api/gui/telemetry",
                  params={"minutes": 5}).json()
    assert len(t["telemetry"]) >= 1
    print("PASS smoke_f")
except Exception as exc:
    print("FAIL smoke_f:", type(exc).__name__, exc)
EOF
)
echo "$RES"
"$PY" -m experiments.mioba.cli --config "$CFG" --runs-dir "$RUNS" stop --timeout 5 >/dev/null 2>&1
kill $W1 $W2 $COORD 2>/dev/null
case "$RES" in PASS*) exit 0;; *) echo "logs in $RUNS"; exit 1;; esac
