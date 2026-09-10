#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")"
PYTHON="${K0_GUI_PYTHON:-$PWD/.venv-k0-monitor/bin/python}"
PORT="${K0_MONITOR_PORT:-8097}"
SSH_HOST="${K0_SSH_HOST:-}"
if [[ "${K0_NO_TUNNEL:-0}" != "1" ]]; then
  if [[ -z "$SSH_HOST" && -f "$PWD/.local/connections/k0-ssh-host" ]]; then
    IFS= read -r SSH_HOST < "$PWD/.local/connections/k0-ssh-host"
  fi
  if [[ -z "$SSH_HOST" || "$SSH_HOST" == -* || "$SSH_HOST" == *[[:space:]]* ]]; then
    echo "接続先を K0_SSH_HOST または .local/connections/k0-ssh-host に指定してください。"
    exit 1
  fi
  ssh -N -o ExitOnForwardFailure=yes -o ServerAliveInterval=15 -L "127.0.0.1:${PORT}:127.0.0.1:8097" "$SSH_HOST" &
  TUNNEL_PID=$!
  trap 'kill "$TUNNEL_PID" 2>/dev/null || true' EXIT
  sleep 1
  if ! kill -0 "$TUNNEL_PID" 2>/dev/null; then
    echo "SSHトンネルに接続できませんでした。既存のトンネルを使う場合は K0_NO_TUNNEL=1 を指定してください。"
    exit 1
  fi
fi
"$PYTHON" -m experiments.k0_brainstem.monitor.mac_gui --server "http://127.0.0.1:${PORT}"
