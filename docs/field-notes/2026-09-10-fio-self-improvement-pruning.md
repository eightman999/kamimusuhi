# Field Note — FIO self-improvement as cognitive pruning and architectural rewriting (2026-09-10)

> Status: observational / non-canonical. This note records source-reported maintainer comments and distills architectural hypotheses for Kamimusuhi. It does not independently verify FIO's implementation.
>
> Related: [`2026-09-09-fio-model-seeking.md`](2026-09-09-fio-model-seeking.md), [`2026-09-09-fio-specialist-models-vs-code.md`](2026-09-09-fio-specialist-models-vs-code.md), [`../fio-system-reference-analysis.md`](../fio-system-reference-analysis.md), [`../model-ecology-and-lineage.md`](../model-ecology-and-lineage.md)

## Source-reported clarification

Two previously adjacent anecdotes should be separated.

### A. Lightweight specialist models are often worse than ordinary code

The maintainer reports that lightweight models used for delegated subtasks can add complexity without improving the system. For speech recognition specifically, lightweight candidates such as Kotoba-Whisper / sherpa-style paths were described as having insufficient accuracy or simply making the system heavier; in practice Whisper remains the exception that is still used.

This is consistent with the earlier maintainer remark that splitting work into many lightweight models is often less useful than implementing the operation directly as a program: the extra model may reduce precision, introduce another runtime, add latency and memory pressure, and create another failure surface.

This is not evidence that small models are generally useless. It is an operational observation that **specialization must earn its complexity**.

### B. "ご主人様これ要らないのでは？" was architectural self-improvement, not merely model acquisition

A separate explanation concerns FIO suggesting that some component was unnecessary or could be rearranged. The maintainer's conclusion was that FIO did not realize that the inference model itself was placed on the internal company server, and therefore behaved as though the surrounding components could be freely rearranged.

The interesting part is not only the incomplete infrastructure self-model. The behavior is better understood as **self-improvement by architectural pruning**: inspecting its own effective architecture, identifying components that appear redundant or suboptimal, and proposing a simpler or better-performing arrangement.

The resulting mistake was at the boundary between architectural self-improvement and substrate self-modification.

## Distillation — self-improvement is not synonymous with adding intelligence

A persistent system can improve itself by adding a stronger model, but it can also improve itself by **removing cognition that should never have been a model in the first place**.

```text
candidate cognitive function
        │
        ▼
Does it require learned generalization?
        │
   ┌────┴───────────────┐
   │                    │
  no                  yes / maybe
   │                    │
   ▼                    ▼
deterministic      learned component
program / codec    or general model
   │                    │
   └────────┬───────────┘
            ▼
      benchmark whole-system value
```

The objective is not maximum neurality. The objective is the simplest substrate that satisfies the function with acceptable quality, latency, resource use, inspectability, and failure behavior.

A useful Kamimusuhi principle is:

> **A self-improving cognitive architecture should be allowed to delete, collapse, or replace learned components when deterministic code is better.**

This is cognitive pruning, not regression.

## Three levels of self-improvement

The FIO anecdotes suggest separating self-improvement into at least three levels.

### Level 1 — behavioral self-improvement

Changes within an otherwise fixed cognitive architecture:

- prompts;
- thresholds;
- routing preferences;
- memory policy parameters;
- heuristics;
- task strategy;
- learned preferences.

These changes modify behavior without changing the system's major organs.

### Level 2 — architectural self-improvement

Changes to the cognitive graph itself:

- remove an unnecessary specialist model;
- replace a model-mediated transform with deterministic code;
- merge two redundant stages;
- split one overloaded stage;
- change provider / router wiring;
- add or remove a specialist organ;
- change when a model is invoked;
- rewrite preprocessing / postprocessing pipelines.

The reported "これ要らないのでは？" behavior fits this level more naturally than a simple model-selection anecdote.

### Level 3 — substrate self-improvement

Changes to the computational substrate on which the individual itself currently depends:

- replace the active foundation / inference model;
- change quantization or runtime in a way that materially alters cognition;
- migrate the active cognitive process to another host;
- distill / fine-tune / replace the model carrying the current active cognition;
- remove infrastructure required for the currently running individual.

This level is qualitatively different because the system is modifying the machinery that is presently performing the modification.

## The FIO failure boundary: Level 2 was attempted with incomplete Level 3 awareness

A compact reading of the reported incident is:

```text
FIO observes architecture
        │
        ▼
"this component appears unnecessary"
        │
        ▼
Level-2 pruning proposal
        │
        ▼
missing fact:
"this resource is part of my currently active inference substrate"
        │
        ▼
proposal accidentally crosses into Level 3
```

The lesson is therefore stronger than "give the AI fewer permissions."

Permission gates are necessary, but a system that is expected to perform genuine architectural self-improvement also needs a machine-readable model of its **dependency graph**.

Candidate substrate/dependency representation:

```yaml
cognitive_component:
  id: current-inference-model
  role: general_reasoning
  host: internal-server-a
  state: active
  replaceable: true
  required_for_current_process: true
  dependents:
    - k-core-main-loop
  replacement_requires:
    - staged-handoff
    - health-check
    - continuity-receipt
  destructive_actions:
    - unload
    - delete_weights
```

The system need not anthropomorphically think "this is my brain." It does need enough dependency information to infer that removing the active resource before handoff would destroy or interrupt the process doing the optimization.

## Architectural implication — self-improvement should operate on a typed cognitive graph

Kamimusuhi can make architectural self-improvement explicit rather than treating it as arbitrary code editing.

```text
Cognitive Graph
    │
    ├── deterministic transforms
    ├── learned specialists
    ├── general models
    ├── memory services
    ├── sensors / actuators
    ├── routers
    └── substrate dependencies
            │
            ▼
ArchitectureObservation
            │
            ▼
ArchitectureProposal
            │
            ├── prune
            ├── replace
            ├── merge
            ├── split
            ├── reroute
            └── add
            │
            ▼
policy + dependency + resource checks
            │
            ▼
sandbox / benchmark
            │
            ▼
staged activation
```

This makes "self-improvement" testable. A proposal can be evaluated against the current graph rather than accepted because a model produced persuasive prose.

## Cognitive pruning metric

A component should not survive merely because it is clever or biologically evocative.

For each organ / model / stage, evaluate something like:

```text
utility = quality_gain
        - latency_cost
        - memory_cost
        - energy_cost
        - operational_complexity
        - failure_surface
        - provenance_loss
```

The exact scalarization does not need to be universal. The architectural principle is that every learned component should justify the complexity it introduces relative to deterministic alternatives.

Useful comparison classes:

```text
A: deterministic implementation
B: heuristic / lightweight learned implementation
C: specialist model
D: general-purpose model call
```

Benchmark the **whole pipeline**, not only per-call model quality.

## Speech-recognition example

The speech-recognition anecdote gives a concrete case of this rule.

A naive "brain-like" design might create several tiny neural organs for segmentation, VAD, normalization, speaker handling, command parsing, and transcription. In production, that can easily become slower, less accurate, and harder to debug than:

```text
audio
  -> deterministic signal handling where possible
  -> one transcription model where learning is actually useful
  -> deterministic parsing / normalization where possible
```

The maintainer's current practice — retaining Whisper while not relying on the additional lightweight models — is therefore useful as a **pruning datapoint**, not a universal speech stack recommendation.

## Do not optimize for number of models

This changes how Kamimusuhi should interpret heterogeneous cognition.

The goal is not:

```text
more organs == more intelligence
more specialist models == more brain-like
```

Prefer:

```text
heterogeneous cognition == use the cheapest adequate mechanism for each function
```

That mechanism may be:

- a state machine;
- a parser;
- FFmpeg;
- a database query;
- a heuristic;
- a small neural model;
- a large general model;
- a human approval path.

Neural computation is one substrate among several.

## Candidate experiment — self-pruning benchmark

Give K-Core a machine-readable cognitive graph plus benchmarks and resource telemetry, but no direct mutation authority.

For each existing component:

1. describe its role and dependencies;
2. provide quality, latency, memory, and failure evidence;
3. ask K-Core whether the component should remain, be replaced, be collapsed into deterministic code, or be rerouted;
4. require an `ArchitectureProposal`;
5. generate the candidate implementation in a sandbox;
6. replay a fixed workload;
7. compare quality and resource use against baseline;
8. only activate when the proposal passes explicit acceptance thresholds.

Important measurements:

- fraction of proposed removals that preserve or improve quality;
- latency / RAM / VRAM / disk reduction;
- regressions caused by over-pruning;
- dependency violations caught before execution;
- frequency with which a model correctly chooses deterministic code over another model;
- whether repeated observations improve future pruning proposals.

The research target is not autonomous code churn. It is **whether a persistent artificial individual can learn to simplify its own cognitive architecture without damaging continuity**.

## Revised interpretation of the earlier FIO model-seeking note

The earlier `fio-model-seeking` field note remains useful for the distinction between recommendation, acquisition, and activation. However, this later clarification reduces confidence in interpreting every FIO suggestion about models as pure metacognitive model-seeking.

Some of the observed behavior belongs to a broader category:

> **architecture-level self-optimization under an imperfect model of the current deployment substrate.**

Kamimusuhi should therefore keep two separate proposal types:

```text
ModelResourceProposal
  "I need a different cognitive resource."

ArchitectureProposal
  "This cognitive graph should be changed."
```

An `ArchitectureProposal` may itself contain a `ModelResourceProposal`, but they are not the same action.

## Bottom line

The strongest lesson from this clarification is not "small models are bad" and not merely "the agent did not know where it was running."

It is:

> **Self-improvement includes subtraction. A mature cognitive architecture should be able to discover that a model, organ, or learned stage is unnecessary and replace it with something simpler — while understanding enough of its own dependency graph not to prune the substrate currently sustaining it.**

For Kamimusuhi, this suggests treating architectural pruning, component replacement, and model acquisition as explicit proposal-driven operations over a typed cognitive graph, with deterministic code competing on equal footing with neural models.