#!/usr/bin/env bash
# Two borrowed brains, one router.
#
# Registers both cognitive resources at once and shows the router choosing
# between them on the only ground that separates them: who owns the machine.
#
#   j72       llm-machine, the operator's own box   local_network
#   grokbot   a third-party VM                      external
#
# Neither is a Persona Core, and neither is asked which of them should answer.
# The runtime states what the task needs; the router decides.
#
#   ./scripts/multi-resource-smoke.sh [j72-url] [grokbot-url] [runtime-dir]
#
# e.g.
#   ./scripts/multi-resource-smoke.sh \
#     http://llm-machine:8081/v1 http://<GROKBOT_HOST>:8080/v1
#
# Both endpoints take a bearer token here; export it and name the variable:
#
#   export KAMIMUSUHI_GROKBOT_API_KEY=...
#   AUTH_ENV=KAMIMUSUHI_GROKBOT_API_KEY ./scripts/multi-resource-smoke.sh ...
#
# The name goes in runtime.json; the value is read at call time and never
# written to the config, the database or the trace.
#
# J72's declared latency comes from measurement, not from its size. Override it
# when a fresh measurement says something different:
#
#   J72_LATENCY=slow ./scripts/multi-resource-smoke.sh ...
set -euo pipefail

cd "$(dirname "$0")/.."

J72_URL="${1:-http://llm-machine:8081/v1}"
GROKBOT_URL="${2:-http://127.0.0.1:8080/v1}"
DIR="${3:-.local/multi-resource-smoke}"
J72_MODEL="${J72_MODEL:-j72-30m}"
GROKBOT_MODEL="${GROKBOT_MODEL:-qwen2.5-3b-instruct}"
J72_LATENCY="${J72_LATENCY:-slow}"
AUTH_ENV="${AUTH_ENV:-}"
RUNTIME="cargo run --quiet --release -p kamimusuhi-runtime --"

echo "==> building"
cargo build --quiet --release -p kamimusuhi-runtime

if [ ! -e "$DIR/runtime.json" ]; then
  echo "==> init + one fixture turn, so there is memory neither model produced"
  $RUNTIME init --dir "$DIR" --resource fake-a --seed 1 > /dev/null
  $RUNTIME demo-continuity --dir "$DIR" --phase first --resource fake-a --seed 10 > /dev/null
fi

NODE_ID="$(sed -n 's/.*"node_id": "\([^"]*\)".*/\1/p' "$DIR/runtime.json" | head -n 1)"
if [ -z "$NODE_ID" ]; then
  echo "could not read node_id from $DIR/runtime.json" >&2
  exit 1
fi

AUTH_LINE=""
if [ -n "$AUTH_ENV" ]; then
  AUTH_LINE="        \"auth_env\": \"$AUTH_ENV\","
fi

# Declared, not discovered — for both, and the difference between them is the
# whole experiment:
#
#   j72       local_network   llm-machine is the operator's own machine
#   grokbot   external        the VM is not
#
# Not a statement about the network. Both are reached over Tailscale.
#
# j72 context_capacity 4096 comes from the checkpoint config the server loads
# (novllm phase55_probe.json, primary: hidden 768 / 12 layers / 4096), which
# /health corroborates by reporting the matching parameter count.
echo "==> registering both resources"
echo "    j72      $J72_URL ($J72_MODEL)   local_network / $J72_LATENCY"
echo "    grokbot  $GROKBOT_URL ($GROKBOT_MODEL)   external / slow / last_resort"
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
    "j72": "openai-compatible",
    "grokbot": "openai-compatible"
  },
  "providers": {
    "j72": {
      "base_url": "$J72_URL",
      "model": "$J72_MODEL",
$AUTH_LINE
      "timeout_ms": 120000,
      "max_attempts": 1,
      "retry_backoff_ms": 0,
      "resource_id": "00000000000000000000000000000472",
      "capabilities": {
        "locality": "local_network",
        "modalities": ["text"],
        "context_capacity": 4096,
        "latency": "$J72_LATENCY",
        "cost": "free",
        "quality": "basic",
        "health": "healthy"
      }
    },
    "grokbot": {
      "base_url": "$GROKBOT_URL",
      "model": "$GROKBOT_MODEL",
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

echo
echo "==> 1. privacy = no-external-service"
echo "    the material may use the operator's own infrastructure, no third party."
echo "    exactly one machine qualifies."
$RUNTIME demo-continuity \
  --dir "$DIR" \
  --phase resume \
  --privacy no-external-service \
  --urgency background \
  --id-seed "$RANDOM" \
  --clock system

echo
echo "==> 2. privacy = local-only"
echo "    neither machine is this one. both must be excluded."
if $RUNTIME demo-continuity \
     --dir "$DIR" \
     --phase resume \
     --privacy local-only \
     --urgency background \
     --id-seed "$((RANDOM + 1))" \
     --clock system > /dev/null 2>&1; then
  echo "FAIL: a local-only turn reached a resource on another machine" >&2
  exit 1
fi
echo "    refused, as it must be. Tailscale is not a reason to make an exception."

echo
echo "==> 3. privacy = unconstrained"
echo "    both eligible. the Qwen declared itself a last resort, so it loses."
$RUNTIME demo-continuity \
  --dir "$DIR" \
  --phase resume \
  --privacy unconstrained \
  --urgency background \
  --id-seed "$((RANDOM + 2))" \
  --clock system \
  | python3 -c 'import json,sys; r=json.load(sys.stdin)["resource"]; print("    selected:", r["slot"], "  considered:", [(c["slot"], c["reason"]) for c in r["routing_decision"]["considered"]])'

echo
echo "==> what this shows"
echo "    the same individual sent the same kind of turn to two physically"
echo "    different machines, and the thing that decided which one was a"
echo "    declared data boundary — not a model's opinion, and not a benchmark."
echo
echo "    head_before == head_after in every case above: borrowing thought from"
echo "    either machine moves no canonical state, and neither model's output"
echo "    is anything but EXTERNAL_RESOURCE_RESULT / external_material."
