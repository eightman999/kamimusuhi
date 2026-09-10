#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../../.."
exec "${K0_PYTHON:-python3}" -m experiments.k0_brainstem.train.trainer --config experiments/k0_brainstem/configs/smoke.yaml --artifacts experiments/k0_brainstem/artifacts --run-id "${K0_RUN_ID:-smoke-$(date +%s)}" --device "${K0_DEVICE:-cuda:0}"
