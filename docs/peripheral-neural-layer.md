# Peripheral Neural Layer (PNL)

Status: **provisional architecture decision / living engineering design**

Date: **2026-09-08**

This document defines the intended placement and responsibilities of Kamimusuhi's **Peripheral Neural Layer (PNL, 末端神経層)**. The current design direction is to place learned discrete controllers such as DLGN/LDLGN- or DWN/LUT-like systems primarily in the **peripheral nervous and reflex system**, below language-model cognition.

This is an architectural placement decision, not a commitment to one learning algorithm. DLGN, LDLGN, DWN, finite-state machines, tiny conventional models, and deterministic rules remain competing implementation backends until benchmarks justify a narrower choice.

Related:

- [`learned-discrete-reflex-layer.md`](./learned-discrete-reflex-layer.md) — research note on learned discrete controllers and candidate algorithms;
- [`sensory-nervous-system.md`](./sensory-nervous-system.md) — receptors, sensory event bus, multimodal binding, and reflex arcs;
- [`latency-architecture.md`](./latency-architecture.md) — K-Edge/K-Core escalation and response latency;
- [`native-runtime-stratification.md`](./native-runtime-stratification.md) — native hot-path/runtime placement;
- [`../architecture.md`](../architecture.md) — normative top-level architecture.

---

## 1. Decision

Kamimusuhi SHOULD have an always-available peripheral control layer between high-rate sensing/actuation and semantic cognition.

The preferred architecture is:

```text
raw sensors / body telemetry
          |
          v
receptor-local preprocessing
filter / detect / quantize / compress
          |
          v
+----------------------------------+
| Peripheral Neural Layer (PNL)    |
|                                  |
| stateful low-latency controller  |
| rules / FSM / DLGN / LDLGN /     |
| DWN-LUT / tiny learned policies  |
+---------------+------------------+
                |
       reflex / motor primitive
                |
                v
+----------------------------------+
| deterministic capability /       |
| safety envelope                   |
+---------------+------------------+
                |
                v
             actuator

PNL/reflex result
      |
      +--> attributed ReflexEvent / SensoryEvent
                    |
                    v
              salience / K-Edge
                    |
                    v
             Global Workspace
                    |
                    v
               Persona Core
```

The fundamental rule is:

> **A fast bodily reaction should not require an LLM unless the reaction itself needs semantic interpretation.**

Higher cognition may become aware of a reflex after the physical/digital action has already begun. This is intentional.

---

## 2. Why the learned discrete layer belongs here

The strongest fit for logic-gate/LUT-style learned networks is not general language reasoning but decisions that are:

- high frequency;
- low semantic complexity;
- latency sensitive;
- representable with compact local state;
- repeatedly exercised;
- measurable against clear outcomes;
- useful even when K-Core or network connectivity is unavailable.

Examples include:

```text
tactile slip      -> increase grip
collision risk    -> stop / evade
pose error        -> stabilization primitive
rapid approach    -> protective withdrawal
sound onset       -> orient microphone/camera
moving target     -> gaze/head tracking primitive
battery/thermal   -> local throttling or safe-state request
repeated noise    -> suppress low-value promotion
salient anomaly   -> interrupt / wake K-Edge
```

These are closer to a **peripheral nervous system, spinal reflex, or low-level sensorimotor controller** than to the Persona Core.

The biological analogy is only organizational. The subsystem is judged by latency, robustness, energy use, auditability, and task performance rather than similarity to a human nervous system.

---

## 3. Reflex hierarchy

Not every fast reaction should be learned. Kamimusuhi should separate at least four levels:

```text
R0  hard safety reflex
    deterministic emergency stop / current limit / thermal limit /
    authentication and capability boundaries

R1  learned peripheral reflex
    PNL: body-specific low-latency sensorimotor policy

R2  K-Edge fast cognition
    compact semantic interpretation, local salience, simple dialogue /
    action selection

R3  Persona Core / K-Core deliberation
    meaning, planning, social interpretation, values, long-horizon action
```

R1 MUST NOT bypass R0. A learned controller may propose or select an action only inside a deterministic capability envelope.

This separation also gives a clean failure mode: if the learned controller is unavailable or suspect, the system can fall back to R0 plus a deterministic R1 baseline without losing emergency safety.

---

## 4. PNL responsibilities

The PNL MAY own or participate in:

- near-sensor event detection and suppression;
- compact temporal state;
- reflex selection;
- motor primitives;
- local stabilization;
- orientation / gaze primitives;
- wake/interrupt decisions;
- salience hints;
- bounded prefetch or local routing actions;
- body-specific calibration and adaptation;
- low-cost behavior while K-Edge/K-Core is asleep, busy, offline, or remote.

The PNL SHOULD be able to run continuously without requiring token generation.

It SHOULD prefer compact, attributed inputs rather than unbounded semantic context. Raw streams may remain available to specialized perception components, but the PNL's learned controller should normally consume features such as:

```text
contact_left
slip_probability_bucket
relative_motion_x
relative_motion_y
pose_error_bucket
approach_rate_bucket
sound_direction_bucket
novelty_bucket
local_temperature_bucket
resource_pressure_bucket
refractory_state
```

This makes the controller small enough to train, audit, benchmark, and eventually move onto constrained hardware.

---

## 5. Explicit non-responsibilities

The PNL is **not** the self and is not a miniature Persona Core.

It MUST NOT become the authoritative owner of:

- identity;
- autobiographical memory;
- relationship models;
- values or long-horizon commitments;
- natural-language world interpretation;
- irreversible external actions;
- authentication or authorization policy;
- canonical memory mutation;
- model-lineage authority.

A peripheral learned state may influence behavior, but it must not silently become a second hidden personality database.

---

## 6. Upward reporting: reflex first, interpretation later

Every meaningful learned reflex should be observable from higher layers.

Example:

```text
left-palm slip sensor
      |
      v
PNL selects GRIP_INCREASE_2
      |
      v
capability gate validates bounded grip change
      |
      +--> motor command
      |
      +--> ReflexEvent
              action: GRIP_INCREASE_2
              controller_version: pnl-hand-v7
              local_state_before: ...
              local_state_after: ...
              confidence: ...
              sensor_provenance: ...
              outcome_pending: true
```

K-Edge or the Persona Core can later interpret this as, for example, "the object started to slip, so I tightened my grip." The semantic narrative is an interpretation of the event; it is not required for the reflex to occur.

This preserves both low latency and autobiographical/provenance integrity.

---

## 7. Body-specific learning and individual variation

A learned PNL creates a useful place for **embodied individualization**.

Different bodies, sensors, actuator dynamics, wear, latency, and environments may lead to different optimal reflexes even when the Persona Core is identical.

Possible learned differences include:

- how early a body begins obstacle avoidance;
- preferred stabilization timing;
- gaze-following dynamics;
- grip correction strength;
- habituation to repetitive sensory noise;
- escalation thresholds under uncertainty.

This is a legitimate source of individual behavioral style, but it is **procedural/body-level individuality**, not canonical identity by itself.

PNL versions and learned state therefore SHOULD be associated with an embodiment/body schema and kept separate from the authoritative self/lineage state.

---

## 8. Candidate implementation backends

The interface should allow backend substitution.

Initial candidates:

1. deterministic rules / state machines — mandatory baseline and fallback;
2. decision tree or similarly cheap conventional controller;
3. tiny MLP — conventional learned baseline;
4. DLGN / LDLGN-style learned logic-gate network;
5. DWN / LUT-based learned controller;
6. bit-packed or table-driven temporal controller;
7. later FPGA/MCU/neuromorphic implementations where measurements justify them.

DLGN/LDLGN/DWN are currently **preferred research candidates for R1**, not required production dependencies.

The system MUST benchmark them against simpler baselines. A novel architecture should not be adopted merely because it is biologically suggestive or theoretically fast.

---

## 9. Temporal state

Peripheral control should be allowed to have small local state:

```text
(state[t+1], action[t]) = controller(input[t], state[t])
```

Useful state may include:

- short refractory/cooldown timers;
- recent contact/motion history;
- local confidence;
- habituation;
- short persistence counters;
- stabilization phase;
- actuator saturation state;
- recent-reflex outcome.

This state should be bounded, inspectable where practical, and explicitly versioned.

The PNL does not need a language context window. Its memory is closer to short sensorimotor state than episodic/autobiographical memory.

---

## 10. Training and deployment policy

The preferred path is conservative:

```text
instrument deterministic baseline
        |
        v
collect sensor -> action -> outcome traces
        |
        v
train candidate controller offline
        |
        v
shadow mode
        |
        v
bounded authority inside safety envelope
        |
        v
body-specific adaptation experiments
```

A candidate learned controller SHOULD first run in shadow mode and be compared with the currently active controller.

Required deployment metadata:

- controller version/hash;
- feature-schema version;
- body/embodiment compatibility;
- training-data/derivation manifest;
- benchmark report;
- allowed action capability set;
- fallback controller;
- rollback target.

Unbounded online learning is not the default. Online adaptation MAY be explored only when rollback, drift detection, capability bounds, and outcome logging are in place.

---

## 11. Evaluation

PNL evaluation should use real temporal sensorimotor streams, not only independent classification rows.

Primary metrics:

**Latency**

- sensor-to-decision p50/p95/p99;
- sensor-to-actuator latency;
- K-Edge wake latency where escalation occurs.

**Control quality**

- task success;
- instability/oscillation rate;
- missed-hazard rate;
- unnecessary-reflex rate;
- overshoot/recovery time;
- distribution-shift robustness.

**Resource cost**

- CPU/MCU utilization;
- memory footprint;
- energy per decision where measurable;
- bandwidth avoided by not forwarding raw streams;
- LLM/K-Edge wakeups avoided.

**Observability**

- controller-version attribution;
- input/event provenance completeness;
- reproducibility;
- rollback success.

The practical question is not "how few gates can control a demo?" but:

> **Can a tiny learned peripheral controller outperform deterministic/tree/tiny-MLP baselines for Kamimusuhi's real sensorimotor streams while preserving safety and auditability?**

---

## 12. Near-term implementation consequence

The near-term Kamimusuhi runtime should reserve an explicit interface boundary for the PNL even if v0 uses only ordinary rules.

Suggested logical components:

```text
ReceptorAdapter
  -> PeripheralFeatureFrame
  -> PeripheralController
  -> ReflexProposal
  -> CapabilityGate
  -> ActuatorCommand
  -> ReflexReceipt
  -> SensoryEventBus
```

This prevents the first prototype from hard-wiring all sensorimotor behavior into Python application logic or into the Persona Core prompt, and allows later DLGN/LDLGN/DWN/FPGA experiments without redesigning the higher cognitive architecture.

---

## 13. Current conclusion

As of 2026-09-08, Kamimusuhi treats the **peripheral nervous / reflex system as the primary architectural home for learned discrete control networks**.

What is provisionally adopted:

- an explicit Peripheral Neural Layer below semantic/LLM cognition;
- immediate local reflexes with upward event reporting;
- deterministic safety/capability gates around learned reflexes;
- backend independence;
- body-specific learned adaptation as a research target.

What is **not** yet fixed:

- DLGN vs LDLGN vs DWN vs another backend;
- exact feature representation;
- exact hardware target;
- acceptable online-learning policy;
- hard latency budgets for each embodiment.

Those remain benchmark-driven engineering decisions.
