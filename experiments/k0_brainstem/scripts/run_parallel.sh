#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
exec "${K0_PYTHON:-python3}" -m experiments.k0_brainstem.train.launcher --artifacts experiments/k0_brainstem/artifacts
