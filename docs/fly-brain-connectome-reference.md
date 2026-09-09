# Fly-Brain / Connectome-Derived Reflex Substrate Reference

Status: **research note / external reference analysis**

This note records the parts of the FlyWire / Shiu et al. / Eon `fly-brain` line of work that are relevant to Kamimusuhi. It is not a proposal to copy a fruit-fly connectome into Kamimusuhi, and it is not evidence that a connectome or a spiking model is sufficient for personality, memory, selfhood, or general cognition.

Related Kamimusuhi notes:

- `learned-discrete-reflex-layer.md`
- `sensory-nervous-system.md`
- `latency-architecture.md`
- `native-runtime-stratification.md`
- `research-foundations.md`
- `multiscale-brain-architecture.md`

Primary implementation examined:

- Eon Systems PBC, `eonsystemspbc/fly-brain`: https://github.com/eonsystemspbc/fly-brain

---

## 1. Why this matters

The useful lesson is narrower, and more concrete, than "brain emulation".

A large sparse recurrent network with simple local dynamics can implement nontrivial **sensor-to-internal-state-to-motor transformations** without invoking anything resembling a language model. That makes this work directly relevant to Kamimusuhi's unresolved layer below K-Edge: reflexes, salience accumulation, wake/escalation decisions, sensorimotor loops, low-level action selection, and continuously evolving state between semantic-model invocations.

The strongest Kamimusuhi interpretation is:

> A persistent agent does not need to spend semantic cognition on every event. A cheap temporal control substrate can remain alive continuously, while expensive language/semantic cognition is invoked selectively.

This supports the architectural separation already explored in `learned-discrete-reflex-layer.md`, but adds a concrete spiking/recurrent reference implementation and an experimentally validated neuroscience precedent.

---

## 2. Evidence ledger

Keep three evidence levels separate.

### 2.1 Established, peer-reviewed results

The FlyWire adult Drosophila connectome provides a whole-brain wiring resource at roughly 140k neurons and more than 50 million synapses. The flagship wiring-diagram paper reports 139,255 proofread neurons.

- Dorkenwald et al., **Neuronal wiring diagram of an adult brain**, Nature 634, 124–138 (2024). DOI: https://doi.org/10.1038/s41586-024-07558-y
- FlyWire overview: https://flywire.ai/

Shiu et al. built a whole-brain **leaky integrate-and-fire (LIF)** model from connectivity and predicted neurotransmitter identity. They used computational activation of sensory and internal neurons to predict downstream responses relevant to feeding and grooming, and tested predictions experimentally.

Across 164 predictions that the authors could test empirically, **91% were consistent with the empirical results**. The paper also reports 84% when a large split-GAL4 optogenetic experiment dominated by negative cases is excluded. These numbers are about the tested sensorimotor predictions in that study; they are not a general "91% accurate fly brain" score.

- Shiu et al., **A Drosophila computational brain model reveals sensorimotor processing**, Nature 634, 210–219 (2024). DOI: https://doi.org/10.1038/s41586-024-07763-9
- Open-access article: https://pmc.ncbi.nlm.nih.gov/articles/PMC11446845/

This is important because it demonstrates that **coarse neuron dynamics plus structured connectivity can preserve useful functional information** for some sensorimotor transformations.

### 2.2 Current open implementation

The `eonsystemspbc/fly-brain` repository turns the Shiu-model lineage into a multi-backend benchmark/reference implementation.

At the time of this note it provides six execution paths:

1. Brian2 C++ standalone (CPU);
2. Brian2CUDA;
3. PyTorch CUDA;
4. NEST GPU;
5. GeNN / PyGeNN;
6. Brian2GeNN.

The implementations share the same data/model parameters and use a **0.1 ms simulation timestep**. The benchmark infrastructure separates setup/build cost from simulation cost and exports spike trains so backends can be compared using active-neuron overlap, firing-rate correlation, spike-count ratios, and pairwise spike timing.

The repository currently uses FlyWire public-release v783 data. Its README describes approximately 138k neurons and approximately 5M sparse connections in the simulation representation, while the biological FlyWire source contains more than 50M individual synapses. These should not be conflated: a sparse neuron-pair graph can aggregate many biological synapses into weighted connections.

Implementation: https://github.com/eonsystemspbc/fly-brain

License note: the repository is GPL-2.0-or-later except where otherwise noted; upstream Shiu/Brian2 material called out in the repository retains its upstream MIT license. Reusing implementation code in Kamimusuhi therefore requires deliberate license-boundary review. Reusing the architectural ideas and benchmark methodology does not imply copying GPL code.

### 2.3 2026 embodied-fly demonstration: useful but not equivalent to peer-reviewed validation

In March 2026, Eon described an embodied simulation combining the connectome-based LIF brain model with:

- a connectome-constrained visual model;
- NeuroMechFly v2;
- MuJoCo physics;
- a small set of descending-neuron readouts;
- lower-level behavior controllers.

Eon reports synchronizing brain and body in 15 ms steps. The important engineering pattern is a closed loop:

```text
world / body sensors
        |
        v
identified sensory pathways
        |
        v
connectome-constrained recurrent brain model
        |
        v
low-dimensional descending signals
        |
        v
lower-level motor controllers
        |
        v
body / world changes
        |
        +-------------------------------> next sensory state
```

Technical post: https://eon.systems/updates/embodied-brain-emulation

However, Eon's own technical post explicitly records major limitations:

- only a subset of sensory inputs and behaviors is implemented;
- the LIF model omits substantial biological detail;
- internal state, plasticity, learning, and hormonal modulation are largely missing;
- the body is not driven through the complete biological downstream motor hierarchy;
- a small number of descending outputs are used as low-dimensional control handles;
- mappings from neural activity to body control are partly chosen by hand;
- body controllers already contain learned/imitation-based behavior;
- Eon does not claim that the demo rigorously proves structure alone is sufficient for the fly's full behavioral repertoire.

Independent criticism is therefore important. A 2026 report in *The Transmitter* discusses a "digital sphinx" experiment showing that a mismatched connectome coupled through learned mappings can still produce plausible locomotion. The consequence for Kamimusuhi is methodological: **behavioral plausibility alone is weak evidence that the internal substrate is doing the intended computation**.

- The Transmitter, **Digital sphinx raises questions about connectome models** (2026): https://www.thetransmitter.org/systems-neuroscience/digital-sphinx-raises-questions-about-connectome-models/

---

## 3. What the Shiu model does not establish

The Nature paper is unusually useful because it is explicit about its simplifications. The model treats neurons with a simple LIF formulation and does not attempt to reproduce the full biological substrate.

Reported/acknowledged omissions include, among others:

- detailed neural morphology and dendritic computation;
- diverse receptor/channel dynamics;
- gap junctions;
- non-spiking neurons;
- long-range neuropeptide effects;
- rich internal state;
- learning and plasticity in the embodied demonstration;
- complete brain-to-body motor hierarchy.

The paper also emphasizes that connectivity constrains circuit mechanisms but does not uniquely determine them.

For Kamimusuhi, this blocks an attractive but invalid inference:

```text
connectome-like graph + LIF
    !=
complete organism / personality / self
```

The evidence instead supports the narrower inference:

```text
structured sparse recurrent dynamics
    -> can retain substantial sensorimotor/control competence
       even with very simple node dynamics
```

That narrower result is exactly the part Kamimusuhi can exploit.

---

## 4. Proposed Kamimusuhi mapping: a non-canonical K-Nerve layer

For discussion, call the experimental layer **K-Nerve**. This is a non-normative name.

K-Nerve sits below K-Edge and above raw perception/control plumbing:

```text
Sensors / clocks / runtime events
              |
              v
      event / spike encoder
              |
              v
+----------------------------------+
| K-Nerve                          |
|                                  |
| temporal sparse recurrent state  |
| LIF / SNN / DLGN / DWN / FSM     |
| salience / arousal / interrupts  |
+----------------+-----------------+
                 |
                 | low-dimensional control signals
                 v
+----------------------------------+
| deterministic capability/policy |
| gate                             |
+----------------+-----------------+
                 |
        +--------+---------+
        |                  |
        v                  v
   K-Edge wake       local action/reflex
        |
        v
   K-Core / K-Deep when required
```

Candidate "descending" outputs are not muscles; they are control handles such as:

```text
WAKE_K_EDGE
WAKE_K_CORE
PREFETCH_MEMORY
WRITE_EPISODE_CANDIDATE
RAISE_SALIENCE
BACKCHANNEL
INTERRUPT
IGNORE_EVENT
RUN_BACKGROUND_JOB
ESCALATE_UNCERTAINTY
```

This is closely analogous to the embodied-fly architecture only at the **control-interface level**: a rich recurrent substrate is reduced to a small number of meaningful downstream signals. It is not an argument to reproduce fly neural topology.

---

## 5. The deeper design lesson: topology can be a prior

The Shiu result suggests that useful computation can reside substantially in **network structure**, not only in sophisticated per-node computation.

Kamimusuhi should test this as an engineering hypothesis:

> Instead of making every low-level decision with a more capable node/model, encode useful inductive bias into sparse connectivity, recurrence, time constants, refractory behavior, and routing structure.

Possible engineered modules:

```text
novelty pathway
habituation pathway
threat / anomaly pathway
social-presence pathway
unfinished-goal pathway
resource-pressure pathway
memory-prefetch pathway
speech/backchannel pathway
```

Connections between these modules can create temporal behavior such as accumulation, inhibition, refractory periods, hysteresis, competition, and delayed escalation without decoding language tokens continuously.

The architecture should still be learned or tuned against data. There is no evidence that the Drosophila connectome itself is an appropriate topology for an artificial persistent agent.

A better abstraction is:

```text
biological connectome
    -> evidence that structured sparse recurrence is powerful

Kamimusuhi
    -> engineer/learn a task-appropriate sparse recurrent control graph
```

---

## 6. Relation to the Learned Discrete Reflex Layer

`learned-discrete-reflex-layer.md` currently considers:

- DLGN / LDLGN;
- DWN / LUT networks;
- FSMs;
- event/spike processes;
- stochastic computing;
- bit-packed logic.

The fly-brain line adds **LIF/SNN-style temporal recurrent dynamics** as a first-class benchmark candidate rather than only a conceptual event primitive.

The comparison should therefore become:

```text
hand rules
    vs decision tree / boosted tree
    vs tiny MLP
    vs FSM
    vs DLGN / LDLGN
    vs DWN / LUT
    vs LIF / SNN recurrent controller
```

No architecture should win by analogy. It must win by measured latency, power, robustness, control quality, and interpretability.

---

## 7. Closed-loop evaluation is more important than classifier accuracy

The strongest transferable idea from the embodied-fly work is **closing the loop**.

A Kamimusuhi reflex substrate should not be evaluated only on independent labeled rows. Its outputs change the future input distribution.

Example:

```text
user appears
   -> presence salience rises
   -> memory prefetch fires
   -> K-Edge wakes earlier
   -> conversation begins
   -> recent-interaction state rises
   -> later ambiguous sound is interpreted differently
```

Therefore evaluation must include temporal episodes and feedback.

Required closed-loop metrics:

- event-to-control latency;
- event-to-K-Edge wake latency;
- false wake rate;
- salient-event miss rate;
- unnecessary LLM invocations per hour;
- memory-prefetch precision/recall;
- state oscillation / flapping;
- recovery after interruption or failure;
- behavior under noisy/repeated stimuli;
- compute and energy per wall-clock hour;
- stability during long idle periods;
- quality under resource pressure.

---

## 8. Internal validation, not just outward behavior

The "digital sphinx" criticism is directly relevant to Kamimusuhi.

A system can look plausible while its internal mechanism is wrong or irrelevant. Therefore K-Nerve experiments need **mechanistic tests** in addition to outward behavior.

Recommended tests:

### 8.1 Ablation

Disable a pathway/module and confirm the predicted loss of function.

Examples:

- remove habituation -> repeated noise should wake the system too often;
- remove novelty accumulation -> rare gradual changes should be missed;
- remove refractory state -> interrupt/backchannel oscillation should increase;
- remove memory-prefetch path -> first-token useful-response latency should worsen.

### 8.2 Counterfactual stimulation

Inject a controlled synthetic event into one internal pathway and verify downstream effects without relying on a natural user interaction.

### 8.3 State tracing

Record:

```text
time
input event
active nodes/pathways
state before/after
output control signal
policy gate result
subsequent outcome
```

### 8.4 Backend parity

Borrow the `fly-brain` methodology: if the same controller is implemented in Python/PyTorch, native C++/Rust, GPU kernels, or eventually FPGA/neuromorphic hardware, compare internal event/spike/state traces rather than merely checking that the final action often matches.

This is a strong pattern worth adopting directly.

---

## 9. Practical experiment plan

### Experiment A — reproduce the external reference

When a dedicated GPU window is available, run the `fly-brain` PyTorch backend first, then one reference backend if practical.

Goals:

- understand actual sparse-state representation and memory cost;
- measure simulation-time / wall-time ratio on local hardware;
- measure GPU memory use;
- inspect spike/event sparsity;
- verify that backend-comparison tooling is reproducible.

Do not mix this with production inference or an active long training run merely because a GPU is present.

### Experiment B — synthetic K-Nerve benchmark

Build a small event-stream environment with:

- user-presence events;
- speech start/stop;
- repeated sensor noise;
- slowly accumulating novelty;
- rare urgent interrupts;
- resource-pressure changes;
- memory-hit/miss feedback;
- delayed task outcomes.

Compare the candidate controller families listed in Section 6.

### Experiment C — closed-loop avatar/runtime sandbox

Connect the winning controller to a simulated Kamimusuhi runtime, not to irreversible real actions.

The controller can decide only:

- whether to wake a model;
- whether to retrieve/prefetch;
- whether to emit a harmless local backchannel;
- whether to postpone/ignore an event.

Measure whether internal temporal state improves responsiveness while reducing expensive cognition.

### Experiment D — ablation suite

For every learned/engineered pathway, define at least one expected behavioral change when it is disabled. A pathway with no falsifiable consequence is likely decorative complexity.

---

## 10. Hardware/runtime implications

The external repository is useful because it keeps one neural model while changing the execution substrate. That matches Kamimusuhi's desired architecture-neutral cognition contracts.

Potential evolution path:

```text
research semantics
    Python / PyTorch
        |
        v
stable reference traces
        |
        v
native CPU implementation
    Rust / C++ / SIMD
        |
        v
optional GPU / CUDA
        |
        v
optional FPGA / neuromorphic target
```

The important artifact is not a particular framework; it is the **behavioral and internal-state contract** that each backend must reproduce within defined tolerances.

This also supports `native-runtime-stratification.md`: Python is suitable for exploring the dynamics, while the always-on hot path should migrate only when profiling justifies it.

---

## 11. Architectural boundary: K-Nerve is not the self

This work strengthens, rather than weakens, Kamimusuhi's need to separate low-level regulation from canonical identity.

The fly model lacks the mechanisms Kamimusuhi cares about most for persistent personhood:

- autobiographical continuity;
- semantic memory organization;
- explicit self-model;
- relationship continuity;
- normative values;
- long-horizon goals;
- reflective cognition;
- robust learning/plasticity across lived experience.

Therefore:

```text
K-Nerve
    = replaceable control physiology

Persona / canonical memory / identity
    = versioned continuity substrate
```

A K-Nerve checkpoint may influence temperament-like regulation, but it must not silently become an authoritative source of identity.

---

## 12. Extension: from reflex substrate to multi-timescale nervous system

The fly-inspired substrate should not be restricted to the very lowest reflex layer. Kamimusuhi can use the same design principles to build a **high-update-rate intermediate nervous system** between local peripheral reflexes and slow semantic cognition.

The key idea is not that biological nervous systems have one literal clock per region. Rather, useful neural computation occurs across multiple characteristic timescales. Kamimusuhi should therefore avoid a single synchronous global update loop and instead define distinct runtime domains with explicit event crossings.

Provisional organization:

```text
fast / reactive

raw sensors / body telemetry
        |
        v
PNL / local reflex arcs
hundreds of Hz to kHz-class implementation target
        |
        v
K-Fast / fly-inspired recurrent nervous system
roughly tens to hundreds of Hz-class implementation target
        |
        v
Global Workspace / salience integration
roughly single-digit to tens of Hz-class implementation target
        |
        v
Persona Core / semantic cognition
asynchronous, variable-depth, model-latency-limited

slow / semantic
```

These rates are **engineering target classes, not claims about fly or human biological clock rates**.

### 12.1 Proposed K-Fast role

`K-Fast` is a provisional name for an intermediate high-update-rate control substrate. It should sit above raw peripheral reflexes and below semantic cognition.

Candidate responsibilities:

- salience accumulation and decay;
- novelty and habituation;
- orienting responses;
- interruption and backchannel timing;
- competition between action tendencies;
- affect-like regulatory state;
- memory pre-activation / prefetch hints;
- wake/escalation thresholds for K-Edge or Persona Core;
- short-timescale prediction error;
- conversational turn-taking cues;
- rapid gaze/posture/audio-response coordination.

The important behavioral property is that Kamimusuhi can begin reacting **before** Persona Core completes semantic reasoning.

Example:

```text
speech onset
    -> K-Fast inhibits current output
    -> listening/orienting state rises
    -> Persona Core receives an attributed interruption event

name called
    -> salience rises immediately
    -> memory/person prefetch may begin
    -> Persona Core is woken only if the threshold is crossed
```

### 12.2 Clock-domain crossing as an architectural contract

Fast layers MUST NOT forward every internal update to slower layers.

Instead, boundaries should exchange timestamped events or aggregated state transitions such as:

```text
timestamp
source
signal_type
strength
confidence
duration
state_version
```

For example, a 1 kHz local pathway may update an anomaly accumulator many times, while the next layer receives only a transition such as:

```text
ANOMALY_RISE
strength: 0.76
window: 18ms
source: robot_1.audio.front
```

This is analogous to an engineered clock-domain crossing: each subsystem retains its natural update rate while interfaces remain sparse, explicit, and auditable.

### 12.3 Regulatory state as modulation, not decorative emotion tags

Affect-like variables should preferably modify circuit behavior rather than exist only as labels inserted into a language-model prompt.

Candidate regulatory signals include:

```text
arousal
novelty
threat
curiosity
fatigue
certainty
social_affinity
goal_pressure
resource_pressure
```

Their primary effect should be modulation of other mechanisms, for example:

```text
threat increases
    -> sensory gain increases
    -> interrupt threshold decreases
    -> exploration decreases
    -> semantic wake threshold decreases

fatigue increases
    -> low-value event promotion decreases
    -> expensive cognition threshold increases
    -> consolidation/sleep pressure increases
```

Persona Core may later interpret these states semantically, but the control effect should exist without requiring language generation.

### 12.4 Suggested first prototype size

Do not begin by copying the full fly connectome. A first Kamimusuhi-specific recurrent substrate can be intentionally small, for example **1k-10k simple stateful nodes**, with explicit sparse modules for:

- sensory novelty;
- social presence;
- memory activation;
- goal/action competition;
- regulatory state;
- orienting/reflex coordination;
- integrator/broadcast pathways.

Candidate per-node state can remain minimal:

```text
potential
threshold
decay
refractory
gain
connections[]
```

The goal of the first prototype is not biological fidelity. It is to test whether a sparse temporal substrate can improve reaction timing, reduce unnecessary model wakeups, and provide useful continuously evolving internal state.

### 12.5 Architecture consequence

The resulting Kamimusuhi brain should be understood as a **multi-timescale organism**, not a stack where all cognition is performed by an LLM:

```text
Persona Core
  semantic / self / social / deliberative cognition
        ^
        |
Global Workspace
  bounded integration and conscious-like access surface
        ^
        |
K-Fast
  fly-inspired sparse recurrent control and regulation
        ^
        |
PNL
  local sensorimotor and machine reflexes
        ^
        |
sensors / body / runtime
```

This preserves Persona Core as the high-level first-person semantic organ while allowing a much faster, cheaper nervous system to remain continuously active.

A dedicated normative design note is maintained in `multiscale-brain-architecture.md`; this section records why the FlyWire/fly-brain line motivates that direction.
