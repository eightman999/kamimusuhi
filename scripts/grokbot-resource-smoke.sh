#!/usr/bin/env bash
# One background turn against a borrowed 4B model on the Grok Bot VM.
#
# The endpoint is llama.cpp serving Qwen3-4B-Instruct with a 4096-token
# context, reached over Tailscale. It is registered here as a *cognitive
# resource*, never as a Persona Core: the individual, its memory and its
# lineage stay in this runtime directory, and the VM is only asked questions.
# Unplugging it is a config edit. See
# docs/experiments/grokbot-external-cognitive-resource.md.
#
#   ./scripts/grokbot-resource-smoke.sh [base-url] [model] [runtime-dir]
#
# e.g.
#   ./scripts/grokbot-resource-smoke.sh http://100.64.0.5:8080/v1 qwen3-4b-instruct
#
# The endpoint is an argument and is never compiled in. Ask the server what it
# calls its model before guessing:
#
#   curl http://<TAILSCALE_IP>:8080/v1/models
#
# If the endpoint needs a token, export it and pass its *variable name*:
#
#   export KAMIMUSUHI_GROKBOT_API_KEY=...
#   AUTH_ENV=KAMIMUSUHI_GROKBOT_API_KEY ./scripts/grokbot-resource-smoke.sh ...
#
# The name goes in runtime.json; the value is read at call time and never
# written to the config, the database or the trace.
set -euo pipefail

cd "$(dirname "$0")/.."

BASE_URL="${1:-http://127.0.0.1:8080/v1}"
MODEL="${2:-qwen3-4b-instruct}"
DIR="${3:-.local/grokbot-smoke}"
AUTH_ENV="${AUTH_ENV:-}"
RUNTIME="cargo run --quiet --release -p kamimusuhi-runtime --"

echo "==> building"
cargo build --quiet --release -p kamimusuhi-runtime

if [ ! -e "$DIR/runtime.json" ]; then
  echo "==> init + one fixture turn, so there is memory the VM never saw"
  $RUNTIME init --dir "$DIR" --resource fake-a --seed 1 > /dev/null
  $RUNTIME demo-continuity --dir "$DIR" --phase first --resource fake-a --seed 10 > /dev/null
fi

# The node ID is this host's, minted by init. Everything else in the file is a
# declaration about the endpoint, so the file is rewritten rather than patched.
NODE_ID="$(sed -n 's/.*"node_id": "\([^"]*\)".*/\1/p' "$DIR/runtime.json" | head -n 1)"
if [ -z "$NODE_ID" ]; then
  echo "could not read node_id from $DIR/runtime.json" >&2
  exit 1
fi

AUTH_LINE=""
if [ -n "$AUTH_ENV" ]; then
  AUTH_LINE="        \"auth_env\": \"$AUTH_ENV\","
fi

# Declared, not discovered. These are assertions an operator makes about an
# endpoint they configured — nothing here probes the VM, and the VM does not
# get to describe itself.
#
#   locality          external — Tailscale is a transport, not ownership
#   context_capacity  4096     — what llama.cpp was started with
#   latency           slow     — nothing has been measured yet
#   quality           basic    — a 4-bit 4B model
#   cost              free     — no metered API behind it
#
# The Grok Bot slot is the only resource configured, so a turn that reaches the
# VM did so because it qualified rather than because nothing else was left.
echo "==> pointing the runtime at $BASE_URL ($MODEL) as a cognitive resource"
cat > "$DIR/runtime.json" <<EOF
{
  "config_version": 1,
  "node_id": "$NODE_ID",
  "persona": {
    "backend": "fake",
    "seed": {
      "seed": "v0"
    }
  },
  "resources": {
    "grokbot-qwen3-4b": "openai-compatible"
  },
  "providers": {
    "grokbot-qwen3-4b": {
      "base_url": "$BASE_URL",
      "model": "$MODEL",
$AUTH_LINE
      "timeout_ms": 120000,
      "max_attempts": 1,
      "retry_backoff_ms": 0,
      "resource_id": "0000000000000000000000000006b04b",
      "capabilities": {
        "locality": "external",
        "modalities": ["text"],
        "context_capacity": 4096,
        "latency": "slow",
        "cost": "free",
        "quality": "basic",
        "health": "healthy"
      }
    }
  }
}
EOF

echo "==> a background turn delegating to the borrowed model"
echo "    --urgency background: a resource declared slow is deliberately"
echo "    unreachable from a turn a person is waiting on"
$RUNTIME demo-continuity \
  --dir "$DIR" \
  --phase resume \
  --urgency background \
  --id-seed "$RANDOM" \
  --clock system

echo
echo "==> the same turn, with the material marked as not allowed to leave"
echo "    this must refuse; it must not quietly use the VM anyway"
if $RUNTIME demo-continuity \
     --dir "$DIR" \
     --phase resume \
     --urgency background \
     --privacy no-external-service \
     --id-seed "$((RANDOM + 1))" \
     --clock system > /dev/null 2>&1; then
  echo "FAIL: a no-external-service turn reached an external resource" >&2
  exit 1
fi
echo "    refused, as it must be (PRIVACY_EXCLUDED in $DIR/trace.jsonl)"

echo
echo "==> what to look for in the JSON above"
echo "    resource.slot          grokbot-qwen3-4b — the borrowed cortex answered"
echo "    resource.adapter       openai-compatible — the existing adapter, no new client"
echo "    workspace item domain  EXTERNAL_RESOURCE_RESULT / external_material"
echo "    head_before == head_after   borrowing thought moves no canonical state"
echo "    relationship[].payload restored from the database, not from the VM"
echo
echo "    Identity, memory and lineage stayed in $DIR. Deleting the VM costs"
echo "    the 'providers' entry above and nothing else."
