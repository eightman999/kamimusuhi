# Learned Discrete Reflex Layer

Status: **design hypothesis / research note**

This note explores whether Kamimusuhi should place a **learned, discrete, stateful control substrate below language-model cognition**. The goal is not to replace the Persona Core or language models with logic gates. The goal is to move high-frequency, low-semantic-cost decisions — reflexes, salience, routing, wake/sleep decisions, memory triggers, and simple action selection — onto a substrate that can remain always-on with very low latency and energy cost.

This is currently **non-normative**. `architecture.md` and `spec.md` remain authoritative.

Related:

- `latency-architecture.md`
- `sensory-nervous-system.md`
- `technology-watchlist.md`
- Issues #9 and #12 for reflex/background cognition and Cognitive Budget

---

## 1. Core hypothesis

A discrete computational element is locally non-smooth, but a sufficiently large collection of discrete elements distributed over **space and/or time** can represent and process effectively smooth quantities.

This suggests a broader design space than conventional dense neural inference:

```text
discrete computational elements
        +
spatial expansion / temporal expansion
        +
state / probability / population coding
        +
a learnable rule
        =
low-latency learned control substrate
```

The interesting question for Kamimusuhi is not whether a Boolean circuit can imitate a Transformer. It is whether many of the organism's **fast control decisions should never require a Transformer at all**.

Candidate primitives include:

- Boolean gates;
- lookup tables;
- finite-state machines;
- counters and threshold units;
- event/spike processes;
- small cellular-automaton-like local rules;
- stochastic/probabilistic bit streams;
- bit-packed integer operations.

The implementation should be judged by measured behavior, not by biological analogy.

---

## 2. Why this fits Kamimusuhi

Kamimusuhi already separates immediate reflex/background behavior from deeper cognition. The existing latency design is approximately:

```text
input
  -> reflex / interrupt
  -> K-Edge Persona Core
  -> K-Core deliberate thought
  -> tools / specialists
  -> frontier/external cognition
  -> K-Deep
```

A learned discrete layer can sit **before K-Edge**:

```text
                    +-----------------------------+
                    | Language / Semantic Cortex  |
                    | K-Edge / K-Core / K-Deep    |
                    +--------------^--------------+
                                   | wake / route
                    +--------------+--------------+
                    | Learned Discrete Reflex     |
                    | Layer                       |
                    |                             |
                    | gates / LUT / FSM / state   |
                    | salience / routing / action |
                    +--------------^--------------+
                                   |
                    +--------------+--------------+
                    | Perception / Memory / Clock |
                    | sensors / events / state    |
                    +-----------------------------+
```

The layer should be thought of as a **fast nervous system / controller**, not as the canonical self.

Canonical identity, autobiographical continuity, normative self-state, and semantic interpretation must remain outside this replaceable optimization substrate.

---

## 3. Candidate responsibilities

The first targets should be decisions with:

- high event frequency;
- low semantic complexity;
- a compact observable state;
- clear measurable outcomes;
- strong latency or energy sensitivity.

Candidate outputs:

```text
WAKE_K_EDGE
WAKE_K_CORE
RETRIEVE_EPISODIC_MEMORY
RETRIEVE_LIBRARY
WRITE_EPISODE
UPDATE_SALIENCE
SPEAK_OR_BACKCHANNEL
IGNORE_EVENT
RUN_BACKGROUND_JOB
USE_LOCAL_MODEL
USE_REMOTE_MODEL
ESCALATE_COGNITION
INTERRUPT_CURRENT_ACTION
```

Candidate inputs:

```text
user_called
user_present
conversation_active
idle_duration_bucket
sensor_changed
novelty_bucket
salience_bucket
uncertainty_bucket
memory_hit_bucket
recently_spoke
current_cognitive_budget
resource_pressure_bucket
recent_failure
pending_goal_count_bucket
interruptibility
```

The first implementation should prefer quantized/bucketed features rather than attempting to feed raw language or raw sensor streams directly into a logic network.

---

## 4. Stateful temporal controller

A purely feed-forward reflex classifier is useful but does not exploit the most important property of a persistent organism: **time**.

Instead of only learning

```text
y_t = f(x_t)
```

consider

```text
(state_{t+1}, action_t) = f(input_t, state_t)
```

where `state_t` may contain low-bandwidth internal variables such as:

- attention;
- arousal;
- curiosity;
- interaction momentum;
- confidence;
- fatigue/resource pressure;
- unresolved-goal pressure;
- social distance / relationship mode signal;
- cooldown and refractory state.

These values are not themselves the full personality. They are **control-state signals** that influence when expensive cognition is invoked.

Example:

```text
small environmental change
        |
        v
novelty increases
        |
        v
change persists for several cycles
        |
        v
attention state crosses threshold
        |
        +--> prefetch memory
        |
        +--> if still salient, wake K-Edge
```

This gives Kamimusuhi internal dynamics even while no language model is decoding tokens.

---

## 5. Relationship to personality

Do **not** encode the whole persona into a small discrete controller.

Language, world knowledge, social interpretation, autobiographical meaning, and nuanced value judgments remain Persona Core responsibilities.

However, a discrete stateful layer may encode **behavioral tendencies and control biases**, for example:

- how rapidly curiosity accumulates;
- how readily repeated stimuli become ignorable;
- when uncertainty forces escalation;
- when the system chooses to stay silent;
- how aggressively memories are prefetched;
- how quickly an interrupted thread regains priority.

This is closer to temperament / regulation than to prose persona prompting.

Any learned control state that materially affects long-horizon identity should be observable, versioned, and auditable. It must not silently become a second hidden canonical self.

---

## 6. Relevant external model families

### 6.1 Deep Differentiable Logic Gate Networks (DLGN)

Petersen et al., NeurIPS 2022:

- https://proceedings.neurips.cc/paper_files/paper/2022/hash/0d3496dd0cec77a999c98d35003203ca-Abstract-Conference.html

The model learns networks of Boolean logic gates through a continuous differentiable relaxation, then discretizes them for inference. The paper reports inference beyond one million MNIST images per second on a single CPU core for the evaluated configuration.

**Established external result:** learned logic-gate networks can provide extremely cheap inference on suitable classification tasks.

**Kamimusuhi interpretation:** reflex/salience/routing classifiers are structurally closer to this regime than language generation is.

**Unproven:** the reported image-classification throughput says little by itself about persistent-agent control quality.

### 6.2 Convolutional Differentiable Logic Gate Networks

Petersen et al., NeurIPS 2024:

- https://proceedings.neurips.cc/paper_files/paper/2024/hash/db988b089d8d97d0f159c15ed0be6a71-Abstract-Conference.html

This line extends differentiable logic-gate networks with convolution-like structure and larger-scale image models.

**Why it matters:** it demonstrates that the paradigm is not limited to the smallest tabular/toy settings, although it still does not establish suitability for temporal agent control.

### 6.3 Light Differentiable Logic Gate Networks (LDLGN)

Rüttgers et al., ICLR 2026:

- https://proceedings.iclr.cc/paper_files/paper/2026/hash/6d8910336fe8a0eee83f2fa660347147-Abstract-Conference.html

The work identifies training/scaling problems in the original parametrization and proposes a reparametrization. For binary inputs the authors report about 4x smaller model parameter size, up to 1.86x faster backward passes, and convergence in up to 8.5x fewer training steps in their experiments.

**Why it matters:** logic-gate learning is an active architecture/training problem rather than only a fixed 2022 curiosity.

### 6.4 Differentiable Weightless Neural Networks (DWN)

Bacellar et al., ICML 2024:

- https://proceedings.mlr.press/v235/bacellar24a.html

DWN uses interconnected lookup tables rather than conventional weighted neurons. Training uses an Extended Finite Difference method to approximate differentiation over binary values. The work evaluates FPGA, microcontroller, and very-low-cost hardware settings and reports favorable latency/throughput/energy/area tradeoffs against its selected baselines.

**Kamimusuhi interpretation:** LUT-based learned routing and control is especially attractive because a deployed controller can be compact, deterministic, inspectable, and hardware-friendly.

### 6.5 Stochastic Computing

Review:

- Lee & Abdul Halim, *Stochastic computing in convolutional neural network implementation: a review* (2021): https://pmc.ncbi.nlm.nih.gov/articles/PMC7924419/

Stochastic Computing represents values by bit-stream statistics. For example, in the unipolar representation the proportion of `1` bits represents a value in `[0,1]`, and simple gates can implement arithmetic relationships under appropriate independence/correlation assumptions.

Example intuition:

```text
stream A: probability of 1 = a
stream B: probability of 1 = b

independent A AND B
    -> probability of 1 approximately a*b
```

This is a concrete example of **discrete local operations yielding smooth aggregate behavior over time/populations**.

However, SC is not automatically low latency. Accuracy may require longer bit streams, random/stochastic number generation has cost, and stream correlation can materially affect correctness.

For Kamimusuhi, SC should therefore be treated as a **research mechanism**, not the default v1 implementation.

---

## 7. Time-space tradeoff

The original motivating idea is important: precision/smoothness can be obtained by spreading computation across time, space, or both.

A conceptual workload requiring `N` samples does not necessarily imply a purely serial design:

```text
1 lane   x 256 cycles
16 lanes x  16 cycles
64 lanes x   4 cycles
```

This suggests a hardware-aware optimization problem:

```text
latency ~= temporal depth / available parallel spatial lanes
```

This is only a conceptual relation; actual latency depends on encoding, memory movement, synchronization, hardware utilization, and required confidence/precision.

The important Kamimusuhi design parameter is therefore **how much cognition is represented spatially versus temporally**.

On CPUs and Apple/PC GPUs, bit-packed Boolean/LUT operations may be preferable initially. On FPGA/ASIC-like hardware, more aggressive spatial unrolling becomes realistic.

---

## 8. Recommended implementation path

Do not start by building custom stochastic hardware.

### Phase A — instrument the existing controller

Keep current heuristics and LLM routing. Log decision episodes:

```text
(timestamp,
 observed_features,
 compact_state_before,
 action_taken,
 resource_used,
 latency,
 eventual_outcome,
 compact_state_after)
```

This produces a training/evaluation corpus from real organism operation.

### Phase B — train a small learned controller offline

Candidate baselines:

1. hand-written rule system;
2. decision tree / boosted tree;
3. tiny MLP;
4. DLGN/LDLGN-style logic network;
5. LUT/DWN-style controller;
6. finite-state-machine baseline.

The goal is not to make the exotic architecture win. The goal is to measure whether it is actually better for this task.

### Phase C — shadow mode

The learned controller predicts actions but cannot execute them.

Compare:

- action agreement;
- missed salient events;
- unnecessary wakeups;
- escalation quality;
- energy/CPU cost;
- decision latency;
- stability over long runs.

### Phase D — bounded authority

Allow only reversible/low-risk actions first:

- cache/prefetch;
- local retrieval;
- salience updates;
- scheduling background cognition;
- selecting among already-approved local inference resources.

High-impact actions remain behind explicit policy/capability gates.

### Phase E — temporal/stateful training

After the stateless controller is understood, add recurrent discrete state and evaluate whether it improves long-horizon behavior without causing opaque drift.

### Phase F — hardware experiments

Only after software evidence exists:

- bit-packed SIMD CPU inference;
- GPU integer/bitwise kernels;
- Raspberry Pi / microcontroller edge control;
- FPGA synthesis;
- Stochastic Computing variants if they offer a concrete win.

---

## 9. Teacher distillation from the current organism

A practical way to bootstrap the controller is to use the existing runtime as a teacher.

```text
heuristics + K-Edge/K-Core routing
              |
              | many logged events
              v
      (features, action, outcome)
              |
              v
     learned discrete controller
              |
              v
      shadow-mode comparison
```

Possible labels include both the teacher decision and delayed outcome metrics. This matters because blindly cloning the current controller would also clone its mistakes.

Longer-term training can optimize for outcomes such as:

- useful response latency;
- unnecessary model wakeups avoided;
- salient event recall;
- memory-prefetch precision/recall;
- user interruption handling;
- cognitive cost per useful episode;
- task completion / recovery rate.

---

## 10. Auditability requirement

This subsystem is attractive partly because it may be easier to inspect than a language model, but that benefit is not automatic.

Required observability:

```text
input features at decision time
controller version / hash
state before
selected action
state after
fallback/escalation reason
outcome feedback
```

For logic/LUT implementations, tooling should support tracing the active gates/table entries responsible for an output where practical.

Every deployed version should be reproducible from:

- training data snapshot or derivation manifest;
- feature schema version;
- training code/version;
- frozen controller artifact;
- benchmark report.

The controller must have an escape hatch that disables it and returns to a known-safe deterministic baseline.

---

## 11. Safety and architectural boundaries

A fast learned reflex must **not** bypass:

- capability/security policy;
- authentication/authorization;
- irreversible-action confirmation rules;
- canonical identity/versioning rules;
- memory integrity rules;
- external-resource budget ceilings.

Recommended layering:

```text
learned suggestion / routing decision
              |
              v
 deterministic policy/capability gate
              |
              v
            action
```

This keeps learned reflex optimization separate from authority.

---

## 12. Benchmark plan

A useful benchmark should contain realistic event streams rather than only independent rows.

### Workloads

- ordinary conversation with long idle periods;
- rapid back-and-forth speech;
- repeated low-value sensor noise;
- rare high-salience events;
- ambiguous events requiring escalation;
- resource pressure / model unavailability;
- multiple simultaneous goals;
- interruptions during long-running cognition;
- sleep/background consolidation windows.

### Metrics

**Latency**

- p50/p95/p99 controller decision latency;
- event-to-reflex latency;
- event-to-K-Edge wake latency.

**Quality**

- salient-event miss rate;
- false wake rate;
- escalation precision/recall;
- memory-prefetch hit rate;
- inappropriate silence/action rate;
- temporal stability.

**Resource cost**

- CPU time;
- joules or wall-power delta where measurable;
- memory footprint;
- model wakeups per hour;
- frontier/API calls avoided.

**Long-horizon behavior**

- oscillation/flapping rate;
- state saturation;
- recovery after failure;
- behavior under distribution shift;
- drift after online adaptation.

Do not accept a controller that is 100x faster but materially worse at noticing the rare events that matter.

---

## 13. Research questions

1. Can a DLGN/LDLGN/DWN-like model match or beat a conventional tiny MLP/tree on persistent-agent routing at lower latency/energy?
2. Does explicit temporal discrete state improve behavior over independent event classification?
3. How many state bits are needed before useful emergent regulation appears?
4. Can an interpretable controller learn useful curiosity/salience dynamics from real event traces?
5. What features should be binary/bucketed, and which should remain continuous?
6. Does time-space expansion offer a useful quality/latency tradeoff on commodity CPU hardware before FPGA deployment?
7. Can the controller be safely updated online without catastrophic policy drift?
8. Which decisions should remain hand-written forever because their correctness requirements exceed the benefit of learning?
9. Can a learned controller reduce LLM wake frequency enough to materially improve an always-on system's power budget?
10. Does the controller improve the subjective sense of continuous life by keeping low-level internal dynamics active between language-model invocations?

---

## 14. Near-term decision

**Do not rewrite the current architecture around DLGN/DWN yet.**

Adopt the following now:

```text
1. define a stable Reflex Controller interface;
2. instrument decisions and outcomes;
3. keep the current deterministic controller as baseline/fallback;
4. add an interchangeable learned-controller experiment slot;
5. benchmark trees / tiny NN / DLGN/LDLGN / LUT-DWN approaches;
6. explore temporal state only after the stateless benchmark is reliable.
```

This preserves Kamimusuhi's architecture-neutral cognition contracts while making the project ready to exploit learned discrete computation if the measurements support it.

The strongest framing is:

> Kamimusuhi should not spend language-model cognition on every event. A persistent organism may benefit from a cheap, learned, continuously running nervous system beneath its semantic cortex.
