# Runtime Language, Execution Layers, and Hardware Backward Compatibility

Status: **living engineering design note / current preferred design**

This note complements [`native-runtime-stratification.md`](./native-runtime-stratification.md) and makes Kamimusuhi's implementation-language, dynamic execution, CPU ISA, GPU, and OS compatibility strategy explicit.

The current preferred design is:

> **Use Rust as the default language for the always-resident life-support/nervous-system runtime, Python as the research and learning language, C/C++/CUDA/Metal as acceleration organs, and WASM/scripts/DSLs as replaceable behavior layers.**

Kamimusuhi as a whole should not depend on one interpreter for continued operation. Dynamic execution remains desirable for self-improvement, skills, habits, and experimentation, but it should sit above a smaller native survival/continuity core.

---

## 1. Core principle

Kamimusuhi should be implemented as a **polyglot organism centered on a native nervous system**, rather than as a single-language application.

```text
Kamimusuhi
│
├─ Rust: always-resident organism runtime
│  ├─ heartbeat / lifecycle
│  ├─ event bus / interrupt / cancellation
│  ├─ continuity / state transition
│  ├─ memory plumbing / IPC
│  ├─ sensor and actuator routing
│  ├─ model / organ supervision
│  └─ capability discovery / watchdog
│
├─ Python: research and learning plane
│  ├─ training / fine-tuning
│  ├─ novllm / tokenizer experiments
│  ├─ evaluation / benchmark analysis
│  ├─ memory and cognition prototypes
│  └─ offline consolidation experiments
│
├─ C / C++ / accelerator code: performance leaves
│  ├─ llama.cpp and native inference backends
│  ├─ SIMD / DSP kernels
│  ├─ CUDA / ROCm
│  └─ Metal / platform libraries
│
└─ WASM / script / DSL: mutable behavior layer
   ├─ skills
   ├─ habits
   ├─ experimental policies
   └─ bounded self-modifiable modules
```

Language boundaries are not dogma and may move after profiling. The important constraint is that **minimum life-support and continuity should not depend solely on the health of a high-level interpreter environment**.

---

## 2. Is Kamimusuhi an interpreted system?

### 2.1 The Core should default to a compiled native binary

The minimum resident functions of `kamimusuhi-core` / `kamimusuhi-edge` should preferentially be implemented as Rust native binaries.

Candidate Core responsibilities include:

- startup, shutdown, restart, watchdog;
- event loop;
- interruption and cancellation;
- canonical state mutation boundary;
- continuity and lineage primitives;
- resource-health observation;
- organ discovery;
- local IPC;
- crash-safe journal boundaries.

This keeps the minimum runtime recoverable even when a Python environment, virtual environment, package dependency, JIT, or individual skill is broken.

### 2.2 Keep a dynamically replaceable layer

A native Core does not imply banning dynamic execution.

Skills, habits, experimental policies, and short-lived cognition modules may use:

- a WASM execution substrate;
- Python workers;
- a small scripting runtime such as Lua;
- a future Kamimusuhi-specific DSL;
- versioned external executables or plugins.

For self-improvement, a **small fixed execution kernel plus permission-bounded mutable modules** is easier to validate, roll back, isolate, and trace than rewriting and recompiling the Core for every behavioral change.

WASM is treated here as a **sandboxable execution substrate**, not strictly as an interpreter, because implementations may use interpretation, JIT, or AOT compilation.

---

## 3. Hardware backward-compatibility strategy

Rust does not automatically make a binary compatible with old CPUs. Compatibility depends on the **target triple, CPU baseline, enabled ISA, libc/OS requirements, and dependencies**.

Distributable Core builds should therefore be separated from machine-local optimized builds.

### 3.1 Conservative baseline for distributable Core binaries

A distributable `kamimusuhi-core` SHOULD:

- avoid `target-cpu=native`;
- explicitly use a conservative x86_64 baseline;
- avoid making AVX, AVX2, or AVX-512 mandatory for Core startup;
- make the ARM64 baseline/platform acceleration boundary explicit;
- retain scalar/reference fallbacks for essential paths;
- test that dependencies do not silently raise the ISA requirement.

The exact minimum x86_64 baseline should be fixed only after real-hardware and target-OS testing. An `x86-64-v1`-class baseline is a candidate, but naming a baseline is not itself proof of compatibility.

### 3.2 Reserve `target-cpu=native` for local optimized artifacts

Host-specific builds may use `target-cpu=native`, PGO, LTO, and platform-specific optimizations.

Such binaries must be treated as host-local or compatibility-scoped artifacts and should not be copied blindly to older CPU generations.

---

## 4. Treat ISA/GPU acceleration as optional organs

CPU/GPU-specific optimization should normally be separated from the Core rather than becoming a Core boot requirement.

```text
kamimusuhi-core-baseline
        │
        ├─ detect x86/ARM features
        ├─ detect GPU / driver / runtime
        ├─ detect memory / storage / network
        │
        └─ load compatible organs
             ├─ x86-avx2
             ├─ x86-avx512
             ├─ arm-neon
             ├─ cuda
             ├─ rocm
             ├─ metal
             └─ scalar/reference
```

### Compatibility invariants

1. The continuity/event runtime SHOULD be able to boot without a GPU.
2. Lack of AVX2/AVX-512 SHOULD not normally prevent Core startup.
3. Hardware-specific organs activate only after capability detection.
4. Missing unsupported organs are handled as graceful degradation.
5. Canonical state schemas and identity lineage remain independent of CPU ISA and GPU vendor.
6. Host migration or accelerator replacement alone does not create a new identity.
7. Any capability that truly requires an accelerator is declared in the capability manifest.

---

## 5. Startup capability discovery

The Core should mechanically inspect the machine rather than relying on generated prose to describe its embodiment.

Conceptual boot sequence:

```text
boot baseline core
      ↓
probe architecture / ISA
      ↓
probe RAM / storage / network
      ↓
probe GPU / accelerator / driver
      ↓
validate organ compatibility
      ↓
activate compatible organs
      ↓
publish Embodiment State / Capability Manifest
```

Candidate manifest structure:

```yaml
host:
  arch: x86_64
  os: linux
cpu:
  baseline: x86_64
  features: [sse2, avx, avx2]
accelerators:
  - kind: cuda
    device: "..."
    runtime: "..."
organs:
  available: [...]
  disabled_incompatible: [...]
limits:
  memory_bytes: ...
  storage_bytes: ...
```

Persona-level self-description should derive this information from the runtime-owned `Embodiment State`, not maintain a second prose source of truth.

---

## 6. OS, libc, and driver compatibility

Matching the CPU ISA does not guarantee compatibility with an old machine.

Linux compatibility should separately test:

- minimum kernel;
- minimum glibc;
- components suitable for `musl` static builds;
- native dependencies such as TLS libraries;
- GPU driver / CUDA / ROCm runtime requirements;
- filesystem and syscall assumptions.

Self-contained/static artifacts are useful where appropriate, but universal static linking is not a goal. Accelerator compatibility should be tracked separately from Core compatibility because GPU drivers and runtimes impose their own constraints.

---

## 7. Role of low-performance and old hosts

Kamimusuhi does not require every node to run the same model.

Older machines may still host organs such as:

- event relay;
- sensor ingestion;
- watchdog;
- memory cache;
- low-cost reflex;
- storage;
- telemetry;
- offline batch work;
- network gateway.

Heavy Persona Core / K-Core inference may be delegated to an appropriate accelerator host.

The compatibility goal is therefore not "full performance on every old computer" but **"old hardware can still participate as part of the organism when it has a useful compatible organ."**

---

## 8. Build and artifact policy

Conceptually separate artifacts into classes such as:

```text
release/
  kamimusuhi-core-linux-x86_64-baseline
  kamimusuhi-core-linux-aarch64-baseline
  kamimusuhi-core-macos-aarch64-baseline

accelerators/
  organ-x86-avx2
  organ-x86-avx512
  organ-cuda-<compat-class>
  organ-rocm-<compat-class>
  organ-metal-<compat-class>

local-optimized/
  kamimusuhi-core-<host-id>-native
```

Packaging may change. The important rule is not to blur the meaning of a baseline Core binary and a hardware-specific optimized artifact.

---

## 9. Self-modification and plugin safety

Mutable script/WASM/plugin layers must not implicitly receive the same authority as the Core.

At minimum, consider:

- capability-based permissions;
- CPU, memory, and wall-time quotas;
- filesystem/network scope;
- restricted canonical-state mutation APIs;
- version and provenance;
- signatures/hashes;
- rollback;
- crash isolation;
- deterministic/reference tests where practical.

The ability to write code is distinct from the authority to mutate canonical self-state.

---

## 10. Validation matrix

Language and compatibility policy must be validated through compatibility tests and benchmarks rather than by declaration.

Minimum candidate cases:

```yaml
boot:
  baseline_cpu_only
  unsupported_accelerator_present
  accelerator_missing
  old_linux_userspace
  arm64_edge_host

isa:
  baseline_x86_64
  avx2_enabled
  avx512_enabled
  arm64_neon

failure:
  python_worker_broken
  wasm_skill_trap
  gpu_driver_unavailable
  optimized_organ_illegal_instruction

continuity:
  state_migration_between_hosts
  restart_on_lower_capability_host
  accelerator_replacement
```

Expected properties:

- optional organ failure does not take down the Core;
- capability downgrade is explicit;
- lineage remains continuous after host migration;
- optimized-code failure can fall back to a baseline path where feasible;
- failure of Python/WASM layers alone does not stop heartbeat/continuity.

---

## 11. Current adoption policy

For the first serious implementation:

1. **Rust is the leading candidate for the always-resident Core/nervous-system runtime.**
2. **The Core itself should not require an interpreter-hosted application model.**
3. **Python remains for training, research, prototypes, and offline cognition.**
4. **C/C++/CUDA/Metal remain appropriate for inference backends and leaf acceleration.**
5. **WASM/scripts/DSLs are allowed as bounded mutable behavior layers.**
6. **Distributable Core binaries use conservative baselines; host-specific optimization is separate.**
7. **AVX2/AVX-512/GPU acceleration should normally be optional organs rather than Core boot requirements.**
8. **Hardware replacement/migration remains separate from identity continuity.**
9. **Final CPU/OS minimums are decided by a measured compatibility matrix.**

In one sentence:

> **Rust is the native language, Python is the research language, native accelerators are the muscles, and WASM/DSLs are the rewritable skill layer.**
