#!/bin/zsh
# Double-click to talk with the always-on Kamimusuhi individual (澪 TUI).
# Prefers the installed `kami` binary; falls back to building from source.
set -eu
ROOT="${0:A:h}"
cd "$ROOT"
if command -v kami >/dev/null 2>&1; then
    exec kami "$@"
fi
exec cargo run --quiet --release -p kamimusuhi-tui -- "$@"
