# Native Runtime Stratification for Low-Latency Cognition

Status: **living engineering design note / design hypothesis**

This note refines the implementation strategy behind `latency-architecture.md`. It does **not** require Kamimusuhi to be implemented in one language. The working hypothesis is that perceived immediacy is easier to achieve if the latency-critical nervous-system path is native and always resident, while Python remains available where iteration speed and ML ecosystem access matter more than microsecond-to-millisecond overhead.

The important distinction is not "Python bad, C good." It is **hot path vs control/research path**, and **long-lived runtime vs repeated process startup/serialization**.

## 1. Design hypothesis

Kamimusuhi should be allowed to use multiple implementation layers:

```text
┌──────────────────────────────────────────────────────────────┐
│ Python / notebooks / training / research / tool adapters     │
│ rapid iteration, model experiments, evaluation, offline jobs │
├──────────────────────────────────────────────────────────────┤
│ Native cognitive runtime (Rust and/or C++)                   │
│ event loop, reflex, salience, routing, IPC, state plumbing   │
├──────────────────────────────────────────────────────────────┤
│ Native inference / DSP / storage libraries                   │
│ llama.cpp, MLX/Metal bridges, SQLite, vector/search kernels  │
├──────────────────────────────────────────────────────────────┤
│ ISA / accelerator-specialized leaf kernels                   │
│ SIMD intrinsics, NEON, AVX2/AVX-512, Metal/CUDA, rare ASM    │
└──────────────────────────────────────────────────────────────┘
```

Python can remain the best language for many Kamimusuhi components even if it is excluded from the strict reflex critical path.

## 2. Why this matters specifically to Kamimusuhi

The latency architecture currently targets approximately:

- reflex/event reaction: 20–100 ms class;
- K-Edge first response: 100–300 ms class where practical;
- sub-second to low-seconds K-Core response start.

At these budgets, model inference is not the only source of delay. Event dispatch, process wakeups, serialization, IPC, memory retrieval, audio buffering, scheduling jitter, garbage collection, interpreter contention, and cold process/model startup can become visible.

A persistent artificial organism also differs from a request/response web application: the nervous system is expected to stay alive, react to unsolicited events, process sensory streams, and interrupt ongoing work. This favors an always-resident runtime rather than repeatedly launching scripts.

## 3. Recommended responsibility split

### 3.1 Native latency-critical runtime

Strong candidates for Rust/C++ implementation:

```text
K-Edge runtime
├── event bus / interrupt dispatch
├── reflex gate
├── salience and novelty scoring hot path
├── streaming sensory frame plumbing
├── VAD/audio chunk routing around existing DSP libraries
├── resource-health and watchdog signals
├── cognitive-budget fast classification
├── cancellation / interruption propagation
├── cache-key and cache-state plumbing
├── shared-memory / local IPC transport
└── actuator streaming and backpressure

Continuity/runtime infrastructure
├── atomic state-transition primitive
├── lineage/head compare-and-swap
├── crash-safe journal boundary
└── split-brain / stale-writer rejection primitive
```

The native runtime does **not** need to contain all cognition. It should provide the low-latency nervous system in which slower cognition can participate.

### 3.2 Python remains appropriate

Good Python territory:

- model training and fine-tuning;
- dataset generation and evaluation;
- experimental cognition policies;
- offline memory consolidation and dream experiments;
- research scripts and benchmark analysis;
- tool/provider adapters where network/model latency dominates;
- early prototypes before profiling demonstrates a bottleneck;
- orchestration that makes coarse-grained calls into native backends.

If a Python process makes one coarse llama.cpp/MLX request that takes hundreds of milliseconds or seconds, Python interpreter overhead may be negligible. Rewriting that wrapper in C will not magically accelerate the model.

## 4. Rust vs C++ vs C

### Rust

Recommended default candidate for a new always-resident Kamimusuhi runtime because it provides:

- memory safety without a garbage collector;
- strong concurrency primitives;
- good async and systems-programming ecosystem;
- straightforward C ABI interoperability;
- suitability for long-lived daemons where corruption is especially undesirable.

Potential cost: integration with C++-centric ML projects can require an FFI boundary.

### C++

Recommended where tight integration with existing inference/runtime code matters, especially llama.cpp-style codebases and performance libraries.

Advantages:

- direct ecosystem compatibility with many ML/inference systems;
- mature SIMD/compiler optimization support;
- low abstraction overhead when written carefully.

Potential cost: larger memory-safety and lifetime-management burden in a persistent multi-threaded organism.

### C

Useful as a **stable interoperability ABI** and for very small low-level components. It is not necessary to make C the principal application language merely because latency matters.

A practical boundary is:

```text
Rust runtime
    │
    ├── C ABI ── llama.cpp / native DSP / storage
    │
    └── IPC/FFI ── Python experimental workers
```

or, if inference integration dominates:

```text
C++ runtime
    │
    ├── native inference backends
    └── narrow C ABI / IPC boundary to Python and other organs
```

The choice should be benchmarked and can vary by organ.

## 5. Executables: useful, but process boundaries are not free

A native executable is useful for:

- isolation;
- crash containment;
- independent deployment to K-Edge/K-Core machines;
- stable service ownership;
- watchdog/restart semantics;
- hardware-specific builds.

However, **spawning a new executable per event is the wrong low-latency pattern**. Process creation, model/library initialization, serialization, and cache loss can cost more than Python dispatch itself.

Prefer long-lived processes:

```text
kamimusuhi-edge       # always-resident K-Edge nervous system
kamimusuhi-core       # K-Core integration / continuity service
kamimusuhi-memory     # optional native/search memory service
python workers        # replaceable experiments/tools/offline cognition
model backends        # llama.cpp / MLX / other resident servers
```

The exact process split should remain small initially. Too many microservices can create latency and operational complexity.

## 6. IPC hierarchy

For same-machine communication, benchmark from the simplest viable mechanism upward.

Conceptual ordering by expected overhead, not a guaranteed universal ranking:

```text
same process / direct call
        ↓
FFI through a narrow C ABI
        ↓
shared-memory ring buffer + wakeup primitive
        ↓
Unix-domain socket / named pipe with compact binary messages
        ↓
local RPC
        ↓
HTTP + JSON
        ↓
spawn-a-process-per-request
```

Use structured provenance and versioning regardless of transport.

For high-rate sensory streams, do not serialize full audio/video frames into JSON. Pass bounded binary buffers or shared-memory references and send small metadata/events separately.

## 7. Assembly should be a leaf optimization, not an architectural layer

Handwritten assembly is justified only after profiling identifies a stable leaf kernel that:

1. consumes a material part of the latency/CPU budget;
2. cannot be adequately optimized by the compiler or an existing library;
3. has enough benchmark coverage to preserve correctness across hardware.

Prefer first:

- compiler optimization (`-O3`, LTO/PGO where useful);
- data-layout changes;
- batching elimination where latency matters;
- branch/cache optimization;
- SIMD intrinsics or portable SIMD;
- optimized platform libraries;
- Metal/CUDA/Accelerate/vectorized DSP;
- existing inference kernels.

Then consider ISA-specific code:

```text
Apple Silicon / ARM64: NEON and platform acceleration
x86-64: AVX2 / AVX-512 where available
GPU: Metal / CUDA kernels
```

Handwritten assembly should normally remain behind a feature-tested dispatch layer with a scalar/reference implementation.

## 8. The largest wins may not come from rewriting Python

For Kamimusuhi, likely higher-value latency work includes:

- keep K-Edge and Persona Core models resident;
- stable prefix/KV cache reuse;
- progressive ASR and memory prefetch;
- parallel retrieval;
- streaming TTS/output;
- eliminating cold model/process starts;
- cancellation and interruption rather than waiting for stale work;
- reducing copies/serialization of sensory data;
- choosing the correct compute tier before expensive inference;
- speculative decoding where measured to help.

A native runtime should make these mechanisms cheaper and more deterministic; it is not a substitute for them.

## 9. Proposed v1 runtime shape

```text
                    sensors / UI / network
                            │
                            ▼
                 ┌─────────────────────┐
                 │ kamimusuhi-edge     │  Rust/C++ native daemon
                 │---------------------│
                 │ event bus           │
                 │ reflex gate         │
                 │ salience            │
                 │ interrupt/cancel    │
                 │ stream plumbing     │
                 │ fast resource view  │
                 └──────┬──────────────┘
                        │
             ┌──────────┼──────────┐
             │          │          │
             ▼          ▼          ▼
         K-Edge      memory     actuator
         model       service      stream
             │          │          │
             └──────┬───┴──────────┘
                    ▼
             ┌─────────────────┐
             │ kamimusuhi-core │ native or mixed runtime
             │-----------------│
             │ workspace       │
             │ router          │
             │ continuity      │
             │ escalation      │
             └──────┬──────────┘
                    │
          ┌─────────┼─────────────┐
          ▼         ▼             ▼
      K-Core     Python        K-Deep /
      model      workers       external models
```

Initial implementation can collapse `edge` and `core` into one binary on one host while preserving the logical boundary.

## 10. ABI and message design

Keep the native boundary narrow. A versioned event envelope is preferable to exposing internal C++/Rust object layouts.

Conceptually:

```text
EventEnvelope
  version
  monotonic_timestamp
  source_id
  event_type
  priority
  deadline
  provenance
  payload_ref / compact payload
  cancellation_token
  trace_id
```

Latency-critical events should carry deadlines/priority so an old background task cannot block a new interrupt.

For state mutation, separate event transport from canonical commit semantics. Fast transport must not bypass Continuity Kernel authority.

## 11. Benchmark before language migration

Before rewriting a component, measure p50/p95/p99 rather than only average latency.

Minimum benchmark set:

```yaml
case:
  event_dispatch
  reflex_rule
  salience_update
  local_ipc_roundtrip
  memory_lookup
  audio_chunk_handoff
  cancellation_propagation
  K-Edge_request

runtime:
  python_sync
  python_async
  native_rust_or_cpp

metrics:
  p50_us_or_ms
  p95_us_or_ms
  p99_us_or_ms
  cpu_percent
  allocations
  copies
  wakeups
  resident_memory
```

Also measure under contention while K-Core inference is running. Tail latency under load is more important to the "alive" feeling than an unloaded microbenchmark.

## 12. Migration rule

Do not pre-emptively rewrite every Python component.

Use this progression:

```text
prototype in the fastest language to iterate
        │
        ▼
instrument and trace
        │
        ▼
identify hot path / jitter source
        │
        ▼
move the narrow critical primitive native
        │
        ▼
keep a stable ABI / event contract
        │
        ▼
benchmark parity + latency regression
```

The desired endpoint is a **polyglot organism with a native nervous system**, not a ceremonial all-C codebase.

## 13. Current recommendation

For a first serious Kamimusuhi implementation:

1. Treat **Rust** as the leading candidate for a new K-Edge/event/continuity runtime.
2. Use **C++** directly where inference backend integration makes it cheaper or simpler.
3. Preserve a **C ABI** at important interop boundaries.
4. Keep **Python** for training, research, experimental cognition and coarse orchestration.
5. Use **long-lived executables/daemons**, not subprocess-per-event execution.
6. Treat **assembly/SIMD as profiler-driven leaf optimization** only.
7. Benchmark Python-vs-native p99 latency before and after each migration.

This is a design hypothesis until measured on the actual Kamimusuhi hardware mix. The implementation should preserve the architecture-level K-Edge/K-Core/K-Deep boundaries even if specific language choices change.
