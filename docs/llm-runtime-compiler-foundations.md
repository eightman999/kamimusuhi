# LLM Runtime, Compiler, and Inference Foundations

Status: **durable research / engineering note, non-normative**  
Captured: **2026-09-10**  
Scope: tokenizer → autoregressive inference → KV cache → prefill/decode → kernels → quantization → CUDA Graphs → speculative decoding → compiler/IR → heterogeneous placement.

Related Kamimusuhi documents:

- [`latency-architecture.md`](./latency-architecture.md)
- [`heterogeneous-cognitive-compute-substrate.md`](./heterogeneous-cognitive-compute-substrate.md)
- [`native-runtime-stratification.md`](./native-runtime-stratification.md)
- [`runtime-language-and-hardware-compatibility.md`](./runtime-language-and-hardware-compatibility.md)
- [`model-ecology-and-lineage.md`](./model-ecology-and-lineage.md)

This note distills a set of implementation articles supplied on 2026-09-10 and follows their references toward papers, official documentation, and source repositories. Secondary tutorials are useful for intuition and implementation anecdotes; they are **not** treated as stronger evidence than primary papers, official runtime/compiler documentation, or reproducible source code.

---

## 1. Executive thesis

For Kamimusuhi, local LLM inference should not be modeled as one opaque operation called `generate()`.

A more useful decomposition is:

```text
text / sensory-language material
        |
        v
  tokenizer / ids
        |
        v
+----------------------+   variable-length, large matrix work
| PREFILL PROGRAM      |   often compute / attention-I/O heavy
| prompt -> KV state   |
+----------+-----------+
           |
           v
+----------------------+   q_len = 1 in ordinary autoregressive decode
| DECODE PROGRAM       |   often memory-bandwidth + launch-overhead heavy
| token + KV -> token  |
+----------+-----------+
           |
           +--> optional speculative draft/verify schedule
           |
           v
   streaming expression
```

The durable engineering rule is:

> **Treat prefill and decode as different programs with different shapes, bottlenecks, kernels, compilation strategies, caches, and benchmark metrics.**

This single distinction explains why an optimization can help prompt ingestion but not token generation, why a generic GEMM library is not automatically optimal for batch-1 decode, why static KV storage can unlock graph replay, and why a “faster” quantization scheme can regress if it adds unfused memory traffic.

---

## 2. Evidence levels used here

### Primary / high-confidence engineering evidence

- PyTorch official GPT Fast engineering write-up and `gpt-fast` source.
- PyTorch official CUDA Graphs documentation/blog.
- FlashAttention-2 paper.
- original Transformer paper.
- `tiktoken` source/README for BPE implementation properties.
- speculative decoding / speculative sampling papers.
- DDTree and DSpark papers for 2026 speculative-decoding variants.
- MLIR official overview/tutorial and LLVM source tree.

### Secondary / useful implementation evidence

- the supplied Zenn articles and Medium explainers;
- individual benchmark results on one GPU/model/runtime;
- educational from-scratch Llama/GPT explanations.

A secondary benchmark may motivate an experiment, but it must not become a universal architecture claim.

---

## 3. Tokenization is part of model-and-hardware co-design

Tokenization is not merely a UI preprocessing step. It sets the sequence seen by the model and therefore affects:

- prompt token count;
- KV-cache growth;
- prefill work;
- decode context length;
- effective context capacity in characters/bytes;
- embedding/output-head size when designing a model from scratch;
- training throughput and artifact size;
- compatibility between target and draft models in speculative decoding.

`tiktoken` documents a BPE tokenizer that is reversible/lossless over arbitrary text and compresses common byte sequences into tokens. Simon Willison's 2023 tokenizer explainer is useful historical intuition for how strongly token density can vary across text and languages, but concrete token counts must always be recomputed with the **actual tokenizer/version** used by the model.

For `novllm` / future Persona Core tokenizer work, vocabulary size is therefore an end-to-end optimization variable:

```text
larger vocabulary
  -> often fewer sequence positions
  -> larger embedding / LM head
  -> different memory traffic / parameter count
  -> different training and inference optimum
```

### Kamimusuhi experiment

For every candidate tokenizer, record at minimum:

```yaml
corpus_slices:
  - Japanese conversation
  - Japanese technical text
  - English
  - code
  - metadata / tool protocol
metrics:
  bytes_per_token
  chars_per_token
  p50_p95_sequence_tokens
  embedding_head_parameters
  model_train_chars_per_second
  inference_chars_per_second
```

Do not select a tokenizer from tokenizer statistics alone.

Primary/implementation references:

- https://github.com/openai/tiktoken
- https://simonwillison.net/2023/Jun/8/gpt-tokenizers/

---

## 4. Autoregressive decoder mental model

The Illustrated GPT-2 remains a useful visual explanation of decoder-only autoregressive generation: a token is produced, appended, and the model runs again under causal masking. The original Transformer paper remains the primary architectural root for scaled dot-product attention.

For runtime engineering, the important state transition is roughly:

```text
prefill(prompt_tokens)
    -> first_distribution + KV_state

decode(last_token, KV_state)
    -> next_distribution + extended_KV_state
```

The Q/K/V distinction matters operationally. For a future token's query, **past keys and values are needed again**, while old queries were used to produce old outputs and normally need not be retained. This is why the inference cache is a K/V cache rather than a Q/K/V history.

For Llama-family educational implementation, the supplied from-scratch article is useful for locating major moving parts such as RMSNorm, RoPE, SwiGLU, attention, and generation without hiding them behind a framework. It is best treated as a learning aid; model-family truth should come from the model configuration/source being implemented.

References:

- https://jalammar.github.io/illustrated-gpt2/
- https://arxiv.org/abs/1706.03762
- https://zenn.dev/fusic/articles/fd6fbe8a5e966d

---

## 5. Prefill and decode are different execution regimes

The supplied custom inference-engine write-up independently reaches the same practical split emphasized by PyTorch GPT Fast.

### Prefill

Typical properties:

- many prompt tokens are processed together;
- large GEMMs are common;
- attention over a nontrivial query length matters;
- variable prompt length makes shapes/control more dynamic;
- chunked prefill can bound working shapes and latency spikes;
- compute throughput and attention I/O are often important limits.

### Decode

Typical batch-1 autoregressive properties:

- `q_len = 1`;
- linear layers become GEMV-like rather than large GEMM-like;
- model weights are repeatedly streamed for each generated token;
- memory bandwidth often dominates arithmetic throughput;
- many small kernel launches can make CPU/driver overhead visible;
- the logical KV length grows each step even if physical allocation is static.

This means **one generic kernel choice is not a sufficient inference strategy**.

A runtime/compiler specialization key should be allowed to include:

```text
phase = prefill | decode
batch class
query-length bucket
KV-capacity bucket / logical length
model architecture
weight dtype / quantization
activation dtype
head configuration
GPU architecture / backend
```

References:

- https://zenn.dev/jame443/articles/2445290e2a5040
- https://pytorch.org/blog/accelerating-generative-ai-2/
- https://github.com/meta-pytorch/gpt-fast

---

## 6. KV cache is both an algorithmic optimization and a compiler contract

KV caching prevents recomputation of past token K/V projections, but its **memory layout and allocation policy** also determine which compiler/runtime optimizations are available.

A useful distinction is:

```text
physical capacity: allocated buffer size / address
logical length:    number of valid cached positions
```

PyTorch GPT Fast uses a static KV cache: allocate a maximum-capacity buffer and mask/use only the logical prefix. The major benefit is not only avoiding repeated reallocation/copying; it makes decode substantially more static and therefore friendlier to compilation and CUDA Graph replay.

Trade-off:

```text
static KV
  + stable addresses/shapes
  + graph/compile friendliness
  + no grow/reallocate loop
  - reserves memory up front
  - can waste VRAM at short contexts

paged / rotating / dynamic schemes
  + potentially better capacity utilization / long-context serving
  - different scheduling and graph-capture constraints
```

Kamimusuhi should not mandate one cache layout globally. K-Edge single-user low-latency inference may prefer a different cache policy than high-concurrency serving or a VRAM-starved legacy GPU.

---

## 7. CUDA Graphs: eliminate repeated launch orchestration when the graph is stable

CUDA Graphs record a sequence of GPU work and replay it with the same kernels/arguments and stable pointer addresses. PyTorch's official explanation emphasizes the trade:

```text
less runtime dynamism
        in exchange for
much lower CPU / driver launch overhead
```

This is especially attractive for decode because its per-token program can become nearly static when the KV buffer is physically static.

Graph capture is not free:

- shapes/control flow must be graph-safe enough;
- pointer/address stability matters;
- warmup/capture cost must be amortized;
- multiple shape buckets may be required;
- a dynamic prefill path may need a separate compilation strategy.

### Required benchmark additions

Do not report only `tok/s`. Record:

```yaml
cpu_submission_time_ms: ...
gpu_idle_gap_ms: ...
graph_capture_ms: ...
graph_replay_ms: ...
steps_to_amortize_capture: ...
static_kv_reserved_bytes: ...
```

References:

- https://pytorch.org/blog/accelerating-pytorch-with-cuda-graphs/
- https://zenn.dev/kosuke_sugimoto/articles/1614e65b27f6b4
- https://pytorch.org/blog/accelerating-generative-ai-2/

The large speedups reported in toy or A100 benchmarks are **workload-specific evidence**, not a promise for every Kamimusuhi device.

---

## 8. Quantization only wins if the runtime preserves the bandwidth advantage

Reducing weight precision can reduce model footprint and, especially in memory-bound decode, reduce bytes moved per generated token. But a naive quantized graph can add work such as:

```text
activation quantize
  -> temporary write
  -> int8 matmul
  -> temporary write
  -> dequantize / rescale
```

If these become separate kernels, intermediate memory traffic and launch overhead can erase part of the theoretical gain.

The durable rule is:

> **Quantization format, kernel fusion, supported tensor/integer units, and decode memory traffic must be benchmarked as one package.**

Do not reason from “INT8 is 2× smaller than FP16” directly to “INT8 inference is 2× faster.”

PyTorch GPT Fast similarly demonstrates that quantization can regress before compiler fusion and improve after the compiler fuses the relevant path. The custom-engine article arrives at the same practical lesson from W8A8 kernel implementation.

For every quantized backend record:

```yaml
weight_bytes_per_token_step: ...
intermediate_bytes_estimate: ...
kernel_count_per_decode_step: ...
quant_dequant_fused: true|false
tensor_core_or_native_int_path: ...
prefill_tok_s: ...
decode_tok_s: ...
quality_delta: ...
```

References:

- https://zenn.dev/jame443/articles/2445290e2a5040
- https://pytorch.org/blog/accelerating-generative-ai-2/

---

## 9. Attention kernels: exactness and phase-specific usefulness

FlashAttention-2 is an **exact attention algorithm**, not an approximation. It improves attention execution by reducing memory traffic and improving parallel work partitioning compared with naive attention / earlier FlashAttention implementations.

Its relevance is strongest when attention is a meaningful bottleneck, commonly long-sequence training and prefill-like workloads. For batch-1 `q_len=1` decode, the optimal kernel regime may differ. PyTorch GPT Fast reports a special decode compilation regime where generated kernels can outperform general-purpose library alternatives in that benchmark.

Kamimusuhi should therefore benchmark attention separately by phase:

```text
prefill_short
prefill_long
single-token_decode_short_KV
auto-regressive_decode_long_KV
```

Reference:

- https://arxiv.org/abs/2307.08691
- https://zenn.dev/kaeru39/articles/1ea73bfa40c7df  # useful local anecdote only

---

## 10. Speculative decoding changes the serial schedule

Ordinary autoregressive generation pays one expensive target-model step for each new token. Canonical speculative decoding introduces a cheap draft path that proposes several future tokens, then lets the target score/verify them in a more parallel step.

The foundational speculative-decoding/sampling papers are especially important because the canonical acceptance/rejection procedure can preserve the **target model's output distribution exactly**. This “no distribution change” property applies to the mathematically defined algorithm; it must not be casually generalized to every heuristic multi-token predictor, tree decoder, or approximate serving mode.

Primary references:

- https://arxiv.org/abs/2211.17192
- https://arxiv.org/abs/2302.01318
- https://aclanthology.org/2023.findings-emnlp.257/

### Performance is a three-way trade

At minimum:

```text
draft cost
x acceptance rate
x useful accepted/emitted tokens per target verification
```

Longer draft branches are not automatically faster. They increase opportunity to emit more tokens per target call, but also increase draft cost and the probability that later draft tokens are rejected.

ELYZA's published vLLM experiment is a useful concrete example: in its specific environment, baseline generation was 18.90 TPS and the best tested draft setup reached 32.25 TPS; the best speculative-token count was not simply the largest tested value. Treat this as evidence that **branch length is a tuning parameter**, not as a transferable speed figure.

Reference:

- https://zenn.dev/elyza/articles/4e0b45a8c11220

### Kamimusuhi-specific hypothesis: same-lineage draft models

Kamimusuhi's model ecology creates an interesting research direction:

```text
small Persona Core checkpoint / organ
          drafts
             |
             v
larger same-lineage Persona Core
          verifies
```

A shared tokenizer, data lineage, persona priors, and related architecture may improve draft alignment relative to an unrelated tiny model. This is a **hypothesis** and must be measured by acceptance rate and end-to-end latency, not assumed.

Required speculative benchmark record:

```yaml
target_model: ...
draft_method: model | ngram | mtp | tree | diffusion | other
draft_model: ...
shared_tokenizer: true|false
speculative_tokens_or_node_budget: ...
draft_ms: ...
verify_ms: ...
acceptance_rate: ...
accepted_tokens_per_verify: ...
ttft_ms: ...
decode_tok_s: ...
peak_vram: ...
quality_or_distribution_contract: exact_target | heuristic | unknown
```

---

## 11. 2026 speculative-decoding directions: DDTree and DSpark

The supplied 2026 overview links to newer work that should be kept as a watchlist rather than promoted to baseline architecture.

### DDTree

`Accelerating Speculative Decoding with Block Diffusion Draft Trees` (2026) builds on block-diffusion drafting. The paper describes constructing a draft tree under a node budget and letting the target verify candidate structure in parallel. This broadens the design space beyond a single linear draft sequence.

- https://arxiv.org/abs/2604.12989

### DSpark

`DSpark: Confidence-Scheduled Speculative Decoding with Semi-Autoregressive Generation` (2026) combines semi-autoregressive draft generation with adaptive, load-aware verification. Its motivation is directly relevant to serving: blindly verifying long low-confidence blocks can waste batch capacity under concurrency.

- https://arxiv.org/abs/2607.05147

The production speedups stated by such papers are **author-reported, hardware/traffic/model dependent**. Kamimusuhi should copy the scheduling ideas only after reproducing them on its own target workload.

Secondary overview:

- https://zenn.dev/katalab/articles/386df3380c2888
- https://zenn.dev/u_hyszk/articles/7bf9c6fc4dd154
- https://medium.com/@TitanML/in-the-fast-lane-speculative-decoding-10x-larger-model-no-extra-cost-f33ea39d065a

---

## 12. Compiler perspective: inference optimization is staged lowering

The supplied compiler articles expose two useful extremes.

### Direct PTX generation

A hobby language/compiler can generate PTX and launch NVIDIA GPU work directly. This is valuable proof that the full path from custom language semantics to GPU execution can be owned by a small project.

However, direct PTX ownership also means owning more of:

- NVIDIA-specific ABI/runtime details;
- address spaces and memory movement;
- kernel launch conventions;
- target-specific tuning;
- backend portability burden.

References:

- https://zenn.dev/spica314/articles/3e44764ec17433
- https://github.com/spica314/felis-lang

### MLIR as reusable multi-level compiler infrastructure

MLIR is designed to reduce the cost of domain-specific compiler construction and connect multiple abstraction levels/backends. Its dialect/lowering model allows domain semantics to survive long enough for relevant transformations before eventually lowering toward lower-level GPU/LLVM representations.

That makes it a more plausible research substrate if Kamimusuhi eventually owns **specialized cognitive/inference kernels** rather than merely calling mature backends.

References:

- https://mlir.llvm.org/
- https://mlir.llvm.org/docs/Tutorials/Toy/
- https://github.com/llvm/llvm-project/tree/main/mlir/examples/toy
- https://zenn.dev/lemolatoon3/articles/self-made-lang-run-on-gpu

### Kamimusuhi rule

Do **not** create a custom compiler because compilers are interesting.

Only introduce a Kamimusuhi-specific IR/dialect when a stable repeated optimization problem exists, for example:

```text
Persona decode kernel family
quantize + projection + rescale fusion
K-Fast / discrete-organ lowering
device-capability-specialized leaf kernels
fixed metadata/action protocol parsing kernels
```

Until then:

```text
canonical architecture contracts
        |
        v
native runtime ABI / resource interface
        |
        +--> llama.cpp / MLX / PyTorch / vLLM
        +--> CUDA / Metal / Vulkan leaf kernels
        +--> optional compiler experiment
```

The inference backend must remain replaceable.

---

## 13. Runtime optimization ladder: profile by bottleneck, not fashion

A practical order for a single-user local model is:

```text
0. establish correctness + reproducible baseline
1. split prefill/decode metrics
2. inspect trace: compute? memory bandwidth? launch overhead? cache? transfer?
3. keep model/context warm
4. eliminate avoidable realloc/copies and CPU launch gaps
5. specialize/fuse kernels for the measured phase
6. reduce bytes moved with a quantization the backend executes efficiently
7. optimize attention where attention is actually dominant
8. add speculative decoding if target/draft economics work
9. add offload / heterogeneous placement only when memory capacity requires it
10. own custom compiler lowering only for a demonstrated recurring kernel problem
```

A benchmark that only reports average generation `tok/s` is insufficient to choose among these steps.

---

## 14. VRAM overflow: capacity and speed are separate objectives

When weights + KV + workspace do not fit on one accelerator, the runtime may need CPU/GPU or multi-device placement. The custom inference-engine article explicitly identifies CPU↔GPU transfer/overlap as a next step.

Possible strategies:

```text
A. all resident on one GPU
B. layer/block offload with CPU-resident weights
C. stream upcoming weights while current layer computes
D. task-level split across heterogeneous devices
E. remote escalation to another node
```

But PCIe/host transfer can become the dominant bottleneck. Therefore “it runs” and “it is a useful cognitive organ” are separate acceptance criteria.

For old heterogeneous hardware, Kamimusuhi should continue preferring **task-level decomposition** over pretending mismatched GPUs are one symmetric tensor-parallel pool.

Measure:

```yaml
resident_weight_bytes: ...
kv_bytes: ...
host_to_device_gbps: ...
device_to_host_gbps: ...
transfer_overlap_percent: ...
compute_waiting_for_transfer_ms: ...
end_to_end_decode_tok_s: ...
energy_or_power: optional
```

---

## 15. ResourceProfile additions for Kamimusuhi

The Cognitive Resource Router should eventually know runtime/compiler capabilities, not just “GPU model” and nominal VRAM.

Extend the research-level `ResourceProfile` concept with dimensions such as:

```text
memory_bandwidth
host_link_bandwidth
kernel_launch_overhead
compiler_backend
cuda_graph_or_equivalent_support
graph_safe_shape_classes
static_kv_supported
paged_kv_supported
flash_attention_variants
native_int8_int4_paths
tensor_core_matrix_modes
supported_quant_formats
prefill_throughput_curve
decode_throughput_curve
speculative_modes
model_load_cost
```

This lets the router ask a meaningful question:

> Which resource is fastest **for this phase, shape, context, precision, and latency deadline**?

rather than:

> Which GPU has the highest theoretical FLOPS?

---

## 16. Benchmark plan for Kamimusuhi / limit-GPU systems

### R0 — tokenizer end-to-end sweep

Measure Japanese/English/code token density **and** resulting model parameter/throughput effects.

### R1 — prefill/decode roofline characterization

On RTX 3060, P100, Apple Silicon, and any candidate legacy GPU:

```text
prefill tok/s vs prompt length
decode tok/s vs KV length
measured memory bandwidth utilization
GPU utilization
CPU launch gaps
```

### R2 — static KV + graph replay

Compare eager/dynamic cache vs static cache + CUDA Graph/compiled decode. Record VRAM cost and graph-capture amortization.

### R3 — quantization fusion

Compare at least:

```text
FP16/BF16 baseline
weight-only quantized
W8A8 or backend-equivalent unfused
same quantized path fused/compiled
```

Do not compare formats the hardware/runtime cannot execute natively enough to be meaningful.

### R4 — phase-specific attention

Compare attention backends separately on long prefill and single-token decode.

### R5 — same-lineage speculative decoding

Use a small Persona Core / novllm checkpoint as draft for a related larger checkpoint. Sweep speculative length and record acceptance, VRAM, and latency.

### R6 — VRAM oversubscription / overlap

Measure CPU↔GPU offload with and without transfer/compute overlap. Stop if PCIe traffic makes it operationally worse than a smaller resident model.

### R7 — compiler pilot

Only after R1–R4 isolate a repeatable leaf bottleneck, implement one kernel through either direct CUDA/PTX or an MLIR-based path and compare:

```text
correctness parity
kernel count
bytes moved
latency
portability cost
maintenance complexity
```

A failed pilot is useful evidence against owning a custom compiler layer.

---

## 17. Consequences for Persona Core design

These runtime findings affect native-model research, but do not dictate the identity architecture.

### Model shape is a deployment variable

When searching Persona Core architectures, evaluate:

```text
quality
x tokenizer efficiency
x prefill latency
x decode latency
x KV footprint
x quantizability
x compiler/kernel availability
x draft-model compatibility
```

Two models with similar parameter counts can behave very differently on the same hardware.

### Keep identity above the backend

Optimizations such as:

- replacing a CUDA kernel;
- switching quantization;
- enabling graph replay;
- changing draft model;
- moving inference to another GPU;

must not mutate canonical identity/continuity semantics. They are substrate changes beneath the Persona Core contract.

### Same-lineage small/large checkpoints are more than fallback tiers

They may serve three roles:

```text
small checkpoint
  -> always-warm K-Edge mind
  -> speculative drafter for a larger relative
  -> degraded-mode continuity vessel

large checkpoint
  -> K-Core / deeper local cognition
  -> verifier / escalated reasoning vessel
```

Whether one model can perform all three roles efficiently is an experiment, not a requirement.

---

## 18. Failure modes to guard against

1. **Tok/s monoculture** — optimizing decode throughput while TTFT, voice onset, or queueing worsens.
2. **Prefill/decode conflation** — choosing one kernel/compiler policy for both phases.
3. **Quantization by bit-width alone** — ignoring kernel fusion and hardware support.
4. **Static-cache correctness bugs** — stale positions, wrong masks, cross-session leakage.
5. **Graph-capture state bugs** — replaying with invalid addresses/state or silently stale buffers.
6. **Speculative folklore** — assuming more draft tokens or a larger draft model always helps.
7. **Exactness overclaim** — applying canonical speculative-sampling guarantees to heuristic MTP/tree methods without proof.
8. **Compiler hobbyism becoming architecture** — binding Kamimusuhi to PTX/MLIR without a measured recurring need.
9. **Offload success mistaken for useful speed** — a model fitting by PCIe streaming but becoming unusably slow.
10. **Hardware peak-FLOPS routing** — ignoring decode memory bandwidth and launch behavior.

---

## 19. Source trail from the supplied reading list

The following records the intake source and the stronger/primary material followed from it where useful.

| Supplied source | Distilled use | Followed source(s) |
|---|---|---|
| https://zenn.dev/jame443/articles/2445290e2a5040 | custom LLM compiler/runtime; W8A8 fusion; separate prefill/decode; fixed shapes; decode GEMV | FlashAttention-2; source repo linked by article |
| https://zenn.dev/fusic/articles/fd6fbe8a5e966d | from-scratch Llama mental model | model architecture should still be checked against actual config/source |
| https://zenn.dev/kosuke_sugimoto/articles/1614e65b27f6b4 | CUDA Graph intuition and local benchmark | PyTorch CUDA Graphs official write-up |
| https://zenn.dev/umeko/articles/fe961fda3148d1 | PyTorch LLM optimization survey | PyTorch GPT Fast + `gpt-fast` |
| https://zenn.dev/spica314/articles/3e44764ec17433 | proof of custom language → PTX → GPU path | https://github.com/spica314/felis-lang |
| https://zenn.dev/lemolatoon3/articles/self-made-lang-run-on-gpu | custom language with MLIR GPU lowering | MLIR official overview, Toy tutorial, LLVM source |
| https://simonwillison.net/2023/Jun/8/gpt-tokenizers/ | tokenizer intuition / language-dependent density | https://github.com/openai/tiktoken |
| https://jalammar.github.io/illustrated-gpt2/ | decoder-only autoregressive visual model | https://arxiv.org/abs/1706.03762 |
| https://medium.com/@joaolages/kv-caching-explained-276520203249 | KV-cache intuition | PyTorch GPT Fast/static KV used for durable engineering claims |
| https://medium.com/@TitanML/in-the-fast-lane-speculative-decoding-10x-larger-model-no-extra-cost-f33ea39d065a | speculative-decoding intuition | Leviathan/Chen/EMNLP papers |
| https://zenn.dev/elyza/articles/4e0b45a8c11220 | measured draft/acceptance/branch-length tradeoff | speculative decoding/sampling papers |
| https://zenn.dev/kaeru39/articles/1ea73bfa40c7df | local optimization comparison anecdote | FlashAttention/PyTorch evidence preferred for general claims |
| https://zenn.dev/u_hyszk/articles/7bf9c6fc4dd154 | explanation of quality-preserving speculative sampling | Leviathan + Chen papers |
| https://zenn.dev/katalab/articles/386df3380c2888 | 2026 MTP/DFlash/DDTree/DSpark overview | DDTree + DSpark papers |

Core primary links followed:

- PyTorch GPT Fast: https://pytorch.org/blog/accelerating-generative-ai-2/
- gpt-fast: https://github.com/meta-pytorch/gpt-fast
- CUDA Graphs: https://pytorch.org/blog/accelerating-pytorch-with-cuda-graphs/
- FlashAttention-2: https://arxiv.org/abs/2307.08691
- Transformer: https://arxiv.org/abs/1706.03762
- tiktoken: https://github.com/openai/tiktoken
- Speculative Decoding: https://arxiv.org/abs/2211.17192
- Speculative Sampling: https://arxiv.org/abs/2302.01318
- Seq2seq speculative decoding: https://aclanthology.org/2023.findings-emnlp.257/
- DDTree: https://arxiv.org/abs/2604.12989
- DSpark: https://arxiv.org/abs/2607.05147
- MLIR: https://mlir.llvm.org/
- MLIR Toy: https://mlir.llvm.org/docs/Tutorials/Toy/

---

## 20. Promotion criteria

Promote any optimization in this note into normative Kamimusuhi architecture only after:

1. correctness parity is demonstrated where parity is expected;
2. the target hardware/runtime/model combination is stated;
3. prefill and decode are measured separately;
4. TTFT / first-action latency is measured in addition to throughput;
5. peak and reserved memory are recorded;
6. failure/fallback behavior is understood;
7. identity, continuity, privacy, and authority boundaries remain unchanged;
8. the gain survives an end-to-end Kamimusuhi workload rather than only a microbenchmark.

The intended outcome is not “Kamimusuhi owns an AI compiler.” The intended outcome is a system that **understands enough of its inference substrate to route, compile, cache, quantize, and speculate only where measurements show that doing so improves the lived latency of the same persistent individual.**
