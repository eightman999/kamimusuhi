# Native dialogue GUI

kamimusuhi-desktop is a native Rust/eframe surface for the conversation-side
K-CORE path. It is not a web frontend and does not start a local HTTP server.

    # Mock / local-only smoke
    ./scripts/run-desktop.sh

    # Jev -> all admitted organs -> Jev selection/gate. External transfer is explicit.
    /Users/eightman/dev/sandbox/kamimusuhi/scripts/run-desktop.sh \
      --privacy unconstrained \
      --dir /Users/eightman/dev/sandbox/kamimusuhi/.local/desktop

The first launch creates .local/desktop with a FakeA runtime if the directory
has not been initialized. When the operator exports the configured TYPESAFE_*,
KAMIMUSUHI_LLM_* variables, the worker uses the same
provider resolution as the CLI. If `HAI_API_KEY` is also present, the worker
automatically registers `hai-qwen3.8-27b-uncensored` and
`hai-llm-jp-4-vl-9b` at `https://hai-api.hcloud.ltd/v1`. Credential values are
never sent to the UI or saved in runtime configuration.

The current live configuration excludes LFM: primary is HAI `llm-jp-4-vl-9b`,
and the other organ is HAI `qwen3.8-27b-uncensored`. Use:

```bash
export KAMIMUSUHI_LLM_PROVIDER=hai
export KAMIMUSUHI_LLM_BASE_URL=https://hai-api.hcloud.ltd/v1
export KAMIMUSUHI_LLM_MODEL=llm-jp-4-vl-9b
```

HAI reads `HAI_API_KEY` even when replacing a Grokbot primary. The preset
registration skips the primary model and removes an unchanged duplicate
preset, so each HAI model generates once. Customized entries are preserved.

The window has a left navigation rail, central conversation, state/trace
inspector, and provider summary. The worker owns one Runtime and one
DialogueSession; the UI thread never touches SQLite or provider calls. Turns
are serialized and duplicate request IDs are refused. The trace view shows
normalized Jev decisions and public timing/transition data only, not model
reasoning or unselected response text.

The `Extra LLM API` panel registers additional OpenAI-compatible language
organs with an ID, base URL, model, and authentication environment-variable
name. Only the variable name is stored in `runtime.json`; the secret value is
read by the provider at call time. The turn workflow is:

```text
DialogueSession / conversation-side K-CORE state
  -> Jev preparation: invocation_gate + recall relevance + observation_need
  -> bounded local context selection / clarification guidance
  -> all enabled, privacy-admitted, auth-configured organs generate concurrently
  -> wait for every organ to finish or time out
  -> Jev assessment: response_candidate + per-candidate grounding / attribution / task_fit / gate / repair_reason
  -> optional reasoned regeneration of one organ, then another assessment
  -> accepted response / state update
```

The generation pool includes the primary organ and eligible registered organs;
organs requiring no authentication are eligible too. Jev selects after
generation, using response quality first and telemetry only as advisory
evidence. Empty responses and responses larger than 16 KiB (UTF-8 bytes) are
excluded. Valid responses are judged in full, without truncating their text.
The host validates every typed answer and binds the selected assessment to its
candidate ID, attempt, response digest, and evidence digest. Jev receives the
actual attributed conversation, memory, runtime and research context supplied
to the language organs. Candidate text remains untrusted data. Jev itself
remains exclusively on `/v1/systemone`.

Parallel generation waits for all completion/timeout results, so its wall time
is dominated by the slowest organ. `RETRY` regenerates only the selected organ,
once, with host-authored repair instructions and the previous response labelled
as untrusted JSON data. Jev then reselects and reassesses the updated pool against
the same factual evidence snapshot. Other organs are not regenerated, and the
final choice can change. Conflicting grounding, attribution, task-fit, or repair
answers prevent acceptance even if the raw gate says ACCEPT.
A configured Jev failure cannot silently select the primary organ or accept a
response. Missing Jev credentials use the separate rule-based path, which
remains compatible with local Mock operation and marks semantic axes as
NOT_EVALUATED. Normal turns use two logical Jev calls, or three after generation
retry; schema repair can add one HTTP attempt per batch.

Preparation can consult bounded, source-scoped recall candidates, reuse already
loaded research, refresh local runtime time/uptime once, or request one short
clarification. It does not poll MIO again or launch external research. See
[the runtime contract](kcore_llm_jev.md) for recall and payload limits. Byte limits
do not guarantee that the provider's token window will fit.

The registered entries are stored under `language_providers` in the runtime
configuration. The current adapter accepts OpenAI-compatible chat-completions
wire format; provider-specific Anthropic/Gemini adapters are not implied by
this panel.

Each language-organ attempt records calls, successes, failures, success rate,
last latency, EWMA latency, and a stable last-error code. The current
secret-free snapshot is included in Jev's `response_candidate` choice state
and criteria, and is shown in the trace/provider panel. These are per-call
timings and session-only statistics, not evidence of live throughput,
tokens/second, or sustained service performance.

The trace lists every generated attempt, including the primary organ, errors,
and retries: ID, provider/model, `attempt` (0 initial, 1 retry), latency,
response bytes, and error code. A selected mark requires a successful trace
whose final candidate ID and digest match that attempt. If a retry returns the
same text, only the latest matching attempt is marked. The existing
`provider_selection` trace field now represents the generated
`response_candidate` choice. Preparation and assessment timings are measured per
batch. The legacy selection and gate fields share the same assessment latency
and must not be added together. The `assessments` list retains both attempts
when a regeneration occurs. Each batch timing includes schema repair, if any.
The trace view also shows the final quality axes, repair reason, and raw gate.
Confidence is a model distribution statistic, not a calibrated correctness rate.
Evidence and rejected response text are absent from these metadata records.
`generation_latency_ms` records initial parallel wall
time, not the sum of organ timings and not retry time. Retry generation timing
appears on the separate attempt row.

On Send, the UI clears the previous successful trace, generation report, and
turn latency. After `DialogueSession::turn` returns, including on failure, the
worker sends `GenerationReport` before the reply or error using
`last_generated_candidates()` and `last_generation_latency_ms()`. The report's
`elapsed_ms` is the initial parallel generation wall time, not total turn time.
It stays visible even when selection, gating, or a later turn step fails, with
no selected mark in the absence of a successful trace. An empty report denotes
no generation attempts, including a turn stopped before generation. This is a
completed-turn report; there is no live-progress callback in this pass.

The image supplied with the request is used only as a visual reference for
the dark layout, blue accents, dense panels, and fixed input area. The
following are intentionally not fabricated: GPU utilization, price,
tokens/second, arbitrary provider-specific wire adapters, audio, or token streaming.

This GUI does not call KCore::tick(), J72, or any learning path.

For an explicitly requested live check, the following opt-in smoke sends one
short greeting to the configured primary organ, distinct HAI presets and Jev. Export
the same credential and provider environment variables first. It creates a
temporary runtime, leaves the user's dialogue history untouched, and prints
only timing/decision metadata. Omitting `--live` sends no requests.

```bash
cargo run --manifest-path /Users/eightman/dev/sandbox/kamimusuhi/Cargo.toml \
  -p kamimusuhi-runtime --example dialogue_fanout_smoke -- --live
```
