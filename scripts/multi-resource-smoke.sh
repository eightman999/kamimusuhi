#!/usr/bin/env bash
# Two borrowed brains, one router.
#
# Registers both cognitive resources at once and shows the router choosing
# between them.
#
# As deployed today both run on the same VM, which the operator does not
# control, so both are external:
#
#   j72       cursor VM, custom PyTorch, CPU   external
#   grokbot   cursor VM, llama.cpp             external
#
# J72 was originally on `llm-machine`, hardware the operator controls, which
# made it local_network and gave a no-external-service turn exactly one place
# to go. It moved, so the declaration moved with it. A boundary that does not
# follow the machine is worse than no boundary: it reads as enforced and is
# not. Set J72_LOCALITY=local_network again when it is back on owned hardware.
#
# Neither is a Persona Core, and neither is asked which of them should answer.
# The runtime states what the task needs; the router decides.
#
#   ./scripts/multi-resource-smoke.sh [j72-url] [grokbot-url] [runtime-dir]
#
# e.g.
#   ./scripts/multi-resource-smoke.sh \
#     http://cursor:8081/v1 http://cursor:8080/v1
#
# Both endpoints take a bearer token here; export it and name the variable:
#
#   export KAMIMUSUHI_GROKBOT_API_KEY=...
#   AUTH_ENV=KAMIMUSUHI_GROKBOT_API_KEY ./scripts/multi-resource-smoke.sh ...
#
# The name goes in runtime.json; the value is read at call time and never
# written to the config, the database or the trace.
#
# Both latencies come from measurement, not from model size. Override them when
# a fresh measurement says something different:
#
#   J72_LATENCY=fast GROKBOT_LATENCY=slow ./scripts/multi-resource-smoke.sh ...
#
# J72 is declared last_resort because measurement says so, not because it is
# small: 0/36 on every task in the comparison harness, and roughly 8x the
# median latency of the Qwen on the same bounded workload. Precedence is the
# axis for "reach for this last"; lying on health or cost to get the same
# ordering would corrupt every other reading of those fields.
set -euo pipefail

cd "$(dirname "$0")/.."

J72_URL="${1:-http://cursor:8081/v1}"
GROKBOT_URL="${2:-http://cursor:8080/v1}"
DIR="${3:-.local/multi-resource-smoke}"
J72_MODEL="${J72_MODEL:-j72-30m}"
GROKBOT_MODEL="${GROKBOT_MODEL:-qwen2.5-3b-instruct}"
GROKBOT_LATENCY="${GROKBOT_LATENCY:-slow}"
GROKBOT_PRECEDENCE="${GROKBOT_PRECEDENCE:-ordinary}"
J72_LATENCY="${J72_LATENCY:-slow}"
J72_PRECEDENCE="${J72_PRECEDENCE:-last_resort}"
J72_LOCALITY="${J72_LOCALITY:-external}"
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

# Declared, not discovered — for both. Locality follows whose machine it is,
# not how the packets get there; both are reached over Tailscale, and that is
# not what makes either of them anything.
#
# j72 context_capacity 4096 comes from the checkpoint config the server loads
# (novllm phase55_probe.json, primary: hidden 768 / 12 layers / 4096), which
# /health corroborates by reporting the matching parameter count.
echo "==> registering both resources"
echo "    j72      $J72_URL ($J72_MODEL)   $J72_LOCALITY / $J72_LATENCY / $J72_PRECEDENCE"
echo "    grokbot  $GROKBOT_URL ($GROKBOT_MODEL)   external / $GROKBOT_LATENCY / $GROKBOT_PRECEDENCE"
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
        "locality": "$J72_LOCALITY",
        "modalities": ["text"],
        "context_capacity": 4096,
        "latency": "$J72_LATENCY",
        "cost": "free",
        "quality": "basic",
        "health": "healthy",
        "precedence": "$J72_PRECEDENCE"
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
        "latency": "$GROKBOT_LATENCY",
        "cost": "free",
        "quality": "basic",
        "health": "healthy",
        "precedence": "$GROKBOT_PRECEDENCE"
      }
    }
  }
}
EOF

echo
echo "==> 1. privacy = no-external-service"
if [ "$J72_LOCALITY" = "external" ]; then
  echo "    both models are on a machine the operator does not control, so both"
  echo "    are excluded and this turn must fail. That is a real loss of"
  echo "    capability, reported rather than papered over."
  if $RUNTIME demo-continuity \
       --dir "$DIR" \
       --phase resume \
       --privacy no-external-service \
       --urgency background \
       --id-seed "$RANDOM" \
       --clock system > /dev/null 2>&1; then
    echo "FAIL: a no-external-service turn reached an external resource" >&2
    exit 1
  fi
  echo "    refused, as it must be."
else
  echo "    the material may use the operator's own infrastructure, no third party."
  echo "    exactly one machine qualifies."
  $RUNTIME demo-continuity \
    --dir "$DIR" \
    --phase resume \
    --privacy no-external-service \
    --urgency background \
    --id-seed "$RANDOM" \
    --clock system
fi

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
echo "    both eligible. J72 declared itself the last resort on measured"
echo "    evidence, so the Qwen takes it."
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
echo "    one individual, two interchangeable models, and a router that decided"
echo "    between them from declared capabilities alone — not from a model's"
echo "    opinion and not from a benchmark."
if [ "$J72_LOCALITY" = "external" ]; then
  echo
  echo "    what it does NOT show, while both models live on the same borrowed"
  echo "    VM: a turn choosing its machine by data boundary. That needs J72"
  echo "    back on owned hardware and J72_LOCALITY=local_network."
fi
echo
echo "    head_before == head_after in every case above: borrowing thought from"
echo "    either machine moves no canonical state, and neither model's output"
echo "    is anything but EXTERNAL_RESOURCE_RESULT / external_material."
