#!/usr/bin/env bash
# Local CI gate. This script is the source of truth for "green"; hosted CI is
# optional and must run exactly these steps.
set -euo pipefail

cd "$(dirname "$0")/.."

echo "==> cargo fmt --check"
cargo fmt --all -- --check

echo "==> cargo clippy"
cargo clippy --workspace --all-targets --all-features -- -D warnings

echo "==> cargo test"
cargo test --workspace

echo "==> core dependency boundary"
# kamimusuhi-core must not pull in storage clients, HTTP clients, provider SDKs,
# accelerator backends, vector databases or speech libraries (plan §3.1).
forbidden='^(rusqlite|libsqlite3-sys|sqlx|reqwest|hyper|ureq|tokio|async-openai|openai|anthropic|cudarc|metal|qdrant-client|lancedb|whisper-rs|tts)( |$)'
core_tree="$(cargo tree -p kamimusuhi-core -e normal --prefix none | awk '{print $1}' | sort -u)"
if hits="$(printf '%s\n' "$core_tree" | grep -E "$forbidden")"; then
  echo "kamimusuhi-core depends on forbidden crates:" >&2
  echo "$hits" >&2
  exit 1
fi
echo "kamimusuhi-core dependency tree is clean"

echo "==> local CI passed"
