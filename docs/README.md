# Kamimusuhi Documentation Index

This directory contains research notes, architecture notes, implementation plans, audits, and daily surveys for Kamimusuhi.

## Architecture and runtime

- [`../architecture.md`](../architecture.md) — top-level cognitive and continuity architecture.
- [`multiscale-brain-architecture.md`](./multiscale-brain-architecture.md) — multi-timescale nervous-system design: PNL → K-Fast → Workspace/K-Edge → Persona Core.
- [`latency-architecture.md`](./latency-architecture.md) — latency, wake/escalation, and low-latency interaction architecture.
- [`native-runtime-stratification.md`](./native-runtime-stratification.md) — native/runtime implementation layering.
- [`organ-contracts-and-implementation-plan.md`](./organ-contracts-and-implementation-plan.md) — organ boundaries, contracts, and implementation sequencing.
- [`peripheral-neural-layer.md`](./peripheral-neural-layer.md) — peripheral nervous/reflex layer and learned local control.
- [`sensory-nervous-system.md`](./sensory-nervous-system.md) — artificial sensory nervous system, receptors, event bus, multimodal binding, proprioception, and interoception.
- [`learned-discrete-reflex-layer.md`](./learned-discrete-reflex-layer.md) — DLGN/LDLGN/DWN/LUT/FSM and related low-latency learned-controller candidates.

## Research references

- [`fly-brain-connectome-reference.md`](./fly-brain-connectome-reference.md) — FlyWire/connectome-derived control lessons, K-Nerve/K-Fast implications, closed-loop and mechanistic validation.
- [`fio-system-reference-analysis.md`](./fio-system-reference-analysis.md) — FIO system reference analysis.
- [`persistent-agent-implementation-pitfalls.md`](./persistent-agent-implementation-pitfalls.md) — persistent-agent failure modes and implementation traps.
- [`persona-core-training-taxonomy.md`](./persona-core-training-taxonomy.md) — Persona Core training and taxonomy notes.
- [`model-ecology-and-lineage.md`](./model-ecology-and-lineage.md) — model ecology, specialization, and lineage.

## Surveys and audits

- [`daily-surveys/`](./daily-surveys/) — recurring cross-domain research survey notes.
- [`audits/`](./audits/) — architecture and implementation audits.
- [`implementation/`](./implementation/) — implementation planning and wave-specific notes.

## Current architectural through-line

Kamimusuhi is intentionally not defined as a single LLM with memory. The current architecture separates:

```text
sensors / body / runtime
        |
        v
Peripheral Neural Layer (PNL)
        |
        v
K-Fast / fast recurrent regulation
        |
        v
Global Workspace / K-Edge
        |
        v
Persona Core / K-Core
        |
        v
K-Deep / external cognitive resources when needed
```

Fast layers may react before semantic interpretation completes, while canonical identity, autobiographical continuity, and authoritative state remain governed by the continuity/self architecture.
