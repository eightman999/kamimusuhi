#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
PYTHON="${K0_PYTHON:-$PWD/.venv-k0/bin/python}"
exec "$PYTHON" -m experiments.k0_brainstem.monitor.server "$@"
