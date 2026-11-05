#!/usr/bin/env bash
# Thin wrapper for calling the resident HTTP API directly.
#
#   ./scripts/kami-api.sh status                  # GET /status
#   ./scripts/kami-api.sh models                  # GET /v1/models
#   ./scripts/kami-api.sh ask "テキスト" [model]   # POST /v1/chat/completions
#   ./scripts/kami-api.sh talk "テキスト" [subject] # POST /v1/kamimusuhi/talk（個体）
#   ./scripts/kami-api.sh history [subject] [n]   # POST /v1/kamimusuhi/history
#   ./scripts/kami-api.sh call <method> <path> [json-body]
#                                                 # 任意エンドポイント
#
# 接続先: --url > $KAMIMUSUHI_URL > ~/.config/kamimusuhi/nodes の先頭行
# トークン: $KAMIMUSUHI_NODE_TOKEN > ~/.config/kamimusuhi/node_token
# loopback 宛てならトークン不要。
set -euo pipefail

URL="${KAMIMUSUHI_URL:-}"
if [[ "${1:-}" == "--url" ]]; then URL="$2"; shift 2; fi
[[ -n "$URL" ]] || URL="$(head -n1 "$HOME/.config/kamimusuhi/nodes" 2>/dev/null || true)"
[[ -n "$URL" ]] || { echo "接続先がありません: --url / KAMIMUSUHI_URL / ~/.config/kamimusuhi/nodes" >&2; exit 64; }
URL="${URL%/}"

TOKEN="${KAMIMUSUHI_NODE_TOKEN:-$(cat "$HOME/.config/kamimusuhi/node_token" 2>/dev/null || true)}"
AUTH=(); [[ -n "$TOKEN" ]] && AUTH=(-H "Authorization: Bearer $TOKEN")

pretty() { if command -v jq >/dev/null 2>&1; then jq .; else cat; fi; }

cmd="${1:-}"; shift || true
case "$cmd" in
  status)  curl -fsS "$URL/status" "${AUTH[@]}" | pretty ;;
  health)  curl -fsS "$URL/health" | pretty ;;
  models)  curl -fsS "$URL/v1/models" "${AUTH[@]}" | pretty ;;
  ask)
    text="${1:?usage: ask <text> [model]}"; model="${2:-kamimusuhi}"
    curl -fsS "$URL/v1/chat/completions" "${AUTH[@]}" -H 'Content-Type: application/json' \
      -d "$(jq -nc --arg m "$model" --arg t "$text" '{model:$m,messages:[{role:"user",content:$t}]}' 2>/dev/null \
           || python3 -c 'import json,sys;print(json.dumps({"model":sys.argv[1],"messages":[{"role":"user","content":sys.argv[2]}]}))' "$model" "$text")" | pretty ;;
  talk)
    text="${1:?usage: talk <text> [subject]}"; subject="${2:-${USER:-local-user}}"
    curl -fsS "$URL/v1/kamimusuhi/talk" "${AUTH[@]}" -H 'Content-Type: application/json' \
      -d "$(jq -nc --arg s "$subject" --arg t "$text" '{subject:$s,message:$t}' 2>/dev/null \
           || python3 -c 'import json,sys;print(json.dumps({"subject":sys.argv[1],"message":sys.argv[2]}))' "$subject" "$text")" | pretty ;;
  history)
    subject="${1:-${USER:-local-user}}"; limit="${2:-20}"
    curl -fsS "$URL/v1/kamimusuhi/history" "${AUTH[@]}" -H 'Content-Type: application/json' \
      -d "$(jq -nc --arg s "$subject" --argjson l "$limit" '{subject:$s,limit:$l}' 2>/dev/null \
           || python3 -c 'import json,sys;print(json.dumps({"subject":sys.argv[1],"limit":int(sys.argv[2])}))' "$subject" "$limit")" | pretty ;;
  call)
    method="${1:?usage: call <GET|POST> <path> [json-body]}"; path="${2:?usage: call <GET|POST> <path> [json-body]}"
    body="${3:-}"
    if [[ -n "$body" ]]; then
      curl -fsS -X "$method" "$URL$path" "${AUTH[@]}" -H 'Content-Type: application/json' -d "$body" | pretty
    else
      curl -fsS -X "$method" "$URL$path" "${AUTH[@]}" | pretty
    fi ;;
  *) sed -n '2,14p' "$0" >&2; exit 64 ;;
esac
