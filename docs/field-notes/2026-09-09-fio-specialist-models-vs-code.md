# Field Note — FIO on lightweight specialist models vs code (2026-09-09)

> Status: observational / non-canonical. This note records a source-reported FIO maintenance/design observation and deliberately treats it as a challenge to an easy but unproven assumption in Kamimusuhi's model-ecology direction.
>
> Related: [`../model-ecology-and-lineage.md`](../model-ecology-and-lineage.md), [`../heterogeneous-cognitive-compute-substrate.md`](../heterogeneous-cognitive-compute-substrate.md), [`2026-09-09-fio-resident-maintenance-agent.md`](./2026-09-09-fio-resident-maintenance-agent.md), [`2026-09-09-fio-model-seeking.md`](./2026-09-09-fio-model-seeking.md)

## Source-reported observation

The FIO maintainer reports, in response to the idea of delegating work across many lightweight local models:

> 軽量モデルに分担させるの、意外とプログラムにした方がいいですよ。もってるけどwhisper以外使ってない。精度落ちる

Paraphrased: although many lightweight models are available, the maintainer found that some duties were better implemented as ordinary program logic; outside Whisper, the lightweight models were reportedly not used much because accuracy degraded.

Treat this as source-reported operational experience, not a controlled benchmark. It is useful precisely because it pushes against the attractive assumption that every narrow cognitive function should become a learned specialist model.

## Distilled lesson

**A specialist function does not imply a specialist neural model.**

For a narrow subsystem, the first question should be:

```text
what is the function?
      │
      ├─ exact / symbolic / stateful / testable
      │       -> ordinary code first
      │
      ├─ statistical perception / noisy signal interpretation
      │       -> learned model may be justified
      │
      └─ ambiguous semantic judgment
              -> model or hybrid path, benchmarked against code
```

The architectural unit should therefore be an **organ / capability contract**, not "one model per organ."

An organ may be implemented by:

```text
- deterministic code
- rules / state machine
- database query / retrieval algorithm
- classical ML
- tiny neural model
- medium local model
- external model
- hybrid pipeline
```

The implementation is replaceable beneath the contract.

## Why code can outperform a lightweight model

For many narrow tasks, a smaller model adds a probabilistic inference layer where the problem does not require one.

Examples include:

- parsing known schemas;
- checking thresholds and quotas;
- file/resource classification from explicit metadata;
- dependency traversal;
- routing based on declared capability;
- duplicate suppression;
- bookkeeping and accounting;
- lifecycle/wiring checks;
- exact transformations;
- cache and retention policy;
- state-machine transitions;
- permission checks;
- health/liveness invariants.

For these tasks, normal code can provide:

- exact reproducibility;
- lower latency;
- lower memory/VRAM use;
- easier testing;
- explicit failure states;
- stronger provenance;
- no model hallucination surface.

A lightweight model may still be worse even if it is "specialized" because specialization does not compensate for insufficient capacity, poor training distribution, or stochastic errors in an otherwise deterministic task.

## Why Whisper is a useful exception

The maintainer specifically reports continuing to use Whisper. This is consistent with a broader distinction:

```text
raw/noisy perceptual signal
  -> learned representation is often genuinely useful

already-structured machine state
  -> code often has the stronger prior
```

Speech recognition has uncertainty at the input itself. The function is not merely a symbolic transformation over already-canonical state, so a learned model earns its complexity much more naturally.

This does **not** establish that Whisper is uniquely suitable or that every sensory function should be neural. It identifies a category where learned inference has a clearer reason to exist.

## Implication for Kamimusuhi model ecology

The existing model-ecology hypothesis that small specialist models can act as organs should be read as:

> **Some organs may be learned specialist models.**

not:

> **Every organ should be a model.**

A more robust hierarchy is:

```text
capability / organ contract
        │
        ▼
cheapest implementation meeting quality target
        │
        ├─ code / algorithm
        ├─ code + retrieval
        ├─ classical ML
        ├─ learned specialist
        ├─ resident general model
        └─ external escalation
```

This changes optimization from "how many models can we distribute work across?" to:

> **What is the least expensive mechanism that meets the required accuracy, latency, and authority constraints?**

## Candidate routing rule

For every proposed specialist organ, benchmark in this order when meaningful:

1. deterministic/rule baseline;
2. algorithmic or retrieval baseline;
3. tiny/small learned model;
4. resident general model;
5. larger/external model only if necessary.

Do not promote the neural implementation because it is more biologically suggestive or architecturally fashionable.

A candidate decision record:

```yaml
capability: memory_candidate_routing
quality_target:
  recall_at_k: 0.95
latency_budget_ms: 50
implementations_tested:
  - bm25_plus_rules
  - embedding_router_small
  - tiny_transformer_router
selected: bm25_plus_rules
reason:
  - highest measured recall under latency budget
  - deterministic degradation behavior
  - no GPU residency required
```

## Consequence for "brain-like" architecture

This is a useful correction to biological analogy.

Kamimusuhi can remain organism-like at the level of **functional decomposition** without requiring every organ to be neural.

```text
brain-like functional architecture
!=
neural model everywhere
```

A digital organism can have:

- neural perception;
- deterministic reflexes;
- database-backed memory indexes;
- rule-based homeostasis;
- symbolic permission boundaries;
- general-model semantic integration;
- learned policies only where measurement shows benefit.

The analogy should guide questions about function, timing, integration, and adaptation—not dictate implementation substrate.

## Research hypothesis

### H1 — Neural specialist proliferation can reduce end-to-end quality

Replacing exact program logic with weak specialist models may accumulate independent errors across a pipeline.

If each stage has accuracy `p < 1`, a serial chain can degrade rapidly even before considering correlated mistakes, routing errors, and recovery cost.

Therefore the benchmark target should be **end-to-end task quality**, not specialist-model benchmark scores in isolation.

### H2 — Learned organs should have an explicit reason to be learned

A model-backed organ should justify at least one of:

- noisy/unstructured input;
- semantics difficult to encode manually;
- adaptation from experience materially improves performance;
- generalization beyond enumerated rules is required;
- a measured quality/cost advantage over code exists.

If none applies, code should remain the default candidate.

## Suggested experiment

For one planned Kamimusuhi organ, implement three versions behind the same typed interface:

```text
A. deterministic/program implementation
B. small specialist model
C. resident general-model call
```

Measure:

- end-to-end accuracy;
- latency p50/p95;
- CPU/GPU/RAM/VRAM residency;
- energy / monetary cost;
- deterministic replayability;
- failure detectability;
- downstream error amplification;
- maintenance burden.

A model implementation should be adopted only if its measured benefit compensates for its additional stochastic and operational complexity.

## Bottom line

The FIO report adds a strong caution to Kamimusuhi's model-ecology direction:

> **Decompose cognition by function first, not by model.**

and:

> **Use a model where uncertainty or learned generalization is the problem. Use code where the answer can be made explicit, testable, and exact.**

The goal is not a swarm of neural models. The goal is a heterogeneous cognitive system in which each function uses the substrate that actually performs best.