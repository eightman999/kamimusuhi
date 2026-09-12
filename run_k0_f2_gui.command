#!/bin/zsh
set -eu
K0F2_ROOT="${0:A:h}"
K0F2_PYTHON="${K0F2_GUI_PYTHON:-$K0F2_ROOT/.venv-k0-monitor/bin/python}"
if [[ ! -x "$K0F2_PYTHON" && -x "$K0F2_ROOT/../kamimusuhi-k0/.venv-k0-monitor/bin/python" ]]; then
  K0F2_PYTHON="$K0F2_ROOT/../kamimusuhi-k0/.venv-k0-monitor/bin/python"
fi
if [[ ! -x "$K0F2_PYTHON" ]]; then
  print -u2 "GUI用Pythonを K0F2_GUI_PYTHON で指定してください（PyQt5・pyqtgraph）。"
  exit 1
fi
cd "$K0F2_ROOT"
exec "$K0F2_PYTHON" -m experiments.k0_f2_interoception_confirmatory.gui --artifacts "${K0F2_ARTIFACTS:-$K0F2_ROOT/experiments/k0_f2_interoception_confirmatory/artifacts/review}" "$@"
