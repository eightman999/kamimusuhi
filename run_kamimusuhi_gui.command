#!/bin/zsh
# Double-click to talk with the always-on Kamimusuhi individual (Pi resident).
# Token: ~/.config/kamimusuhi/node_token (created by deploy/resident/push-secrets.sh).
set -eu
ROOT="${0:A:h}"
cd "$ROOT"
exec cargo run --quiet --release -p kamimusuhi-desktop -- "$@"
