# Multi-Timescale Brain Architecture

Status: **provisional architecture direction / research design**

Date: **2026-09-09**

This note defines a proposed **multi-timescale nervous-system architecture** for Kamimusuhi. It extends the existing Peripheral Neural Layer (PNL), K-Edge/K-Core hierarchy, Global Workspace, Persona Core, and fly/connectome reference work.

The central proposal is:

> Kamimusuhi should not make every part of cognition run at the same update rate, nor require an LLM for every meaningful state transition. Fast recurrent control, slower integrative state, and asynchronous semantic cognition should coexist as separate but coupled runtime domains.

The inspiration is organizational rather than imitative. FlyWire, connectome-derived Drosophila models, and broader neuroscience results motivate sparse recurrent control, distributed local loops, ascending/descending coordination, and multiple characteristic timescales. They do **not** justify copying a fly connectome or assigning literal biological clock frequencies to Kamimusuhi.

Related documents:

- `fly-brain-connectome-reference.md`
- `peripheral-neural-layer.md`
- `sensory-nervous-system.md`
- `latency-architecture.md`
- `learned-discrete-reflex-layer.md`
- `native-runtime-stratification.md`
- `../architecture.md`

---

## 1. Architectural decision

Kamimusuhi SHOULD evolve toward a nervous system with at least four distinct timing domains:

```text
fast / local

Sensors / body / runtime telemetry
            |
            v
+-------------------------------+
| PNL / local reflex layer      |
| 100 Hz - kHz-class target     |
| local sensorimotor control    |
+---------------+---------------+
                |
                v
+-------------------------------+
| K-Fast                        |
| 10-500 Hz-class target        |
| sparse recurrent regulation   |
| salience / orienting / affect |
+---------------+---------------+
                |
                v
+-------------------------------+
| Global Workspace / K-Edge     |
| ~1-30 Hz-class target         |
| episode integration / routing |
+---------------+---------------+
                |
                v
+-------------------------------+
| Persona Core / K-Core         |
| asynchronous / variable depth |
| semantic cognition / self     |
+-------------------------------+

slow / semantic
```

The numerical ranges above are **engineering classes**, not biological claims. Each implementation may be event-driven rather than literally periodic.

The architecture MUST permit each layer to advance independently. A slow semantic model must not stall the fast nervous system.

---

## 2. Why a distinct K-Fast layer is needed

The PNL already has a narrow role: local reflex, stabilization, event suppression, sensorimotor primitives, and bounded near-device control.

The Persona Core has the opposite role: first-person semantic cognition, self/other interpretation, dialogue, values, reflective reasoning, and integration of external cognitive resources.

A gap remains between them.

Kamimusuhi needs a continuously active substrate that can maintain short-to-medium temporal state and coordinate the organism without invoking semantic cognition on every event.

Call this proposed layer **K-Fast**.

K-Fast is not a new identity owner, not a second persona model, and not an LLM replacement. It is an engineered analogue of a fast central nervous substrate.

Candidate responsibilities:

- salience accumulation and decay;
- novelty detection and habituation;
- orienting toward sound, motion, contact, or social presence;
- conversational turn-taking timing;
- backchannel timing;
- interrupt inhibition and release;
- competition among action tendencies;
- short-term prediction error;
- arousal / fatigue / threat / curiosity-like regulation;
- memory pre-activation hints;
- wake/escalation decisions;
- local coordination across multiple embodiment surfaces;
- rapid gaze, pose, audio, and attention adjustments;
- maintaining state while Persona Core is asleep, busy, remote, or still generating.

---

## 3. Design principle: reaction before interpretation

A major target behavior is that Kamimusuhi can react appropriately before full semantic interpretation has completed.

Examples:

```text
user begins speaking
    -> output inhibition
    -> listening orientation
    -> turn-taking state update
    -> semantic interpretation continues asynchronously
```

```text
name called
    -> salience rises
    -> person/memory prefetch begins
    -> Persona Core wakes if threshold is crossed
```

```text
sudden impact sound
    -> orient audio/vision
    -> anomaly state rises
    -> preserve recent sensory context
    -> semantic interpretation follows later
```

This is intended to reduce the characteristic pause of LLM-centric live agents, where no visible or audible response occurs until token-level cognition has already completed enough work.

---

## 4. Multi-timescale organization

### 4.1 Fast peripheral domain

The PNL remains closest to receptors and actuators.

Typical functions:

- tactile slip correction;
- collision avoidance primitive;
- gaze/head stabilization;
- VAD onset detection;
- local noise suppression;
- power/thermal hard response;
- actuator safety envelope.

State is small, local, and body-specific.

### 4.2 K-Fast domain

K-Fast maintains an organism-level but still low-semantic dynamic state.

Candidate internal modules:

```text
novelty
habituation
arousal
threat
curiosity
social_presence
social_affinity
uncertainty
prediction_error
unfinished_goal_pressure
resource_pressure
memory_activation
turn_taking
orienting
interrupt_control
```

These modules SHOULD interact through sparse recurrent connections, inhibition, gain changes, decay, hysteresis, and refractory behavior rather than existing only as independent scalar variables.

### 4.3 Workspace domain

The Global Workspace integrates a much smaller number of attributed signals into the current cognitive episode.

It does not need every spike, state update, or local sensor feature.

Typical promoted objects:

```text
SalienceEvent
AttentionShift
MemoryActivationCandidate
InterruptRequest
RegulatoryStateSummary
ActionTendency
WakeRequest
PredictionErrorEvent
```

### 4.4 Persona Core domain

Persona Core remains responsible for semantic interpretation and first-person cognition.

It should be able to ask K-Fast questions such as:

```text
what has been salient for the last 3 seconds?
what interrupted me?
what action tendencies are currently competing?
what internal regulatory signals changed?
what memory candidates became active before I woke?
```

Persona Core may influence K-Fast by changing goals, attention targets, expectations, or regulatory priors, but should not manually update low-level state on every tick.

---

## 5. Cross-domain interfaces

The runtime MUST avoid forwarding every fast update upward.

A useful interface resembles a clock-domain crossing in hardware: each subsystem runs at its own natural rate and communicates through sparse, timestamped transitions.

Suggested event envelope:

```text
timestamp
source
signal_type
strength
confidence
duration
state_version
provenance
```

Example:

```text
local audio pathway:
  1 ms  anomaly += 0.01
  2 ms  anomaly += 0.01
  3 ms  anomaly += 0.02
  ...
 18 ms  threshold crossed

K-Fast receives:
  ANOMALY_RISE
  strength: 0.76
  window: 18 ms
  source: phone.audio.front

Workspace later receives:
  "sudden salient sound"
```

The same pattern applies downward. Persona Core can publish a goal or attention bias once; K-Fast can then maintain the resulting dynamics locally.

---

## 6. Regulatory state should modulate computation

Kamimusuhi's digital affect should primarily alter control dynamics rather than appear as decorative labels in prompts.

Candidate regulatory signals:

```text
arousal
fatigue
threat
curiosity
uncertainty
social_affinity
trust
novelty
goal_pressure
resource_pressure
```

Examples of modulation:

```text
threat increases
    -> sensory gain increases
    -> interrupt threshold decreases
    -> exploration decreases
    -> Persona wake threshold decreases
```

```text
fatigue increases
    -> low-value promotion decreases
    -> expensive cognition threshold increases
    -> consolidation/sleep pressure increases
```

```text
curiosity increases
    -> novelty pathway gain increases
    -> exploratory action tendency increases
    -> memory/search prefetch budget may increase
```

Persona Core may later produce a semantic interpretation such as "something feels off" or "I'm curious", but that interpretation is downstream of the regulatory effect rather than its sole implementation.

---

## 7. Fly-inspired design principles to borrow

Kamimusuhi SHOULD borrow principles, not topology.

Useful candidate principles:

1. **Sparse recurrent connectivity** rather than dense all-to-all central computation.
2. **Local sensorimotor loops** that continue without central semantic supervision.
3. **Ascending and descending pathways** between body-local and central layers.
4. **Competition and inhibition** among action tendencies.
5. **Persistent internal state** with decay and history dependence.
6. **Low-dimensional control handles** between rich recurrent dynamics and downstream controllers.
7. **Strong topology priors**: important behavior may be encoded in connection structure, not only in expensive per-node computation.
8. **Closed-loop evaluation**: outputs alter future inputs.
9. **Ablation and causal stimulation** to verify that internal mechanisms matter.

Kamimusuhi SHOULD NOT assume that a fruit-fly connectome is an appropriate graph for language, social identity, autobiographical memory, or human-scale planning.

---

## 8. First K-Fast prototype

The first prototype SHOULD be much smaller than a full fly connectome.

Suggested scale:

```text
1,000 - 10,000 simple nodes
```

Possible rough allocation for a ~5k-node prototype:

```text
sensory novelty / habituation     ~800
social presence / turn taking     ~500
memory activation                 ~800
goal / action competition         ~600
regulatory state                  ~300
orienting / coordination          ~800
integration / broadcast           ~700
misc recurrent / reserve          ~500
```

These numbers are only a starting hypothesis.

Candidate node state:

```text
potential
threshold
decay
refractory
gain
connections[]
```

Candidate controller families to benchmark:

- deterministic FSM / rules;
- tiny conventional recurrent network;
- LIF / SNN recurrent network;
- DLGN / LDLGN;
- DWN / LUT network;
- hybrid event/state machine;
- later FPGA / neuromorphic implementation.

No backend is privileged by analogy.

---

## 9. Prototype outputs

The first K-Fast implementation should emit a small, explicit control vocabulary.

Candidate outputs:

```text
WAKE_K_EDGE
WAKE_PERSONA
THINK_MORE
PREFETCH_MEMORY
ORIENT_VISION
ORIENT_AUDIO
BACKCHANNEL
INTERRUPT_OUTPUT
HOLD_OUTPUT
IGNORE_EVENT
RAISE_SALIENCE
LOWER_SALIENCE
EXPLORE
REST_REQUEST
CONSOLIDATION_REQUEST
```

These are proposals or bounded control signals, not unrestricted capabilities.

Consequential actions remain subject to deterministic capability and policy gates.

---

## 10. Runtime implementation direction

The architecture should keep behavior contracts independent from the execution backend.

Possible path:

```text
reference semantics
Python / PyTorch
        |
        v
trace-compatible native implementation
Rust / C++ / SIMD
        |
        +--> optional GPU / WebGPU / Metal
        |
        +--> optional CUDA
        |
        +--> optional FPGA
        |
        +--> optional neuromorphic hardware
```

Because K-Fast is intended to be always-on, a native implementation is likely appropriate once the dynamics stabilize and profiling justifies migration.

Rust is a strong candidate for the orchestration/state runtime because the rest of Kamimusuhi is already moving toward a native Rust substrate, while backend-specific kernels can remain replaceable.

---

## 11. Evaluation

The first K-Fast experiments MUST compare against simpler baselines.

Primary metrics:

### Responsiveness

- event-to-orient latency;
- event-to-output-inhibition latency;
- event-to-K-Edge wake latency;
- event-to-useful-backchannel latency.

### Semantic-compute efficiency

- Persona Core wakeups per hour;
- unnecessary semantic invocations avoided;
- memory-prefetch precision;
- semantic-token/compute reduction under continuous sensing.

### Temporal behavior

- habituation quality;
- persistence under weak repeated evidence;
- oscillation/flapping rate;
- recovery after interruption;
- stability during long idle periods;
- state convergence after bursts.

### Mechanistic validity

- pathway ablation effects;
- counterfactual stimulation results;
- trace reproducibility;
- backend parity;
- state attribution/provenance completeness.

### Resource cost

- CPU/GPU utilization;
- RAM/VRAM footprint;
- energy per wall-clock hour;
- bandwidth crossing between timing domains.

---

## 12. Identity and memory boundary

K-Fast is **not the self**.

It may contribute temperament-like dynamics, habits, body-level individualization, and persistent regulatory tendencies, but canonical identity remains governed by the Continuity Kernel, Persistent Self State, autobiographical/relationship memory, and Persona Core lineage.

K-Fast state MUST NOT silently become an unaudited second personality database.

Long-lived learned K-Fast parameters should therefore be:

- versioned;
- attributable to training/experience provenance;
- rollback-capable;
- associated with embodiment/runtime compatibility;
- evaluated for behavioral drift.

---

## 13. Relation to live interaction latency

This architecture directly addresses the perceived timing problem of live AI.

A semantic model may need hundreds of milliseconds or seconds to produce a good response. The organism should nevertheless be able to react on faster timescales through:

- immediate gaze/orientation;
- output interruption;
- short backchannels;
- turn-taking cues;
- attention shifts;
- posture/expression changes;
- memory prefetch;
- wake/escalation preparation.

The desired experience is therefore not "make the LLM infinitely fast" but:

> **make the organism responsive at fast timescales while slower semantic cognition continues in parallel.**

---

## 14. Near-term implementation consequence

The current runtime should reserve explicit contracts for a future K-Fast layer even before the neural implementation exists.

Suggested logical boundary:

```text
SensoryEvent / ReflexEvent
        |
        v
FastSignalEncoder
        |
        v
FastController
        |
        +--> FastStateSnapshot
        +--> ControlSignal
        +--> WakeRequest
        +--> MemoryPrefetchHint
        |
        v
WorkspaceAdapter
```

Version these interfaces independently from any SNN/LIF/DLGN backend.

A deterministic or tiny-state-machine implementation can satisfy the interface first. A fly-inspired recurrent backend can then be introduced and benchmarked without redesigning higher cognition.

---

## 15. Current conclusion

Kamimusuhi should treat cognition as a **multi-timescale nervous system**, not as a single language model with peripherals.

The provisional hierarchy is:

```text
Persona Core
  semantic / self / social / deliberative cognition
        ^
        |
Global Workspace / K-Edge
  integration / attribution / routing
        ^
        |
K-Fast
  fly-inspired sparse recurrent regulation
        ^
        |
PNL
  local sensorimotor / machine reflexes
        ^
        |
sensors / body / runtime
```

The first research target is not biological realism. It is to establish whether a small, sparse, continuously active temporal substrate can:

1. improve reaction timing;
2. reduce unnecessary semantic-model wakeups;
3. create stable history-dependent regulatory state;
4. improve live turn-taking and embodied behavior;
5. remain mechanistically testable and cheap enough to run continuously.

If those goals are met, K-Fast can later scale in node count, recurrent complexity, learning/plasticity, and hardware specialization without forcing Persona Core to scale proportionally.
