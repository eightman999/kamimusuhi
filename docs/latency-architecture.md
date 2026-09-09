# Low-Latency Cognition Architecture

Status: **living engineering design note**

Kamimusuhi should feel immediately responsive on existing hardware even when deep cognition requires seconds or longer. The primary performance goal is therefore not maximum raw token throughput. It is **perceived cognitive latency**: how quickly the organism reacts, begins speaking/acting, and escalates cognition when needed.

See Issues #9 and #12 for reflex/background cognition and Cognitive Budget.

## 1. Latency thesis

Do not make every stimulus wait for maximum intelligence.

```text
input
  -> reflex / interrupt          very fast
  -> K-Edge Persona Core        fast
  -> K-Core deliberate thought  deeper
  -> tools / local specialists  variable
  -> frontier/external models   variable
  -> K-Deep                     expensive
```

A difficult task may take time to complete while still receiving an immediate authentic response from the same individual.

The response need not be a fake filler sentence. K-Edge can genuinely report that it has escalated the task to deeper cognition.

## 2. Proposed latency classes

These are target SLO classes, not guarantees independent of hardware.

```text
Reflex/event reaction          ~20-100 ms class
K-Edge first response          ~100-300 ms class where practical
K-Core response start          sub-second to low-seconds class
K-Deep/research completion     seconds to minutes as justified
Sleep/dream/consolidation      non-real-time
```

Audio interaction should optimize **speech-response onset**, not wait for a complete paragraph.

## 3. Metrics

Primary metrics:

- sensory-event latency;
- reflex latency;
- TTFT / first-action latency;
- first-audio latency for voice;
- K-Edge decision latency;
- cognitive escalation latency;
- memory-prefetch hit rate;
- warm-context ratio;
- prefix-cache hit rate;
- time spent waiting for external cognition;
- time-to-useful-first-response;
- final completion latency.

Secondary metrics:

- decode tokens/s;
- prefill tokens/s;
- memory bandwidth;
- energy/cost per cognitive episode.

Raw tokens/s alone is not a sufficient user-experience metric.

## 4. Always-warm cognition

Persona-related models should avoid cold loading in ordinary interaction.

Suggested residency:

```text
always resident
  K-Edge Persona Core
  reflex/salience processors

warm / reserved capacity
  K-Core Persona Core or deeper local cognition

on demand
  large specialists
  K-Deep
  frontier APIs
```

A question should not trigger a multi-second model-load path before any response can begin.

## 5. Prompt/KV caching

Long-lived personality systems are especially vulnerable to repeated prefill cost because self-state, tool contracts, and relationship context recur across turns.

Partition model input into:

```text
STATIC / HIGH-CACHE
  cognitive protocol
  stable Persona Core constitution
  stable tool/action schema

SEMI-STATIC
  current self summary
  relationship/common-ground state
  active goals

DYNAMIC
  new sensory events
  retrieved episodic memories
  new library evidence
  current user input
```

The most reusable material should appear in a stable prefix whenever the backend cache model supports prefix reuse.

Current implementation references:

- llama.cpp `cache_prompt` reuses KV cache for a common request prefix: https://github.com/ggml-org/llama.cpp/blob/master/tools/server/server-schema.cpp
- MLX-LM supports prompt caching, rotating KV caches, and configurable prefill step size: https://github.com/ml-explore/mlx-lm

MLX-LM can save/load prompt caches across requests, which is useful for persistent local dialogue on Apple Silicon.

## 6. Progressive / speculative cognition before turn end

Voice interaction permits useful work before the speaker has completed the utterance.

```text
microphone
  -> streaming VAD/ASR
  -> partial semantic hypotheses
  -> entity/topic recognition
  -> memory prefetch
  -> library/cache prefetch
  -> likely cognitive-budget classification
  -> final utterance arrives
  -> Persona Core starts with warm context
```

Do not commit actions from unstable partial ASR, but use partial input for reversible prefetch and preparation.

The same principle applies to GUI/vision streams: changes can begin retrieval or salience processing before an explicit question arrives.

## 7. Parallel retrieval

Memory preparation should not be serialized behind one LLM call.

```text
input
 ├-> identity/entity lookup
 ├-> lexical episodic retrieval
 ├-> semantic episodic retrieval
 ├-> relationship/common-ground retrieval
 ├-> library retrieval
 └-> current resource/health lookup
```

Results may arrive progressively. The workspace should distinguish which evidence was present at each cognitive step.

## 8. Adaptive deliberation depth

A fixed amount of reasoning for every task wastes latency and energy.

Conceptual levels:

```text
reflex       0 deliberate loops
casual       shallow
normal       moderate
complex      deeper / iterative
deep         high compute + external resources
research     long-running tool/retrieval/model composition
```

The concrete implementation may initially be multiple explicit runtime passes rather than a recurrent neural architecture. Later Persona Core research can test adaptive-depth or recurrent models.

The model/runtime should be able to ask for `THINK_MORE` rather than generating a long visible chain-of-thought.

## 9. External cognition and immediate response

External-model use should not block all expression.

```text
K-Edge
  -> recognizes hard task
  -> communicates/acts immediately if appropriate
  -> delegates attributed subtask
  -> receives cognitive material
  -> Persona Core integrates
  -> continues response/action
```

This preserves identity continuity while taking advantage of slower/stronger resources.

## 10. Speculative decoding

When output generation itself becomes the bottleneck, speculative decoding can improve throughput by letting a fast draft path predict tokens that a target model verifies in batches.

llama.cpp currently documents several speculative modes, including draft models, EAGLE-3, MTP, DFlash/DSpark, and n-gram methods:

- https://github.com/ggml-org/llama.cpp/blob/master/docs/speculative.md

Possible Kamimusuhi research directions:

- tiny Persona Core as draft for a related larger Persona Core;
- same-lineage small/large checkpoints to improve draft acceptance;
- MTP-capable native Persona Core;
- n-gram speculation for repetitive structured cognitive-action formats.

Speculative decoding must be benchmarked per model/hardware. It is not guaranteed to improve latency when draft overhead or acceptance is poor.

## 11. Output streaming

Response generation should stream to the appropriate actuator:

- text token/phrase streaming;
- low-latency TTS chunking;
- incremental gesture/animation planning;
- interruptibility when new salient input arrives.

An organism that cannot be interrupted until a 500-token response completes will feel less alive regardless of average throughput.

## 12. Fast path vs deep path

A practical v1 architecture:

```text
               input/event
                    │
              Reflex Gate
               /       \
          reflex       cognitive
            │              │
         action          K-Edge
                           │
                    easy? / \ hard?
                        /     \
                   respond   escalate
                               │
                     ┌─────────┼─────────┐
                     │         │         │
                   K-Core    tools    models
                     │         │         │
                     └──── cognitive material
                               │
                           Persona Core
```

This gives existing hardware a feasible path to perceived immediacy without requiring the largest model to be fast enough for every turn.

## 13. Cache correctness and observability

Performance optimizations can introduce state/correctness bugs. Every cache optimization needs parity tests.

Required tests:

- cached vs uncached output distribution/greedy parity where applicable;
- cache invalidation after Persona Core version change;
- cache invalidation after tool/cognitive-protocol schema change;
- self-state revision invalidation;
- stale K-Edge cache detection;
- cross-session leakage tests;
- memory/privacy boundary tests.

Trace cache state and hit/miss reason without logging private raw payloads unnecessarily.

## 14. Benchmark matrix

Every latency benchmark should report enough hardware/runtime context to be reproducible:

```yaml
hardware: ...
backend: llama.cpp | mlx-lm | other
model: ...
quantization: ...
context_tokens: ...
cached_prefix_tokens: ...
dynamic_tokens: ...
ttft_ms: ...
prefill_tok_s: ...
decode_tok_s: ...
escalation_ms: ...
energy_or_power: optional
```

Benchmark at least:

- cold start;
- warm model / cold context;
- warm prefix cache;
- multi-turn chat;
- memory retrieval enabled;
- K-Edge -> K-Core escalation;
- external resource delegation.

## 15. Design rule

> The system should spend deep intelligence where deep intelligence matters, not where a fast reflex or already-warm local mind is sufficient.

## 16. Prefill and decode are different latency programs

The inference backend should expose prefill and decode as distinct measured phases rather than one `generate` black box.

For ordinary batch-1 autoregressive inference:

```text
PREFILL
  many query positions
  large GEMMs / attention
  variable prompt length
  often compute or attention-I/O heavy

DECODE
  q_len = 1
  GEMV-like projections
  weights repeatedly streamed per token
  often memory-bandwidth and launch-overhead heavy
```

This distinction is supported both by custom-engine experience and PyTorch's GPT Fast work. It explains why separate generated/compiled paths are rational even when both phases execute the same model weights.

Kamimusuhi should therefore allow phase-specific:

- kernels;
- compile flags / graphs;
- sequence-length buckets;
- attention implementations;
- quantization choices where a backend supports them;
- device routing.

See [`llm-runtime-compiler-foundations.md`](./llm-runtime-compiler-foundations.md) for the evidence trail.

## 17. Static KV cache and graph-replay contract

A growing **logical** context does not require a growing physical allocation. A backend may reserve a fixed KV-capacity buffer and track the valid logical length separately.

This can trade VRAM for lower orchestration cost:

```text
static physical KV addresses
        +
stable decode shapes/control
        ->
CUDA Graph / compiled replay eligibility
        ->
lower CPU/driver launch overhead
```

CUDA Graph replay uses the same recorded kernels and arguments, including stable pointer addresses. Therefore graph capture is an execution contract, not a magical global switch.

Benchmark both the benefit and cost:

- graph capture/warmup time;
- steps needed to amortize capture;
- CPU submission gaps;
- GPU idle gaps;
- reserved KV memory;
- eager vs replay TTFT/decode latency;
- correctness and invalidation behavior.

Variable-length prefill should be allowed to use a separate dynamic/chunked compilation strategy rather than forcing decode's static assumptions onto it.

References:

- https://pytorch.org/blog/accelerating-pytorch-with-cuda-graphs/
- https://pytorch.org/blog/accelerating-generative-ai-2/

## 18. Optimize bytes and launches, not just arithmetic

Batch-1 decode can be limited by how many bytes are moved and how many kernels are launched, not by peak FLOPS.

Consequences:

1. Weight-only or low-bit quantization can be especially useful when it genuinely reduces memory traffic.
2. Quantize / GEMM-GEMV / dequantize implemented as separate kernels may reintroduce memory traffic and launch overhead.
3. Kernel fusion is part of the quantization design, not a later cosmetic optimization.
4. A general-purpose GEMM/attention implementation that wins at prefill may lose at `q_len=1` decode.
5. CPU launch overhead should be visible in traces before rewriting unrelated high-level code.

Add to inference benchmark records where practical:

```yaml
kernel_count_per_decode_step: ...
cpu_submission_ms: ...
gpu_idle_gap_ms: ...
quant_dequant_fused: true|false
estimated_or_measured_memory_bandwidth: ...
```

## 19. Speculative decoding benchmark contract

Canonical speculative decoding/sampling can preserve the target distribution when the defined acceptance/rejection algorithm is used. That exactness guarantee must not be automatically attributed to every MTP, tree, diffusion, or heuristic speculative method.

Performance is controlled by at least:

```text
draft cost
acceptance rate
accepted/emitted tokens per verification
verification cost
memory footprint
```

The speculative branch length is therefore a tunable parameter, not a monotonic “higher is faster” setting. ELYZA's published vLLM experiment is one concrete example where an intermediate speculative-token count outperformed longer tested values.

Kamimusuhi's same-lineage small/large Persona Core checkpoints are a particularly relevant hypothesis: shared tokenizer/data/persona lineage may improve draft alignment, but this must be demonstrated by acceptance rate and end-to-end latency.

Required record:

```yaml
target_model: ...
draft_method: ...
draft_model: ...
speculative_length_or_node_budget: ...
draft_ms: ...
verify_ms: ...
acceptance_rate: ...
accepted_tokens_per_verify: ...
decode_tok_s: ...
peak_vram: ...
distribution_contract: exact_target | heuristic | unknown
```

References:

- https://arxiv.org/abs/2211.17192
- https://arxiv.org/abs/2302.01318
- https://zenn.dev/elyza/articles/4e0b45a8c11220

Newer 2026 methods such as DDTree and DSpark belong on the research watchlist; their reported gains are workload-specific until reproduced locally.

## 20. Runtime evidence note

The broader source trail, including tokenizer foundations, FlashAttention-2, PyTorch GPT Fast, CUDA Graphs, quantization/fusion, speculative decoding, DDTree/DSpark, direct PTX generation, and MLIR lowering, is maintained in:

- [`llm-runtime-compiler-foundations.md`](./llm-runtime-compiler-foundations.md)

Performance changes should be promoted from that research note into this design only when they improve an end-to-end Kamimusuhi latency objective on a stated hardware/model/backend configuration.
