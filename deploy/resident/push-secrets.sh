#!/usr/bin/env bash
# Write /srv/kamimusuhi/config/secrets.env on a node from the operator's Mac.
# Values travel over ssh stdin only; nothing is printed, logged or committed.
#
#   deploy/resident/push-secrets.sh <ssh-host> [hai-key-file] [ssh options...]
#   GITHUB_TOKEN_FILE=<file> also pushes GITHUB_PERSONAL_ACCESS_TOKEN.
# Keys not managed here (e.g. NETDATA_MCP_KEY) are preserved.
#
# The node token is generated once into ~/.config/kamimusuhi/node_token and
# the same value is pushed to every node.
set -euo pipefail
HOST="${1:?usage: $0 <ssh-host> [hai-key-file]}"
HAI_KEY_FILE="${2:-$HOME/dev/OISINT-wt/kamimusuhi_key}"
TOKEN_FILE="$HOME/.config/kamimusuhi/node_token"
shift || true; shift || true
SSH_OPTS=("$@")

[[ -r "$HAI_KEY_FILE" ]] || { echo "HAI key file not readable: $HAI_KEY_FILE" >&2; exit 1; }
if [[ ! -s "$TOKEN_FILE" ]]; then
  mkdir -p "$(dirname "$TOKEN_FILE")"; chmod 700 "$(dirname "$TOKEN_FILE")"
  (umask 077; openssl rand -hex 32 > "$TOKEN_FILE")
fi
{
  printf 'KAMIMUSUHI_NODE_TOKEN=%s\n' "$(tr -d '[:space:]' < "$TOKEN_FILE")"
  printf 'HAI_API_KEY=%s\n' "$(tr -d '[:space:]' < "$HAI_KEY_FILE")"
  if [[ -n "${GITHUB_TOKEN_FILE:-}" ]]; then
    printf 'GITHUB_PERSONAL_ACCESS_TOKEN=%s\n' "$(tr -d '[:space:]' < "$GITHUB_TOKEN_FILE")"
  fi
} | ssh "${SSH_OPTS[@]}" "$HOST" '
  set -e; umask 077
  f=/srv/kamimusuhi/config/secrets.env
  new=$(cat)
  # Keep keys this script does not manage (e.g. NETDATA_MCP_KEY written on the node).
  keys=$(printf "%s\n" "$new" | cut -d= -f1 | paste -sd"|" -)
  { [ -f "$f" ] && grep -vE "^($keys)=" "$f" || true; printf "%s\n" "$new"; } > "$f.new"
  mv -f "$f.new" "$f" && chmod 600 "$f" && echo "secrets.env updated on $(hostname): $(cut -d= -f1 "$f" | paste -sd" " -)"'
