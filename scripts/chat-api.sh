#!/usr/bin/env bash
# Persistent dialogue using an operator-configured OpenAI-compatible API.
# API keys are read by the adapter from the named environment variable.
set -euo pipefail
cd "$(dirname "$0")/.."

: "${KAMIMUSUHI_API_BASE_URL:?APIのbase URLをKAMIMUSUHI_API_BASE_URLへ設定してください}"
: "${KAMIMUSUHI_API_MODEL:?モデル名をKAMIMUSUHI_API_MODELへ設定してください}"
API_KEY_ENV="${KAMIMUSUHI_API_KEY_ENV:-KAMIMUSUHI_API_KEY}"
RUNTIME_DIR="${1:-.local/text-dialogue}"
SUBJECT="${2:-local-user}"

# Optional MIO source: pin all three identifiers together, never auto-select
# a different genome from a leaderboard or a later generation.
set --
if [[ -n "${KAMIMUSUHI_MIO_URL:-}${KAMIMUSUHI_MIO_EXPERIMENT:-}${KAMIMUSUHI_MIO_GENOME:-}" ]]; then
  : "${KAMIMUSUHI_MIO_URL:?KAMIMUSUHI_MIO_URLを設定してください}"
  : "${KAMIMUSUHI_MIO_EXPERIMENT:?KAMIMUSUHI_MIO_EXPERIMENTを設定してください}"
  : "${KAMIMUSUHI_MIO_GENOME:?KAMIMUSUHI_MIO_GENOMEを設定してください}"
  set -- --mio-url "$KAMIMUSUHI_MIO_URL" \
    --mio-experiment "$KAMIMUSUHI_MIO_EXPERIMENT" --mio-genome "$KAMIMUSUHI_MIO_GENOME"
fi
if [[ -n "${KAMIMUSUHI_MIO_MAX_AGE_SECS:-}" ]]; then
  set -- "$@" --mio-max-age-secs "$KAMIMUSUHI_MIO_MAX_AGE_SECS"
fi

# Validate presence without printing, copying or persisting the credential.
python3 - "$API_KEY_ENV" <<'PY'
import os, re, sys
name = sys.argv[1]
if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", name):
    sys.exit("APIキーの環境変数名が不正です。キーの値は渡さないでください。")
if not os.environ.get(name):
    sys.exit("指定した環境変数にAPIキーが設定されていません。")
PY

cargo build --quiet -p kamimusuhi-runtime
cargo run --quiet -p kamimusuhi-runtime -- research-check --source-root . >/dev/null
if [[ ! -e "$RUNTIME_DIR/runtime.json" && ! -e "$RUNTIME_DIR/kamimusuhi.sqlite" ]]; then
  cargo run --quiet -p kamimusuhi-runtime -- init --dir "$RUNTIME_DIR" >/dev/null
fi

printf 'APIテキスト対話 / 会話相手: %s / 保存先: %s\n' "$SUBJECT" "$RUNTIME_DIR" >&2
cargo run --quiet -p kamimusuhi-runtime -- chat --dir "$RUNTIME_DIR" \
  --subject "$SUBJECT" --persona openai-compatible \
  --persona-url "$KAMIMUSUHI_API_BASE_URL" --persona-model "$KAMIMUSUHI_API_MODEL" \
  --persona-auth-env "$API_KEY_ENV" --persona-locality external --privacy unconstrained "$@"
