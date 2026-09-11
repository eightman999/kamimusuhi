#!/usr/bin/env bash
# GPU startup benchmark — run on the GPU host.
set -u
DEVICE="${1:-cuda:0}"
BACKEND="${2:-torch}"
PY="${MIOBA_PYTHON:-python3}"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
cd "$REPO" || exit 2
exec "$PY" -m experiments.mioba.cli bench --device "$DEVICE" --backend "$BACKEND"
