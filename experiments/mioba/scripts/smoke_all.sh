#!/usr/bin/env bash
# Run all MIOBA smoke tests A-H sequentially; write smoke-results.txt
# to the directory given as $1 (or a temp dir).
set -u
OUT="${1:-$(mktemp -d /tmp/mioba-smoke-all-XXXX)}"
mkdir -p "$OUT"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPORT="$OUT/smoke-results.txt"
: > "$REPORT"

have_gpu() { command -v nvidia-smi >/dev/null 2>&1; }

run() {
  name="$1"; shift
  if "$@" >>"$REPORT" 2>&1; then
    echo "PASS $name" | tee -a "$REPORT"
  else
    echo "FAIL $name" | tee -a "$REPORT"
  fi
}

if have_gpu; then
  run smoke_a_b_backend "$HERE/smoke_a_b_backend.sh" --device cuda:0 --backend torch
else
  echo "SKIP smoke_a_b_backend: no GPU" | tee -a "$REPORT"
  # still exercise the backend on CPU with the mock
  run smoke_a_b_backend_cpu "$HERE/smoke_a_b_backend.sh" --device cpu --backend mock
fi
run smoke_c_two_workers "$HERE/smoke_c_two_workers.sh"
run smoke_d_genome_eval "$HERE/smoke_d_genome_eval.sh"
run smoke_e_resume     "$HERE/smoke_e_resume.sh"
run smoke_f_gui        "$HERE/smoke_f_gui.sh"
run smoke_g_pause_stop "$HERE/smoke_g_pause_stop.sh"
run smoke_h_worker_loss "$HERE/smoke_h_worker_loss.sh"
echo "---" | tee -a "$REPORT"
echo "results in $REPORT"
