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
