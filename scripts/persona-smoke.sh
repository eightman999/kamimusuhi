#!/usr/bin/env bash
# One turn against a real, model-backed Persona Core.
#
# The Persona Core is what speaks as the individual. A cognitive resource is
# something a turn delegates a subtask to. This script exercises the first:
# the endpoint you point it at produces the user-facing expression, and the
# runtime hands it the turn's memory, Library material and any delegated
# result as labelled sections rather than as an unmarked prompt.
#
#   ./scripts/persona-smoke.sh [base-url] [model] [runtime-dir]
#
# Defaults to a local Ollama-style endpoint. Anything OpenAI-compatible works:
#
#   llama.cpp   ./llama-server -m model.gguf --port 8080
#               ./scripts/persona-smoke.sh http://127.0.0.1:8080/v1 any-name
#   Ollama      ollama serve
#               ./scripts/persona-smoke.sh http://127.0.0.1:11434/v1 llama3.2
#   LM Studio   start the local server in the app
#               ./scripts/persona-smoke.sh http://127.0.0.1:1234/v1 your-model
#
# For an endpoint that needs a token, put the *name* of the environment
# variable holding it in runtime.json under persona.provider.auth_env. The
# token itself is read at call time and is never written to the config, the
# database or the trace.
set -euo pipefail

cd "$(dirname "$0")/.."

BASE_URL="${1:-http://127.0.0.1:11434/v1}"
MODEL="${2:-llama3.2}"
DIR="${3:-.local/persona-smoke}"
RUNTIME="cargo run --quiet --release -p kamimusuhi-runtime --"

echo "==> building"
cargo build --quiet --release -p kamimusuhi-runtime

if [ ! -e "$DIR/runtime.json" ]; then
  echo "==> init + one fixture turn, so there is memory to speak from"
  $RUNTIME init --dir "$DIR" --resource fake-a --seed 1 > /dev/null
  $RUNTIME demo-continuity --dir "$DIR" --phase first --resource fake-a --seed 10 > /dev/null
fi

echo "==> one turn with a model-backed Persona Core at $BASE_URL ($MODEL)"
echo "    a fresh process: no prior chat buffer, memory read from disk"
$RUNTIME demo-continuity \
  --dir "$DIR" \
  --phase resume \
  --id-seed "$RANDOM" \
  --clock system \
  --persona openai-compatible \
  --persona-url "$BASE_URL" \
  --persona-model "$MODEL"

echo
echo "==> what to look for in the JSON above"
echo "    persona_backend.kind    openai-compatible — the model produced the expression"
echo "    persona_response        generated text, not the resource's output"
echo "    head_before == head_after   a Persona turn moves no canonical state"
echo "    relationship[].payload  restored from the database, not from a prompt"
echo
echo "    The trace at $DIR/trace.jsonl records persona.invoked, persona.completed"
echo "    and persona.final_expression by ID and digest — never the prompt or the"
echo "    reply text."
