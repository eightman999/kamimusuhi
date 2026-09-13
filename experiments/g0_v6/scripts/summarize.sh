#!/usr/bin/env bash
set -euo pipefail
: "${PYTHON:=python3}"
"$PYTHON" -m experiments.g0_v6.src.run aggregate
"$PYTHON" -m experiments.g0_v6.src.report
