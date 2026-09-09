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

identity-sensitive or difficult cognition
    -> higher-precision Persona Core / specialist / external model
```

The important architectural shift is from:

```text
one homogeneous Transformer does everything
```

to:

```text
function -> required information fidelity -> suitable primitive -> suitable precision -> suitable hardware
```

This is a **heterogeneous cognitive substrate**: heterogeneity is intentional rather than an implementation accident.

---

## 2. Evidence and motivating observations

### 2.1 Learned logic is most plausible for routing/reflex, not semantic payload

Differentiable Logic Gate Networks (DLGN) show that a learned network can be trained through a differentiable relaxation and later discretized into Boolean logic for cheap inference.

Primary reference:

- Petersen et al., *Deep Differentiable Logic Gate Networks*, NeurIPS 2022: https://proceedings.neurips.cc/paper_files/paper/2022/hash/0d3496dd0cec77a999c98d35003203ca-Abstract-Conference.html

The existing Kamimusuhi interpretation is documented in `learned-discrete-reflex-layer.md`: the promising target is salience, wake/sleep, routing, prefetch, interruption, and resource selection rather than replacing language cognition wholesale.

A 2026 community experiment on a Boolean-attention-style language model reached a similar qualitative conclusion after several failed variants: discretizing the *routing decision* was substantially more viable than forcing the whole semantic value path into Boolean logic. This is useful evidence, but it is not a peer-reviewed result and must not be treated as proof that Boolean attention exceeds Transformers generally.

- https://zenn.dev/karumaru/articles/24bca710a2db62

**Design hypothesis:** use low-cost/discrete computation to decide *whether and where to spend cognition*, while preserving richer representations on the path that carries semantic content.

### 2.2 Precision should be heterogeneous too

`LLM.int8()` demonstrated that rare activation outliers can be disproportionately important to Transformer quality. The practical solution was not uniform low precision: ordinary dimensions were handled in INT8 while outlier dimensions retained higher precision.

Primary reference:

- Dettmers et al., *LLM.int8(): 8-bit Matrix Multiplication for Transformers at Scale*, 2022: https://arxiv.org/abs/2208.07339

Secondary explanation:

- https://zenn.dev/bilzard/articles/llm-int8-matrix-multiplication

The broader lesson for Kamimusuhi is not specifically “use INT8”. It is:

> **Do not assume every signal deserves the same numerical precision or computational budget.**

This applies at multiple scales:

```text
most events              -> cheap screening
rare salient events      -> expensive escalation
most activation channels -> low precision
critical channels        -> higher precision
ordinary memory hits     -> approximate retrieval
identity-critical state  -> exact/versioned handling
```

Precision heterogeneity is therefore conceptually aligned with cognitive-budget heterogeneity.

### 2.3 Model dimensions are hardware decisions

The model/hardware co-design literature shows that nominally similar Transformer parameter counts can have materially different throughput because GEMM dimensions interact with tile sizes, SM occupancy, tensor-core requirements, and wave quantization.

Primary reference:

- *The Case for Co-Designing Model Architectures with Hardware*, 2024: https://arxiv.org/abs/2401.14489

Secondary summary:

- https://zenn.dev/bilzard/articles/co-designing-model-architecture-with-hardware

The cited work reports, among other examples, that changing head dimensions to hardware-friendlier multiples can improve throughput materially at similar parameter scale.

**Kamimusuhi implication:** native Persona Core / specialist models should not inherit hidden size, FFN width, head dimensions, vocabulary size, depth, and parallelism settings only because previous models used them. Architecture search should include the actual target hardware.

For `novllm`-derived models this means that tokenizer/vocabulary experiments and network-shape experiments should eventually be evaluated jointly:

```text
vocab size
x hidden width
x FFN width
x head count / head dimension
x depth
x context length
x quantization / dtype
x target hardware/runtime
```

A model that is theoretically elegant but maps badly to the intended hardware is not automatically the best organism substrate.

### 2.4 Attention should be treated as a capability, not a religion

Surveys of self-attention alternatives have long grouped approaches into sparse/structured attention, approximate/linear attention, recurrence/state-space alternatives, and implementation-level optimizations.

Secondary references:

- https://zenn.dev/bilzard/articles/self-attention-alternatives
- https://zenn.dev/bilzard/articles/survey-various-methods-used-in-llm-pre-training

The durable design lesson is that the system does not have to choose one primitive globally. It can reserve expensive precise attention for the parts of cognition where exact long-range association is valuable, while other temporal processing uses cheaper recurrent/stateful mechanisms.

For Kamimusuhi this maps naturally onto its existing multi-timescale design:

```text
K-Nerve / PNL      -> event/reflex/state machine/discrete learned control
K-Fast             -> cheap persistent temporal dynamics
K-Edge             -> low-latency semantic cognition
Persona Core       -> richer identity-bound language cognition
K-Core / K-Deep    -> deliberate or expensive reasoning
external resources -> bounded specialist/frontier computation
```

---

## 3. Proposed heterogeneous substrate map

This is a design map, not an implementation mandate.

| Function | Candidate primitive | Typical precision | Candidate hardware |
| --- | --- | --- | --- |
| interrupt / wake / watchdog | FSM, rules, LUT, DLGN | bits / integers | MCU, CPU, FPGA |
| salience / routing / escalation | tree, LUT, tiny MLP, DLGN | bit / INT / small FP | CPU, FPGA, small GPU |
| always-on temporal regulation | recurrent controller, sparse state machine, SSM-like model | INT / low FP | CPU, edge accelerator, GPU |
| sensor screening | DSP + tiny learned model | INT8 / FP16 | MCU, FPGA, CPU/GPU |
| local semantic response | quantized Transformer/hybrid | 4–8 bit weights + wider accumulators | modern GPU / Apple Silicon |
| long-context exact association | full/sparse attention | quantized or FP16/BF16 | GPU |
| Persona Core | native medium model | measured mixed precision | best local GPU(s) |
| difficult specialist reasoning | specialist / frontier model | provider-dependent | local large GPU / remote resource |

Important boundary:

> heterogeneous compute may change *how cognition is executed*, but must not silently create multiple competing canonical selves.

Canonical identity, autobiographical continuity, authority, and durable memory integrity remain above replaceable compute substrates.

---

## 4. Implications for a “limit GPU server” / heterogeneous old-GPU rack

A rack assembled from old and mismatched GPUs should not be treated as a bad imitation of one homogeneous modern accelerator.

The preferred architecture is **role specialization and task routing**, not pretending all cards are interchangeable tensor-parallel workers.

### 4.1 Avoid synchronous heterogenous tensor parallelism by default

A fast GPU and a slow/legacy GPU placed in the same tightly synchronized tensor-parallel group tend to inherit the slowest worker's latency and may also face backend/kernel compatibility differences.

Prefer:

```text
request / operation
      |
      v
hardware-aware router
   |       |       |
   v       v       v
modern   legacy   CPU/FPGA
GPU      GPU
```

Use heterogeneous devices as independent workers, pipeline stages only when measured beneficial, or separate specialist services.

### 4.2 Current local example: RTX 3060 + Tesla P100

These two GPUs should have different jobs.

#### RTX 3060 (Ampere, 12 GB)

Strengths:

- modern Tensor Cores;
- modern CUDA kernels and quantized inference paths;
- compatible with current runtimes that require relatively recent compute capability;
- good target for INT8/4-bit inference, modern attention kernels, and latency-sensitive model serving.

Likely role:

```text
K-Edge / quantized local LLM
modern-kernel inference
low-latency request handling
quantized experimentation
```

#### Tesla P100 16 GB (Pascal GP100, compute capability 6.0)

NVIDIA specifies the PCIe 16 GB P100 at up to 732 GB/s HBM2 bandwidth and 18.7 TFLOP/s FP16, but it predates Tensor Cores. GP100 also lacks the DP4A INT8 dot-product instruction found in compute capability 6.1 Pascal parts.

Primary NVIDIA references:

- https://www.nvidia.com/ja-jp/data-center/tesla-p100/
- https://developer.nvidia.com/blog/mixed-precision-programming-cuda-8/

This means **“lower bit = faster” must not be assumed on P100**. Quantization may still reduce storage/VRAM pressure, but modern INT8/Tensor-Core speedups are not its natural strength.

Likely role:

```text
FP16/FP32 dense training or evaluation
bandwidth-heavy inference
VRAM-capacity worker
long-running batch jobs
specialist model that benchmarks well on GP100
```

This fits observed local use: the P100 can remain valuable for training even when modern serving frameworks increasingly optimize for newer GPUs.

### 4.3 Runtime support is now part of hardware architecture

As of 2026, software lifecycle is itself a first-class hardware constraint:

- current vLLM NVIDIA documentation requires compute capability **7.5+**, excluding P100;
- current bitsandbytes supports several quantization/optimizer paths on **SM60+**, but its documented `LLM.int8()` fast path has historically required newer hardware and current docs recommend newer architectures for best performance;
- CUDA Toolkit 13 removed offline compilation/library support for Maxwell, Pascal, and Volta; NVIDIA states CUDA 12.x remains the supported build line for those architectures.

References:

- https://docs.vllm.ai/en/stable/getting_started/installation/gpu/
- https://huggingface.co/docs/bitsandbytes/installation
- https://docs.nvidia.com/cuda/archive/13.0.0/cuda-toolkit-release-notes/index.html

Therefore a limit-GPU rack should make toolchain compatibility explicit:

```text
modern lane: CUDA 13.x / current kernels / modern GPUs
legacy lane: CUDA 12.x / pinned compiler + runtime / Pascal/Volta/Maxwell
CPU lane:    ISA-aware portable fallback
FPGA lane:   synthesized discrete/peripheral functions
```

Do not let the modern lane silently break the legacy lane during routine upgrades.

### 4.4 Candidate roles for other cheap legacy cards

These are hypotheses to benchmark, not purchase recommendations.

**P40 / compute capability 6.1**

- 24 GB-class capacity is attractive;
- DP4A makes it more interesting for legacy INT8-style inference than GP100;
- no Tensor Cores;
- potentially useful as a capacity/inference specialist if runtime support is pinned.

**M40 / Maxwell**

- capacity may be cheap;
- substantially worse software-lifecycle position;
- current CUDA 13 toolchains no longer target Maxwell;
- modern quantization libraries increasingly de-emphasize/deprecate Maxwell.

Therefore M40-like cards should be acquired only when the *measured workload per yen* beats alternatives enough to justify a frozen legacy software lane.

**V100**

- Tensor Cores and 16/32 GB variants make it much more capable than P100 for FP16 tensor workloads;
- still falls into NVIDIA's legacy CUDA-12.x architecture bucket after CUDA 13;
- current vLLM's documented 7.5+ requirement also excludes compute capability 7.0 V100.

The lesson is that **VRAM/yen is necessary but no longer sufficient**. A limit-server GPU needs a role that survives its kernel/toolchain constraints.

---

## 5. A hardware-aware scheduler for Kamimusuhi

The resource router should eventually reason about more than free VRAM.

Candidate capability descriptor:

```text
DeviceCapability {
    device_id
    architecture
    compute_capability
    vram_bytes
    memory_bandwidth_class
    tensor_core_generation?
    native_dtypes[]
    efficient_dtypes[]
    supported_backends[]
    max_supported_cuda_lane?
    measured_prefill_profiles[]
    measured_decode_profiles[]
    measured_training_profiles[]
    watts_idle?
    watts_load?
}
```

A job descriptor can then declare what matters:

```text
CognitiveJob {
    latency_class
    model
    context_length
    operation = prefill | decode | train | embed | route | batch_eval
    precision_requirement
    min_vram
    backend_requirement?
    energy_budget?
}
```

Scheduling should be based on measured profiles:

```text
score(device, job)
  = expected_latency
  + energy_cost
  + compatibility_penalty
  + queue_cost
  + migration/startup cost
```

rather than:

```text
pick GPU with most free VRAM
```

This would turn a pile of mismatched devices into an intentional heterogeneous organism substrate.

---

## 6. Hardware-aware native model search

For a native `novllm`-derived Persona Core or specialist model, architecture search should include real device measurements.

Minimum sweep axes:

```text
vocab:       J64 / J72 / J96 / J128 / ...
hidden:      hardware-friendly multiples
FFN:         hardware-friendly multiples
heads:       divisibility + useful head dimension
depth:       fewer-wide vs more-narrow tradeoff
context:     actual target operating window
dtype:       FP32 / FP16 / BF16 / quantized paths as supported
runtime:     PyTorch / llama.cpp / other backend
hardware:    RTX 3060 / P100 / future GPU / CPU
```

Measure separately:

- training tokens/s;
- prefill tokens/s;
- decode tokens/s;
- peak VRAM;
- model load time;
- joules/token or wall-power delta where practical;
- stability and numerical quality;
- compilation/runtime maintenance burden.

Do not optimize model shape from parameter count alone.

### P100-specific warning

The tensor-core-oriented alignment rules in modern model/hardware co-design papers cannot be copied mechanically to GP100 because P100 has no Tensor Cores. Some alignment principles still matter for memory access, tiling, and occupancy, but the optimum must be measured on P100 itself.

The useful experiment is therefore not:

> “Does 128 always win?”

but:

> “Which `(hidden, FFN, heads, depth, dtype)` frontier gives the best quality/training-time/inference-time tradeoff on each available architecture?”

---

## 7. Recommended experimental program

### H0 — heterogeneous routing beats forced pooling

Compare:

1. independent GPU workers with a hardware-aware router;
2. tightly coupled heterogeneous model parallelism;
3. fastest-GPU-only baseline.

Measure latency, throughput, utilization, failure isolation, and energy.

### H1 — quantization optimum differs by GPU generation

For the same model, benchmark:

```text
FP16 / Q8 / Q6 / Q5 / Q4
x
RTX 3060 / P100 / CPU
x
short-context decode / long-context prefill
```

Expected result is **not** monotonic “fewer bits = faster”.

### H2 — native model shape should be GPU-specific

Hold approximate parameter budget constant while sweeping width, depth, FFN shape, and head dimension. Find Pareto frontiers separately for RTX 3060 and P100.

### H3 — Boolean/discrete routing can save expensive wakeups

Use the `learned-discrete-reflex-layer.md` benchmark plan and measure whether a CPU/FPGA/DLGN-style router reduces K-Edge/Persona-Core activations without increasing missed salient events.

### H4 — attention can be a scarce escalation resource

Prototype a cheap recurrent/stateful fast path whose ambiguous/high-value cases escalate to an attention-based semantic model. Measure quality against always-on full semantic inference.

---

## 8. Design rules to carry forward

1. **Do not homogenize heterogeneous hardware.** Give devices roles.
2. **Do not homogenize numerical precision.** Preserve precision where information is disproportionately valuable.
3. **Do not homogenize cognitive primitives.** Reflex, streaming state, association, and deliberation need not share an architecture.
4. **Do not assume low-bit inference is faster on old GPUs.** Benchmark the actual instruction set and runtime.
5. **Treat software support lifetime as a hardware property.** Legacy GPUs need a pinned legacy toolchain lane.
6. **Train/model-search against the deployment hardware.** Parameter count is not a throughput model.
7. **Keep canonical identity above the substrate.** Hardware and model backends remain replaceable organs.
8. **Prefer measured routing over ideological architecture choices.** The organism may use Boolean, FP16, attention, recurrence, CPU, GPU, FPGA, and remote models simultaneously.

---

## 9. Open questions

1. Does P100's HBM2 bandwidth make FP16/Q8 preferable to Q4 for any practical local inference workloads after dequantization overhead is included?
2. Can a 3060 handle latency-critical prefill/decode while P100 concurrently handles batch evaluation/training without causing CPU/PCIe contention?
3. Is there a useful pipeline split where an old high-VRAM GPU holds a specialist stage while a modern GPU performs tensor-core-heavy stages, or does synchronization erase the benefit?
4. Which operations can move entirely off GPU into K-Nerve/FPGA without harming semantic quality?
5. Can the scheduler learn per-device performance models online from execution receipts?
6. Should model lineage record the hardware architecture against which each generation was selected/trained?
7. How should a model evolve when the organism migrates from an old-GPU rack to a newer accelerator substrate?

The intended end state is not “Kamimusuhi requires a particular GPU”. It is an organism whose cognition **degrades gracefully, specializes opportunistically, and exploits whatever heterogeneous compute is actually available**.