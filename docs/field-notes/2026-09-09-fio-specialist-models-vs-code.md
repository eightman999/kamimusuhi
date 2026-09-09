# Field Note — FIO on lightweight specialist models vs code (2026-09-09)

> Status: observational / non-canonical. This note records source-reported FIO maintenance/design observations and deliberately treats them as a challenge to an easy but unproven assumption in Kamimusuhi's model-ecology direction.
>
> Related: [`../model-ecology-and-lineage.md`](../model-ecology-and-lineage.md), [`../heterogeneous-cognitive-compute-substrate.md`](../heterogeneous-cognitive-compute-substrate.md), [`2026-09-09-fio-resident-maintenance-agent.md`](./2026-09-09-fio-resident-maintenance-agent.md), [`2026-09-09-fio-model-seeking.md`](./2026-09-09-fio-model-seeking.md)

## Source-reported observations

The FIO maintainer reports, in response to the idea of delegating work across many lightweight local models:

> 軽量モデルに分担させるの、意外とプログラムにした方がいいですよ。もってるけどwhisper以外使ってない。精度落ちる

Paraphrased: although many lightweight models are available, the maintainer found that some duties were better implemented as ordinary program logic; outside Whisper, the lightweight models were reportedly not used much because accuracy degraded.

A follow-up adds an important correction to a simplistic "code is rigid, models are plastic" framing. The maintainer reports that even after distilling a behavior into program logic, FIO can still modify the thresholds controlling that logic by having Python rewrite `.env` configuration values. They also report converging on the view that some operations such as `ffmpeg` manipulation are faster and more reliable when delegated directly to the resident general agent/tool path rather than wrapped in another lightweight specialist model.

The maintainer describes this direction as reconnecting with older systems/programming knowledge rather than inventing a neural substitute for every operation.

Treat these as source-reported operational experiences, not controlled benchmarks. They are useful precisely because they push against the attractive assumption that every narrow cognitive function should become a learned specialist model.

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
- existing executable/tool
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

## New implication: deterministic code can still be plastic

The follow-up observation matters because it breaks another false dichotomy:

```text
model = adaptive
code  = fixed
```

A better decomposition is:

```text
mechanism
  deterministic implementation

policy parameters
  thresholds / weights / quotas / timing / feature flags

adaptation process
  who may change those parameters, from what evidence, and under what limits
```

A Python script rewriting `.env` thresholds is still deterministic software at execution time, but the **effective policy is plastic across time**.

Conceptually:

```text
observation history
      │
      ▼
resident agent proposes / chooses parameter change
      │
      ▼
config mutation
      │
      ▼
deterministic program executes with new policy
      │
      ▼
measured outcome
      │
      └──────────────► next adaptation
```

This provides a potentially valuable middle layer between hard-coded rules and learned neural weights.

Call it, provisionally, **plastic deterministic substrate**:

> deterministic mechanisms whose behavior can be adapted by explicit, inspectable parameter changes rather than by hidden weight updates.

Advantages over neural adaptation include:

- exact diff of what changed;
- easy rollback;
- bounded parameter ranges;
- replayable behavior under the same configuration;
- direct attribution from outcome to policy change;
- no retraining step;
- much lower compute cost.

The cost is that the adaptation space must be designed explicitly, and a resident agent can still destabilize the system if allowed to move thresholds without bounds or evidence.

Therefore adaptive configuration should preferably carry provenance such as:

```yaml
parameter: silence_timeout_ms
old_value: 900
new_value: 1250
changed_by: resident_agent
reason: repeated premature cutoff
supporting_observations: [...]
allowed_range: [300, 3000]
rollback_value: 900
observed_effect: pending
```

The important architectural insight is not that `.env` should literally become a learning system. It is that **inspectable configuration mutation can serve as a cheap form of procedural learning**.

## Existing tools may be better organs than new models

The `ffmpeg` example suggests another correction.

For many domains, the best specialist already exists as mature executable software:

```text
media transform  -> ffmpeg
image transform  -> ImageMagick / native libraries
compression      -> codecs / archivers
SQL work         -> database engine
network probing  -> OS/network tools
version control  -> git
symbolic math    -> CAS / numerical library
```

The resident agent does not need to internalize or re-learn the transformation. It needs to:

1. understand the goal;
2. select the right existing tool;
3. construct valid parameters;
4. inspect the result;
5. retry or revise if verification fails.

This yields another hierarchy:

```text
need a capability
      │
      ├─ mature exact tool already exists
      │       -> delegate to tool
      │
      ├─ simple exact logic
      │       -> code / rules
      │
      ├─ exact mechanism but operating point changes
      │       -> code + plastic configuration
      │
      ├─ noisy perception / difficult semantic mapping
      │       -> learned specialist
      │
      └─ broad novel reasoning
              -> resident/general/external model
```

This is more efficient than creating a new learned organ for every named function.

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
        ├─ existing tool / executable
        ├─ deterministic code / algorithm
        ├─ code + plastic configuration
        ├─ code + retrieval
        ├─ classical ML
        ├─ learned specialist
        ├─ resident general model
        └─ external escalation
```

This changes optimization from "how many models can we distribute work across?" to:

> **What is the least expensive mechanism that meets the required accuracy, latency, adaptability, observability, and authority constraints?**

## Candidate routing rule

For every proposed specialist organ, benchmark in this order when meaningful:

1. mature existing tool / executable;
2. deterministic/rule baseline;
3. deterministic implementation with bounded adaptive parameters;
4. algorithmic or retrieval baseline;
5. tiny/small learned model;
6. resident general-model call;
7. larger/external model only if necessary.

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
adaptive_parameters:
  candidate_limit:
    range: [10, 100]
    mutation_policy: bounded_runtime_tuning
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
- parameter-plastic reflex thresholds;
- database-backed memory indexes;
- rule-based homeostasis;
- symbolic permission boundaries;
- mature external executables as effectors;
- general-model semantic integration;
- learned policies only where measurement shows benefit.

The analogy should guide questions about function, timing, integration, and adaptation—not dictate implementation substrate.

## Research hypotheses

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

### H3 — Some useful learning can occur in configuration space

For narrow behaviors with understandable control surfaces, bounded configuration mutation may recover part of the benefit usually sought from learned models while preserving inspectability and rollback.

Test this against both fixed rules and learned policies rather than assuming it is sufficient.

### H4 — Tool-use competence may dominate specialist-model ownership

A resident general model that knows when and how to use mature executables may outperform a collection of weak learned specialists on exact operational tasks.

The relevant skill to distill may therefore be **tool selection + parameterization + verification**, not the tool's underlying transformation itself.

## Suggested experiments

### Experiment A — code vs specialist vs resident model

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

### Experiment B — fixed rules vs plastic deterministic substrate

Choose a behavior currently controlled by thresholds.

Compare:

```text
A. fixed configuration
B. human-tuned configuration
C. agent-tuned bounded configuration
D. learned policy/model
```

Measure:

- task quality;
- convergence speed;
- oscillation / instability;
- number of parameter mutations;
- rollback frequency;
- interpretability of failures;
- cost.

Require the agent-tuned path to log every mutation and verify the measured post-change effect.

### Experiment C — specialist model vs mature tool delegation

Choose a domain such as media transformation where a mature executable already exists.

Compare:

```text
A. learned specialist pipeline
B. resident agent -> existing tool -> verification
```

Measure success rate, retries, latency, resource use, and error recovery. This directly tests the `ffmpeg` intuition rather than promoting it from anecdote to architecture.

## Bottom line

The FIO reports add a strong caution to Kamimusuhi's model-ecology direction:

> **Decompose cognition by function first, not by model.**

and:

> **Use a model where uncertainty or learned generalization is the problem. Use code where the answer can be made explicit, testable, and exact.**

The follow-up adds two more compact principles:

> **Code need not be cognitively rigid: explicit parameters can form a plastic, inspectable learning surface.**

> **Do not train a new organ to do what a mature tool already does exactly; train or prompt the individual to use the tool well.**

The goal is not a swarm of neural models. The goal is a heterogeneous cognitive system in which each function uses the substrate that actually performs best, and in which adaptation can happen at multiple layers: weights, retrieval, configuration, procedures, and tool-use policy.