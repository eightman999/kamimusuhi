#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
PYTHON="${K0_GUI_PYTHON:-$PWD/.venv-k0-monitor/bin/python}"
PORT="${K0_MONITOR_PORT:-8097}"
SSH_HOST="${K0_SSH_HOST:-llm_master_now}"
if [[ "${K0_NO_TUNNEL:-0}" != "1" ]]; then
  ssh -N -o ExitOnForwardFailure=yes -o ServerAliveInterval=15 -L "127.0.0.1:${PORT}:127.0.0.1:8097" "$SSH_HOST" &
  TUNNEL_PID=$!
  trap 'kill "$TUNNEL_PID" 2>/dev/null || true' EXIT
  sleep 1
  if ! kill -0 "$TUNNEL_PID" 2>/dev/null; then
    echo "SSH tunnel failed. Reuse an existing tunnel with K0_NO_TUNNEL=1."
    exit 1
  fi
fi
"$PYTHON" -m experiments.k0_brainstem.monitor.mac_gui --server "http://127.0.0.1:${PORT}"
