#!/usr/bin/env bash
# Start the native Kamimusuhi dialogue surface.
set -euo pipefail

cd "$(dirname "$0")/.."
cargo run -p kamimusuhi-desktop -- "$@"
