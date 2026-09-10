#!/usr/bin/env bash
# One background turn against a borrowed small model on the Grok Bot VM.
#
# The endpoint is llama.cpp serving a 4-bit ~3B instruct model with a
# 4096-token context, reached over Tailscale. It is registered here as a *cognitive
# resource*, never as a Persona Core: the individual, its memory and its
# lineage stay in this runtime directory, and the VM is only asked questions.
# Unplugging it is a config edit. See
# docs/experiments/grokbot-external-cognitive-resource.md.
#
#   ./scripts/grokbot-resource-smoke.sh [base-url] [model] [runtime-dir]
#
# e.g.
#   ./scripts/grokbot-resource-smoke.sh http://100.64.0.5:8080/v1 qwen2.5-3b-instruct
#
# The endpoint is an argument and is never compiled in. Ask the server what it
# calls its model before guessing:
#
#   curl http://<TAILSCALE_IP>:8080/v1/models
#
# To watch it lose to a better model, register a second LLM alongside it:
#
#   PEER_URL=http://<OTHER_HOST>:8080/v1 PEER_MODEL=qwen3-4b \
#     ./scripts/grokbot-resource-smoke.sh http://<TAILSCALE_IP>:8080/v1 ...
#
# The peer says nothing about its own standing, which is enough: anything that
# does not declare itself a last resort outranks one that does.
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
MODEL="${2:-qwen2.5-3b-instruct}"
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

# An optional second LLM. It declares no precedence at all, which resolves to
# `ordinary` — and that is already enough to outrank the borrowed machine.
PEER_URL="${PEER_URL:-}"
PEER_MODEL="${PEER_MODEL:-}"
PEER_AUTH_ENV="${PEER_AUTH_ENV:-}"
PEER_RESOURCE_LINE=""
PEER_PROVIDER_BLOCK=""
if [ -n "$PEER_URL" ] && [ -n "$PEER_MODEL" ]; then
  PEER_AUTH_LINE=""
  if [ -n "$PEER_AUTH_ENV" ]; then
    PEER_AUTH_LINE="        \"auth_env\": \"$PEER_AUTH_ENV\","
  fi
  PEER_RESOURCE_LINE="    \"peer-llm\": \"openai-compatible\","$'\n'
  PEER_PROVIDER_BLOCK="    \"peer-llm\": {
      \"base_url\": \"$PEER_URL\",
      \"model\": \"$PEER_MODEL\",
$PEER_AUTH_LINE
      \"timeout_ms\": 120000,
      \"max_attempts\": 1,
      \"retry_backoff_ms\": 0,
      \"resource_id\": \"00000000000000000000000000000f00\",
      \"capabilities\": {
        \"locality\": \"external\",
        \"modalities\": [\"text\"],
        \"context_capacity\": 4096,
        \"latency\": \"slow\",
        \"cost\": \"low\",
        \"quality\": \"basic\",
        \"health\": \"healthy\"
      }
    },"$'\n'
fi

# Declared, not discovered. These are assertions an operator makes about an
# endpoint they configured — nothing here probes the VM, and the VM does not
# get to describe itself.
#
#   locality          external    — Tailscale is a transport, not ownership
#   context_capacity  4096        — what llama.cpp was started with
#   latency           slow        — nothing has been measured yet
#   quality           basic       — a 4-bit 3B model
#   cost              free        — no metered API behind it
#   precedence        last_resort — reach for this after every other model
#
# precedence is what keeps `cost: free` from backfiring. Without it the
# cheapest endpoint wins every tie, and a machine that belongs to someone else
# quietly becomes the thing everything runs on.
echo "==> pointing the runtime at $BASE_URL ($MODEL) as the last-resort resource"
if [ -n "$PEER_PROVIDER_BLOCK" ]; then
  echo "    and at $PEER_URL ($PEER_MODEL) as an ordinary peer, which outranks it"
fi
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
$PEER_RESOURCE_LINE    "grokbot": "openai-compatible"
  },
  "providers": {
$PEER_PROVIDER_BLOCK    "grokbot": {
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
        "health": "healthy",
        "precedence": "last_resort"
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
if [ -n "$PEER_PROVIDER_BLOCK" ]; then
  echo "    resource.slot          peer-llm — the better-standing model answered"
  echo "    routing_decision.considered  grokbot is NOT_PREFERRED, not excluded:"
  echo "                                 still usable, just last in line"
else
  echo "    resource.slot          grokbot — the borrowed cortex answered"
fi
echo "    resource.adapter       openai-compatible — the existing adapter, no new client"
echo "    workspace item domain  EXTERNAL_RESOURCE_RESULT / external_material"
echo "    head_before == head_after   borrowing thought moves no canonical state"
echo "    relationship[].payload restored from the database, not from the VM"
echo
echo "    Identity, memory and lineage stayed in $DIR. Deleting the VM costs"
echo "    the 'providers' entry above and nothing else."
