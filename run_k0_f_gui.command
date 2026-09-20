#!/bin/zsh
set -eu
K0F_ROOT="${0:A:h}"
K0F_PYTHON="${K0F_GUI_PYTHON:-$K0F_ROOT/.venv-k0-monitor/bin/python}"
if [[ ! -x "$K0F_PYTHON" && -x "$K0F_ROOT/../kamimusuhi-k0/.venv-k0-monitor/bin/python" ]]; then
  K0F_PYTHON="$K0F_ROOT/../kamimusuhi-k0/.venv-k0-monitor/bin/python"
fi
if [[ ! -x "$K0F_PYTHON" ]]; then
  print -u2 "GUI 用 Python が未設定です。PyQt5 と pyqtgraph を含む環境を K0F_GUI_PYTHON で指定してください。"
  exit 1
fi
cd "$K0F_ROOT"
exec "$K0F_PYTHON" -m experiments.k0_f_interoception.gui --artifacts "${K0F_ARTIFACTS:-$K0F_ROOT/experiments/k0_f_interoception/artifacts/review}" "$@"
