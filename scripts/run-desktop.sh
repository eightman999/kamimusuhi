#!/usr/bin/env bash
# Start the native Kamimusuhi dialogue surface (resident mode by default;
# pass --local for the Jev test surface against a local runtime).
set -euo pipefail

cd "$(dirname "$0")/.."
cargo run -p kamimusuhi-desktop -- "$@"
