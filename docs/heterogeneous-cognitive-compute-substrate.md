# Heterogeneous Cognitive Compute Substrate

Status: **design hypothesis / research note**  
Scope: computation, model architecture, precision, and hardware placement beneath/around the canonical Persona Core.  
Normative status: **non-normative**. `architecture.md` and `spec.md` remain authoritative.

Related:

- `learned-discrete-reflex-layer.md`
- `multiscale-brain-architecture.md`
- `peripheral-neural-layer.md`
- `native-runtime-stratification.md`
- `runtime-language-and-hardware-compatibility.md`
- `latency-architecture.md`

---

## 1. Thesis

Kamimusuhi should not assume that one model family, one numerical precision, or one accelerator is the correct substrate for every cognitive function.

The working hypothesis is:

```text
high-frequency / low-semantic-cost decisions
    -> discrete, integer, LUT, FSM, tiny learned controller

streaming temporal state
    -> recurrent / state-space / linear-attention-like substrate

precise associative lookup
    -> sparse or full attention when actually needed

ordinary local language cognition
    -> quantized Transformer / hybrid model

hard deliberation
    -> higher-precision / larger local or external model
```

The design target is therefore a **heterogeneous cognitive substrate** rather than a single homogeneous neural model.

---

## 2. Evidence and motivating lines of work

### 2.1 Learned discrete computation

Deep Differentiable Logic Gate Networks and related LUT/weightless-network work show that some learned functions can be compiled to very cheap discrete inference substrates.

Kamimusuhi should interpret this narrowly: routing, salience, wake/sleep, simple temporal control, memory-triggering, and reflex selection are plausible targets. Language generation and canonical identity are not.

See `learned-discrete-reflex-layer.md` for the detailed boundary and benchmark plan.

### 2.2 Boolean or discrete routing with higher-precision values

Recent experimental work around Boolean attention-like routing is especially interesting because it exposes a useful architectural separation:

```text
cheap/discrete decision: where should information flow?
                      +
expensive/continuous value: what information should flow?
```

The lesson is not that all attention should become Boolean. The stronger design principle is that **routing and payload do not need the same representation or precision**.

### 2.3 Mixed precision and activation outliers

LLM quantization work repeatedly shows that ordinary activations and rare/high-impact outlier features may benefit from different numerical treatment.

Kamimusuhi should generalize this beyond quantization:

```text
ordinary / frequent / cheap path
    -> low precision, low power, always available

rare / important / difficult path
    -> higher precision, higher cost, escalated on demand
```

This matches the existing Cognitive Budget and multi-timescale architecture better than forcing every event through the most expensive model.

### 2.4 Attention alternatives and hybrid models

Linear-attention, recurrent/state-space, and full-attention mechanisms have different strengths. The durable lesson is not that one replaces all others, but that hybrid architectures can assign different temporal/information-processing roles to different mechanisms.

Kamimusuhi should therefore avoid a normative commitment such as "all cognition must be Transformer attention". Architecture contracts should describe required behavior and state semantics while allowing backend/model-family replacement.

### 2.5 Hardware/model co-design

Model shape is part of the deployment problem. Hidden dimensions, head dimensions, vocabulary size, depth/width balance, quantization format, batch structure, and kernel availability can materially change real throughput on a specific device even at similar nominal parameter counts.

Therefore model research should optimize over:

```text
quality
x latency
x memory footprint
x energy
x actual target-hardware throughput
```

rather than parameter count or FLOP estimates alone.

---

## 3. Proposed functional stratification

A possible substrate map is:

```text
sensors / event streams
        |
        v
+-----------------------------+
| PNL / discrete reflex       |
| Boolean / LUT / FSM / int   |
| salience / route / wake     |
+-------------+---------------+
              |
              v
+-----------------------------+
| K-Fast temporal substrate   |
| recurrent / SSM / sparse    |
| short state / regulation    |
+-------------+---------------+
              |
              v
+-----------------------------+
| K-Edge / local cognition    |
| quantized hybrid/Transformer|
+-------------+---------------+
              |
      precise lookup / hard task
              |
              v
+-----------------------------+
| full attention / larger     |
| higher-precision cognition  |
+-------------+---------------+
              |
              v
      tools / frontier / K-Deep
```

This is a role map, not a fixed implementation recipe.

---

## 4. Precision is a routing decision

Do not define one global precision policy.

Each organ/backend should advertise supported numerical modes and measured behavior. The runtime may route by a combination of:

- semantic importance;
- uncertainty;
- latency budget;
- available accelerator;
- energy/resource pressure;
- model quality requirement;
- memory pressure;
- current queue depth.

Conceptually:

```text
routine event
  -> bit/int8/int4/tiny model

uncertain or salient event
  -> fp16/bf16 or stronger local model

high-stakes / difficult reasoning
  -> deliberate larger model / external cognition
```

The deterministic policy/capability layer remains above learned routing for any action with authority or irreversible consequences.

---

## 5. Hardware placement

Potential mappings:

### FPGA / programmable logic

Best research targets:

- learned discrete reflex controllers;
- LUT/FSM routing;
- always-on event filtering;
- fixed low-latency feature transforms;
- deterministic streaming pipelines.

Do not begin by forcing a general LLM onto the FPGA. Move functions there only after software benchmarks show that they are stable, bounded, and hardware-suitable.

### CPU

Good default for:

- orchestration;
- deterministic policy;
- event queues;
- memory/index management;
- lightweight bit-packed/discrete control;
- fallback execution when accelerators are absent.

### GPU

Good default for:

- Persona Core inference;
- training;
- larger embedding/perception models;
- batch-heavy learned organs;
- architecture-search experiments.

### Edge / old / heterogeneous accelerators

Treat each device as an organ candidate with explicit capability discovery rather than pretending that all devices form one symmetric pool.

The scheduler should learn or measure:

```text
backend
supported kernels / dtypes
usable memory
transfer cost
startup cost
throughput vs batch/context
energy or wall-power delta
reliability / thermal constraints
```

and route only workloads the device can execute efficiently and reproducibly.

---

## 6. Relevance to the Nukui Extreme Node / heterogeneous GPU farm

The planned Nukui node is a particularly useful experimental target because its value is **heterogeneity**, not aggregate homogeneous FLOPS.

Current intended shape includes a TB250-BTC PRO + low-end CPU host and a mixed collection such as RX 5500 XT, RX 6400, GTX 550 Ti, GT710/GT730-class devices. It should be treated as an **Astra worker / YLSB-Kamimusuhi Extreme Node** rather than as a conventional multi-GPU LLM server.

The design question is:

> Given a pile of old, low-end, vendor-mixed GPUs, what useful cognitive or media/compute organs can each device still execute better than CPU-only fallback, and can a scheduler discover that automatically?

Do **not** assume cross-GPU tensor parallelism, unified runtime support, or useful peer-to-peer transfer. Instead prefer task-level decomposition:

```text
job arrives
   |
   v
capability + benchmark database
   |
   +--> GPU A: integer / lightweight inference candidate
   +--> GPU B: Vulkan/OpenCL-compatible batch kernel candidate
   +--> GPU C: image/video stage candidate
   +--> CPU: unsupported/fallback path
   +--> remote stronger node: escalation
```

This makes the machine valuable even when no single GPU can run a modern LLM well.

### Recommended Extreme Node benchmark dimensions

For every installed accelerator, record at minimum:

```text
identity / architecture / driver/runtime
VRAM and practical allocatable memory
host-link width and measured transfer rate
supported dtypes / kernel backends
model-load success/failure
prefill or batch throughput where applicable
decode throughput where applicable
image/video kernel throughput where applicable
power / thermals where measurable
failure rate and watchdog behavior
```

Then classify each card by **useful organ role**, not by a single synthetic score.

Candidate role labels:

```text
UNUSABLE_FOR_CURRENT_STACK
DISPLAY_ONLY
MEMORY_CAPACITY_EXPERIMENT
LIGHTWEIGHT_INFERENCE
BATCH_COMPUTE
IMAGE_STAGE
VIDEO_STAGE
DISCRETE_INTEGER_KERNEL
LEGACY_CUDA_EXPERIMENT
VULKAN_COMPUTE_EXPERIMENT
REMOTE_WORKER_HELPER
```

This is deliberately broader than LLM inference. The point of the Nukui node is to discover useful work for otherwise marginal hardware.

---

## 7. Hardware-aware architecture search for novllm and specialist organs

Model architecture experiments should include hardware as a first-class independent variable.

For a fixed corpus/training budget, sweep combinations such as:

```text
vocabulary size
hidden width
FFN width
number of layers
head dimension / head count
attention vs recurrent/hybrid blocks
precision / quantization
context length
```

Measure:

```text
validation BPB / downstream quality
characters per second
training time
peak VRAM
inference tokens per second
energy / wall power if available
artifact size
```

Vocabulary experiments are especially relevant because a larger vocabulary can reduce token sequence length while increasing embedding/output-head parameters and memory traffic. Therefore the best vocabulary size is not a tokenizer-only result; it is an **end-to-end model+hardware optimum**.

Do not assume the optimum on a modern GPU transfers to old GPUs, CPUs, Apple Silicon, FPGA, or the Nukui Extreme Node.

---

## 8. Scheduler contract

Long-term, the Cognitive Resource Router should schedule by capability and measured cost rather than device name.

Possible resource advertisement:

```text
ResourceProfile {
    latency_curve,
    throughput_curve,
    memory_capacity,
    supported_precisions,
    supported_model_families,
    supported_kernel_families,
    startup_cost,
    transfer_cost,
    energy_estimate,
    reliability,
    locality,
}
```

Routing inputs include:

```text
task kind
quality floor
latency deadline
context/input size
required memory
precision requirement
privacy/locality requirement
current load
cognitive budget
```

The router may then select a resource without encoding assumptions such as "GPU means fast" or "newer means always preferable".

---

## 9. Experimental roadmap

### H0 — Build the measurement harness

Create one benchmark schema that can record CPU, GPU, FPGA, and remote-resource results without pretending their metrics are identical.

### H1 — Establish homogeneous baselines

For each workload, first establish the best practical single-resource baseline.

### H2 — Mixed-precision / mixed-mechanism comparison

Compare conventional homogeneous execution with selective low/high-precision or hybrid-routing variants.

### H3 — Nukui heterogeneous-device role discovery

Run the same bounded workload families across every usable old GPU and CPU fallback, then assign role labels from measured evidence. Do not force unsupported devices through fragile stacks merely to produce a score.

### H4 — Learned resource routing

After enough traces exist, compare:

1. static rule routing;
2. shortest-queue routing;
3. cost-model routing;
4. learned routing.

Quality constraints and deterministic safety gates remain fixed.

### H5 — FPGA reflex offload

Only after a discrete controller wins in software, synthesize it to FPGA and compare latency, energy, and behavior parity.

---

## 10. What this architecture does not claim

This note does **not** claim that:

- Boolean networks can replace language models;
- linear attention universally beats full attention;
- lower precision is always more efficient;
- FPGA is automatically faster than GPU;
- old GPUs are automatically worth their electricity;
- mixed heterogeneous hardware should be used for one tightly-coupled model;
- every organ should be learned rather than deterministic;
- biological analogy proves engineering merit.

All such claims require measured workload-specific evidence.

---

## 11. Promotion criteria

Promote parts of this note into normative architecture only when:

1. a workload has a reproducible benchmark;
2. at least one heterogeneous/mixed strategy beats the homogeneous baseline on a meaningful objective;
3. quality loss remains within an explicit bound;
4. failure/fallback behavior is understood;
5. state and authority boundaries remain compatible with the canonical architecture;
6. deployment remains replaceable across hardware generations.

Until then this remains a research direction, not a required implementation.
