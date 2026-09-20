# K-CORE v0 — model-independent runtime loop

K-CORE owns a **process lifetime**, not a second identity database. It resumes
Kamimusuhi's existing `Runtime` / Continuity Kernel, queues numeric observations,
and runs explicitly attached cognitive organs. J72, Jev, FBA, MIOBA, model
weights, Python and a GPU are **not required to boot or idle**. Python is needed
only for Python process adapters and the integration test fixture.

## Quick start

From the repository root, with the pinned Rust toolchain:

```sh
# Only for a NEW runtime directory. Never reinitialize an existing individual.
cargo run -p kamimusuhi-runtime --bin kamimusuhi-runtime -- \
  init --dir /tmp/kamimusuhi-core-demo --resource fake-unavailable

# Finite, model-free heartbeat; repeated runs restore the same individual.
cargo run -p kamimusuhi-runtime --bin k-core -- \
  --dir /tmp/kamimusuhi-core-demo --ticks 10 --interval-ms 100

# Continuous idle loop. Ctrl-C / process termination releases the OS loop lock.
cargo run -p kamimusuhi-runtime --bin k-core -- \
  --dir /tmp/kamimusuhi-core-demo

# One JSON document per line; EOF drains the bounded queue and stops.
printf '%s\n' \
  '{"source":"sensor","sequence":1,"observation":[0.2,-0.1],"quality_milli":1000}' \
  | cargo run -p kamimusuhi-runtime --bin k-core -- \
      --dir /tmp/kamimusuhi-core-demo --stdin
```

`--ticks 0` means continuous. Missing or corrupt canonical state fails closed;
the loop never silently initializes an individual. Existing `talk`, `chat`,
`demo-continuity` and default `cargo run -p kamimusuhi-runtime` are unchanged.

## Boundary and lifecycle

- Canonical identity, head, memory and Library remain in `kamimusuhi.sqlite`.
  K-CORE calls no canonical writer, does not claim a writer epoch, and issues
  no mutations. Genuine durable conclusions still require evidence → proposal
  → MutationPolicy → Continuity Kernel through existing authorized paths.
- The FIFO queue, per-source sequence high-water marks, ticks and organ hidden
  states are **ephemeral**. They intentionally reset at restart. No synthetic
  drive or goal is silently saved as personality or autobiographical memory.
- `kcore.loop.lock` uses an OS file lock, released on drop, kill or exit.
  It prevents two K-CORE loops in one local runtime directory. It is not a
  distributed ownership lease and does not exclude the existing dialogue
  commands. Do not unlink/replace the lock file while a loop is alive.
- Tick results are JSONL on stdout, not canonical evidence. They can contain
  organ payloads; treat captured output as sensitive local diagnostics. The
  existing bounded runtime trace continues to record boot/stop metadata.

## Numeric ingress and timing

`PerceptFrame` accepts only `source`, `sequence`, `observation`,
`quality_milli`, and optional `age_ms`. Unknown fields (including external
`evidence_refs` or authority claims) are rejected. Observation vectors must
contain 1–256 finite numbers; quality is 1–1000, and source names are bounded
ASCII routing keys. Quality is input validity metadata, **not calibrated
confidence**. Source names are not authentication.

Defaults: 100 ms interval, 64 queued frames, 64 source keys, 5000 ms maximum
observation age, no organs. Configurable limits are validated before startup.
The numeric JSONL frame limit is 16 KiB including its newline; configuration
files are limited to 64 KiB. The stdin reader has a separate bounded channel
of the same queue capacity (plus one currently read frame). Backpressure does
not turn into an unbounded list or consume a rejected sequence number.

Sequence numbers must increase per source during one process lifetime.
`age_ms` is producer-declared elapsed age, not a synchronized remote timestamp.
Channel wait, local queue wait and execution time are added/measured using
monotonic time. Expired observations cannot produce a live intent. At most one
fresh frame is processed per tick; expired queued frames are discarded. Idle
ticks never invoke a backend or reuse a previously active signal. Slow cycles
do not cause catch-up bursts. The OS scheduler/stdout can delay ticks: this is
not a hard real-time controller.

## Attaching experimental organs

Use `--config <path>` to explicitly load operator configuration. Omission means
**no organs**, even when runtime.json configures dialogue or MIO services.
A minimal config is `{}`. Optional process bindings use the existing
`ProcessOrgan` request/reply protocol, e.g.:

```json
{
  "interval_ms": 100,
  "queue_capacity": 64,
  "max_sources": 64,
  "max_age_ms": 5000,
  "organs": [
    {
      "key": "h0-regulation",
      "program": "/absolute/path/to/python3",
      "args": ["/absolute/path/to/your_checkpoint_wrapper.py"],
      "promotion": "shadow",
      "timeout_ms": 1000
    }
  ]
}
```

The wrapper above is an **operator-supplied example path**, not a bundled H0
checkpoint or an assertion of runtime validation. It must translate the
numeric `PerceptFrame` into that checkpoint's actual observation space.
Each process receives `{"input": OrganInput, "state": previous_state_or_null}`
and returns `{"payload": ..., "ttl_ms": 1000, "state": ...}`. The host supplies
attribution; the child cannot manufacture canonical evidence references.

Only H0/R0/S0/T0/O0 entries in `validated_experiment_manifest` may be configured.
Bindings default to **shadow**; active is an explicit operator choice and is
allowed only for eligible PASS entries. O0 remains shadow-only; G0/P0/unknown
keys and duplicate bindings are rejected. This manifest checks experiment
eligibility, **not the wrapper/checkpoint's identity or deployed accuracy**;
checkpoint provenance and shadow comparison must be reviewed separately.

The existing adapter bounds request/output size and process lifetime. A missing,
malformed, failing or timed-out process becomes a failed organ invocation rather
than a new individual. There are at most 16 organs, sequential execution and a
per-organ timeout of at most 10 seconds: aggregate latency can exceed the tick
interval. A fresh active signal leads to the conservative `observe` intent;
no fresh active signals, observation expiry, or any active-organ failure leads
to `wait`. Shadow signals/failures do not vote in this policy.

Process adapters are **not an OS security sandbox**. Programs run with the
operator's filesystem, environment and network privileges. Configure only
trusted wrappers or apply an external sandbox. The Rust `register` API likewise
accepts trusted in-process code, which must not block/panic indefinitely.

## Deliberate v0 limits

This is the first runnable separation boundary, not a learned/autonomous brain.
The `wait/observe` policy is deterministic scaffolding, not K0/CX training.
No J72 generation, Jev API request, memory judgment, FBA controller, MIOBA
simulation, drive learning or durable-goal policy is automatically connected.
These remain replaceable adapters/policies; none is an owner of canonical
identity. The existing language/resource/MIO implementations remain available
through the existing runtime rather than being implicitly activated here.
No model downloads, API billing, GPU jobs or production deployment occur.

## Verification

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p kamimusuhi-runtime --lib
cargo test -p kamimusuhi-runtime --test kcore_loop
bash scripts/ci-local.sh
```

`K-CORE focused CI` runs formatting, Clippy, runtime units and the K-CORE
acceptance suite independently so failures in unrelated workspace suites do
not hide this slice's result. It does not replace or weaken the full Rust CI.
Tests cover model-free boot/restart, no canonical changes, bounded queues and
source registry, replay/stale/invalid rejection, shadow isolation, active
failure, process failure, ephemeral hidden state, and release after a killed
CLI process. All organ results in these tests are **fixtures**, not research
success claims or live checkpoint validation.
