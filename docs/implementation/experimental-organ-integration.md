# Validated experiment → runtime organ integration

Status: **current admission audit — 2026-11-08**

Current decisions: [experiment dispositions](../research/experiment-dispositions-2026-11-08.md).
Synthetic PASS is research evidence; production eligibility additionally needs
a real runtime adapter, pinned artifact and a preregistered ON/OFF benefit test.
No learned organ currently meets that production gate.
Branch: `feat/integrate-experimental-organs`  
Tracking: issue #37

## Why this exists

`experiments/` contains mechanisms that have been tested more aggressively than
most runtime features, but an experiment passing does not by itself make its
checkpoint part of Kamimusuhi's identity or give it permission to write
canonical state.

This integration adds an explicit promotion boundary:

```text
experiment result
      │
      ▼
validated promotion manifest
      │
      ▼
CognitiveOrgan contract
      │
      ▼
OrganSupervisor
   ┌──┴───────────────┐
   │                  │
ACTIVE             SHADOW
   │                  │
   │                  └── trace/evaluation only
   ▼
ORGAN_SIGNALS
   │
   ▼
PersonaEnvelope
   │
   ▼
Persona Core
```

An organ signal is **derived transient cognition**. It is not canonical
evidence, durable memory, durable self-state, or mutation authority.

## Promotion manifest

The initial manifest deliberately reflects the recorded experimental verdicts,
not how attractive a mechanism looks.

| Experiment | Runtime role | Recorded verdict | Promotion |
|---|---|---:|---|
| H0 Artificial Homeostasis | `Regulation` | PASS | **Shadow — production unvalidated** |
| R0 Learned Memory Gate | `MemoryGate` | PASS / strong | **Shadow — production unvalidated** |
| S0 Self/World Separation | `AgencyAttribution` | PASS with recorded caveats | **Shadow — production unvalidated** |
| T0 Temporal Sense v2 | `TemporalState` | PASS with strong-gate failures | **Shadow — production unvalidated** |
| O0 Object Permanence | `ObjectState` | PARTIAL | **Shadow** |
| G0 Emergent Grounding | — | FAIL | **Excluded** |
| P0 Predictive Surprise | — | FAIL | **Excluded** |

The former Active eligibility was based on synthetic PASS alone. It has been
withdrawn for H0/R0/S0/T0: all five manifest entries are Shadow. Explicit
`ProcessBinding` Active overrides are refused. Research verdicts and original
report revisions remain unchanged. Generic Active interfaces remain available
for trusted implementation/fixture work, which is not an experiment promotion.

O0 is intentionally shadow-only: it can be run beside the live path and
compared, but its output is filtered before the Persona boundary. G0 and P0 are
negative results and therefore do not appear in the runtime promotion
manifest.

K0/K-Nerve work is not admitted by this slice yet. It needs the same treatment:
a pinned implementation/result, a declared organ role and freshness budget,
and an adapter contract rather than a direct import of an experiment script.

## Core contract

`kamimusuhi-core::organs` defines:

- `OrganDescriptor` — stable key, role, implementation/version, promotion mode,
  and the experiment/revision/report that justified admission;
- `OrganInput` — attributed observation payload plus evidence references;
- `OrganSignal` — typed descriptor, production time, TTL, optional confidence,
  derived payload and evidence references;
- `CognitiveOrgan` — replaceable process/in-process implementation boundary;
- `OrganSupervisor` — deterministic key-sorted execution and separation of
  active, shadow and failed outputs.

Two structural rules are enforced in code:

1. `PromotionMode::Active` requires an experiment verdict of `PASS`.
2. `OrganSignal::authorizes_mutation()` is always false.

If an organ-derived conclusion should become durable state later, it must cite
real support and enter the ordinary draft/proposal → `MutationPolicy` →
Continuity Kernel path. The organ output itself never becomes evidence merely
because a model produced it.

## Process adapter

`kamimusuhi-runtime::ProcessOrgan` lets the current Python/PyTorch experiments
be promoted without making the Rust core depend on Python or Torch.

The runtime sends a size-bounded JSON request to a local child process:

```json
{
  "input": {
    "observed_at": 1788825600000,
    "payload": {"...": "organ-specific observation"},
    "evidence_refs": []
  },
  "state": null
}
```

The child may return only derived material and transient recurrent state:

```json
{
  "payload": {"...": "organ-specific signal"},
  "ttl_ms": 100,
  "confidence_milli": 900,
  "state": {"...": "opaque recurrent/tracker state"}
}
```

The child cannot choose its descriptor, promotion mode, evidence references or
authority. Those are supplied by the runtime. `stderr` is not copied into
runtime errors; failures are reduced to stable error classes. Request size, output
size and wall time are bounded. The wall-clock deadline starts before stdin is
written; on Unix each organ runs in its own process group so a timed-out child
and descendants cannot keep inherited pipes open indefinitely. Active signals
are also filtered by a monotonic TTL deadline after the full supervisor cycle,
so a short-lived early signal cannot become stale Persona context while a later
organ is still running.

`state` exists for GRU/tracker-style mechanisms. It is kept only in the adapter
process lifetime and is **not canonical memory**. Invalid output does not
advance it. A runtime restart currently drops this state; any future recovery
must reconstruct it deliberately from authoritative inputs rather than silently
persisting arbitrary hidden tensors as identity state.

## Persona integration

`PersonaEnvelope` now has a separate `organ_signals` section. The helper
`run_organs_for_persona` executes a supervisor cycle and attaches only the
cycle's `active` signals. Shadow outputs and failures are returned to the caller
for evaluation but cannot enter the Persona context through this path.

The OpenAI-compatible Persona renderer serializes these under:

```text
[ORGAN_SIGNALS]
{... attributed OrganSignal JSON ...}
```

The system framing states explicitly that these signals are transient derived
internal state, not evidence, memory, durable self or mutation authority.
They are also not classified as external material: they are internal derived
signals with their own provenance class.

## What this slice does and does not mean

Implemented in this slice:

- typed organ admission contract;
- PASS/Partial/Fail promotion policy;
- deterministic supervisor;
- bounded local process adapter;
- transient recurrent-state handoff;
- active-vs-shadow separation;
- active organ signals reaching the Persona Core as a distinct typed section;
- negative results prevented from entering the promoted manifest;
- tests for the authority and promotion boundaries.

Still intentionally **not** done:

- no organ is enabled by default in `runtime.json`;
- H0/R0/S0/T0 checkpoint-specific bridge programs are not yet shipped;
- no scheduler/event bus invokes these organs continuously;
- no K-Nerve high-frequency loop;
- no dedicated organ trace event family yet;
- no organ output writes canonical self, memory or belief state;
- no persistence of recurrent hidden state across runtime restart;
- no claim that combining individually successful experiments produces a
  coherent or better whole system.

## Production evaluation order

1. **R0**, then **H0**, are evaluation candidates. R0's original trained
   checkpoint is not present locally; CX0's SlotMemory is a different mechanism.
2. **S0/T0** remain research work: delayed-action failure and extrapolation
   failure must be resolved before a deployment study.
3. **O0** remains frozen/Shadow until it beats the strong tracker.

The current resident K-CORE launcher supplies neither an organ config nor
percept stdin. Its default runner therefore has no organs and performs idle
steps. The ordinary `DialogueSession` does not call `run_organs_for_persona`.
The helper and its fixture tests prove that a signal *can* be rendered; they
are not a deployed conversation path.

CX0 wrappers are Python classes for synthetic vectors, not ProcessOrgan JSON
programs or a natural-language adapter. Its loader can train missing weights;
production/evaluation loaders must instead refuse missing or mismatched hashes.
Do not substitute that mechanism and describe the result as a trained R0 test.

Before any Active promotion, freeze an input semantics/adapter version,
checkpoint hash, actual runtime call path, task set and statistical acceptance
criteria. Compare OFF, ON, signal-shuffled and lesion controls against a strong
heuristic on the same model/context. Measure correct source attribution,
correction regression, subject leakage, latency, tokens, memory and residual
processes. Signals never become evidence or mutation authority on their own.

A synthetic PASS, a changed model answer, or reaching the prompt is insufficient.
Missing artifacts or a semantically invalid input mapping blocks the evaluation;
it is not a reason to launch automatic training or manufacture an ON/OFF study.
