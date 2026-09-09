#!/usr/bin/env bash
# The v0.1 continuity demo (issue #8), runnable from a clean checkout.
#
# Two *separate processes* against one runtime directory. That is the point:
# process B is handed a directory and nothing else — no transcript, no prompt
# buffer, no shared memory — and has to restore the same individual from
# canonical state alone, then think again with a different cognitive resource.
#
#   ./scripts/demo-v0.1.sh [runtime-dir]
#
# Default runtime directory: .local/demo. Delete it to start over.
set -euo pipefail

cd "$(dirname "$0")/.."

DIR="${1:-.local/demo}"
RUNTIME="cargo run --quiet --release -p kamimusuhi-runtime --"

echo "==> building"
cargo build --quiet --release -p kamimusuhi-runtime

if [ -e "$DIR/runtime.json" ]; then
  echo "==> $DIR is already initialized; remove it to start from scratch" >&2
  exit 1
fi

echo
echo "==> 1. init: create the runtime directory and one individual"
echo "    (an existing individual is adopted, never replaced; a runtime that"
echo "     cannot restore its individual fails instead of minting a new one)"
$RUNTIME init --dir "$DIR" --resource fake-a --seed 1

echo
echo "==> 2. process A: interaction, evidence, proposal, activation,"
echo "       Library import, Fake A invocation, typed workspace, exit"
$RUNTIME demo-continuity --dir "$DIR" --phase first --resource fake-a --seed 10

echo
echo "==> process A has exited. Nothing from its heap survives."
echo

echo "==> 3. process B: a fresh PID restores the same individual from disk,"
echo "       swaps the cognitive resource to Fake B, retrieves durable memory"
echo "       and the Library, and answers — with no previous chat buffer"
$RUNTIME demo-continuity --dir "$DIR" --phase resume --resource fake-b --seed 20

echo
echo "==> 4. inspect (read-only: claims no writer epoch, changes no row)"
$RUNTIME inspect --dir "$DIR"

echo
echo "==> operational trace (separate from the canonical audit in the DB):"
echo "    $DIR/trace.jsonl"
echo "    Follow the scenario by correlation ID, e.g.:"
echo "      jq -r '[.sequence, .event_kind, .correlation.boot_id] | @tsv' $DIR/trace.jsonl"
echo
echo "==> what this demonstrated"
echo "    - same IndividualId and continuity head across a real process boundary"
echo "    - durable relationship memory and Library material restored from disk"
echo "    - Fake A replaced by Fake B without touching identity or the head"
echo "    - self state, Library evidence and external resource output kept as"
echo "      separate typed workspace items and separate trace events"
