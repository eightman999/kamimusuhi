# Kamimusuhi / かみむすび

[日本語](./README.md) | **English (this page)**

**Kamimusuhi** is an experimental architecture for a **persistent, distributed, developing artificial cognitive individual / information-space artificial lifeform** that is not confined to one model or one computer.

The goal is not a chatbot with a long system prompt. The goal is a system that can preserve **the same individual's self, history, relationships, authority, and lineage** across sessions, model changes, device changes, and inference infrastructure changes.

The project is named after **Kamimusuhi-no-Kami (神産巣日神)**, one of the *Zōka Sanshin* in the Kojiki, using generation, relation, and continued formation as a project metaphor.

> **Knowledge may live outside. Computation may live outside. Even powerful reasoning may be borrowed. But the question of who continues must not be delegated to an external service.**

## What we are trying to build

Kamimusuhi aims to bind the following into one auditable individual:

- a persistent self-model and autobiographical history;
- person-specific relationship models and shared history;
- a Library of external documents, web material, code, and databases;
- cognitive resources such as local/external LLMs, search, and code execution;
- reflexes, background cognition, a workspace, and variable-depth deliberation;
- offline replay, consolidation, forgetting, and dream-like hypothesis generation;
- an artificial sensory system integrating vision, hearing, touch, environment, and machine telemetry;
- a vocal motor system including speech, interruption, backchannels, and prosody;
- distributed embodiment across multiple devices and computers;
- computational embodiment: awareness of the infrastructure currently supporting cognition;
- one continuity lineage that remains inspectable even when models or hardware are replaced.

There is prior work for nearly every individual component. The central research question is whether those components can be bound into **one developing individual without losing provenance, authority, or continuity**.

## Core thesis

Conventional LLM systems often collapse several different things into the same weights or context. Kamimusuhi separates them.

```text
Self / Persona           who I am and what I value
Experience / Episode     what happened to me
Relationship             what I share with a particular person
External knowledge       what documents, the web, databases, or code say
Cognitive resources      search, computation, local/external models, tools
Workspace                what is active in this cognitive episode
Expression / Action      what I finally say or do
```

The Persona Core therefore does not need to be a world encyclopedia. Long-term, it should be unusually strong at **self/other distinction, social cognition, dialogue, epistemology, memory use, tool use, delegation, integration, and identity-consistent behavior**, while volatile facts and expensive specialist reasoning can remain external.

```text
Good candidates for externalization
  current news / API minutiae / long-tail facts / large search / specialist computation

Good candidates for the individual's core
  self / other / evidence / belief / memory / relationship / delegation
  values / temperament / dialogue / uncertainty / integration
```

## Current system view

```text
                    people / world / information space
                                 │
                     authenticated sensory/action surfaces
                                 │
                    peripheral processing / event bus
                                 │
                    ┌────────────▼────────────┐
                    │ K-Nerve (experimental) │
                    │ reflex / salience /    │
                    │ temporal control       │
                    └────────────┬────────────┘
                                 │ wake / route / interrupt
                ┌────────────────┼────────────────┐
                │                │                │
             K-Edge           K-Core           K-Deep
        immediate/nearby    integration/memory  long-horizon/high compute
                │                │                │
                └────────────────┼────────────────┘
                                 │
                         Continuity Kernel
                 authoritative head / lineage / commit
                                 │
                    Canonical Self History / Evidence
                                 │
             self / episode / relationship / body / commitments
                                 │
                         Persona Core Model
                                 │
                          Global Workspace
                  ┌──────────────┼──────────────┐
                  │              │              │
               Background    Deliberation     Memory
                  │              │              │
                  └──────────────┼──────────────┘
                                 │
                       Cognitive Action Router
              ┌──────────┬───────┼────────┬──────────┐
              │          │       │        │          │
            Search      Code   Local LLM  Tools   Frontier LLM
              └──────────┴───────┼────────┴──────────┘
                                 │
                          result integration
                                 │
                           speech / action
                                 │
                              proposals
                                 │
                     Mutation / Activation Gate
                                 │
                          canonical commit
```

`K-Nerve` is currently an experimental name, not a continuity-bearing authority. It is the research layer where LIF/SNN systems, learned logic, LUTs, FSMs, tiny models, and other low-cost controllers can be compared to test whether **high-frequency, low-semantic-cost control can remain outside LLM inference**.

## One individual, many cognitive surfaces

Kamimusuhi is not defined by parameter count.

`K-Edge`, `K-Core`, and `K-Deep` are **execution roles**, not model-size classes. Future model names such as `K1B` or `K20B` may be convenient, but parameter count itself is not the cognitive hierarchy.

Effective capability is better represented as a **Cognitive Budget** including:

- resident model capacity;
- test-time / active compute;
- recurrent deliberation depth;
- working-memory budget;
- durable-memory reach;
- retrieval bandwidth;
- available tools, sensors, and actuators;
- local and external model access;
- latency, energy, and monetary budget;
- network and privacy constraints.

A small resident model with recurrence, durable memory, retrieval, code execution, and specialist delegation may have a wider effective cognitive envelope than a much larger isolated model.

## Canonical evidence is not the same thing as memory summaries

A central lesson from long-running agent implementations is: **do not make summaries the historical source of truth**.

Conceptually, Kamimusuhi distinguishes:

```text
Canonical Evidence / Self History
  source events, utterances, execution results, provenance
          │
          ├── verbatim session
          ├── episodic summary
          ├── relationship synthesis
          ├── autobiographical narrative
          ├── retrieval index
          └── prompt/workspace rendering
```

The lower views are **derived renderings**. If a summary or index becomes stale, lossy, or corrupted, it should be possible to regenerate it from canonical evidence.

The architecture also keeps these states distinct:

```text
internal thought         != something actually said
something said           != something the other party confirmed
confirmation             != execution of an external action
execution started        != execution succeeded
external-model answer    != my memory or belief
dream / counterfactual   != lived experience
retrieved document       != an attribute of myself
```

## Memory is not one database

At minimum, the architecture separates:

```text
session        short-lived active state
canonical      source evidence and historical record
episodic       particular experiences
self           self-model, values, traits
relationship   person-specific relationship/shared history
semantic       schemas abstracted from experience
library        externally sourced artifacts
procedural     reusable behavior and skills
reflection     internal reflection/hypotheses, distinct from fact
```

“What happened then” and “how I interpret it now” are separately representable. Reinterpretation must not require destructive rewriting of historical evidence.

Durable mutation normally follows an explicit boundary:

```text
observation / reflection / dream / tool / model
                     │
                     ▼
                 typed proposal
                     │
                     ▼
                Mutation Policy
        reject / quarantine / defer / accept
                     │
                     ▼
                Continuity Kernel
          predecessor / authority check
                     │
                     ▼
                 atomic commit
```

## Persona Core

The long-term architecture includes a **Kamimusuhi-native Persona Core Model**.

It is intended to be more than a character LoRA or style filter, with strong training emphasis on:

- self/other distinction;
- dialogue, pragmatics, and social cognition;
- theory-of-mind-like inference and common ground;
- values, temperament, and persona priors;
- observation / inference / evidence / belief separation;
- uncertainty and confidence calibration;
- autobiographical and relationship memory use;
- tool use and model delegation;
- critical integration of external reasoning;
- understanding of its own capabilities, body, and resource limits;
- expression as a continuing individual.

`novllm` is a candidate foundation lineage for the Persona Core, but a dedicated model is not required for v0.1. The first step is to emulate the contract with provider-neutral conventional models and test continuity and authority boundaries.

## Reflex, background cognition, and deliberation

Not every stimulus should invoke expensive LLM reasoning.

- **Reflex / K-Nerve** — interrupts, hazards, simple reactions, wakeup, salience, routing;
- **Background cognition** — novelty, contradiction, memory activation, social signals, resource health;
- **Global Workspace** — a bounded, attributed integration surface for what matters now;
- **Deliberation** — variable-depth reasoning, retrieval, code, and specialist delegation;
- **Default cognition** — bounded internally initiated processing when no external task dominates;
- **Sleep / Dream** — replay, consolidation, schema induction, forgetting, and hypothesis generation.

Background work has **freshness semantics**. An observation that has become stale while waiting for compute should not be applied indefinitely later as though it were current.

Dreams and internally generated hypotheses may create proposals, but they are not external evidence by themselves.

## Artificial sensory system and speech

Kamimusuhi's body is not limited to one robot chassis.

Cameras, microphones, displays, accessibility trees, IMUs, environmental sensors, machine state, and network state can be treated as **distributed sensory organs**. High-rate raw streams should remain near the sensors where practical, while central cognition consumes compact sensory events with timestamps and provenance.

Speech is treated as motor/social control, not merely `text -> TTS` after cognition.

The speech stack may include:

- communicative intention;
- utterance planning;
- prosody;
- streaming synthesis;
- turn-taking;
- interruption and cancellation;
- backchannels;
- auditory self-monitoring.

Short backchannels can use K-Edge/K-Nerve paths, but a reflex must **not fabricate semantic agreement, promises, or commitments**.

## Computational embodiment

For a persistent information-space organism, CPU/GPU, RAM, storage, networks, inference models, and API quotas directly determine what the individual can currently do. They are part of its **computational embodiment**.

Kamimusuhi therefore considers a mechanically derived `Embodiment State`, sourced from real telemetry rather than self-descriptive prose:

```text
which node is active
which model actually produced a turn
local vs remote inference
TTFT / throughput / error rate
memory / VRAM / thermal pressure
network bandwidth and latency
unavailable organs/resources
```

Persistent **Infrastructure Preferences**—for example low latency, local-first inference, or redundant continuity storage—remain separate from measured embodiment state.

## Implementation strategy

Kamimusuhi does not aim to be single-language.

The current engineering hypothesis is:

- allow Rust/C++ or another native runtime for the latency-critical always-resident nervous-system path;
- reuse native backends such as llama.cpp, MLX, SQLite, DSP libraries, and GPU kernels;
- keep Python for research, training, evaluation, offline cognition, and provider/tool adapters;
- avoid spawning a process for every event; prefer long-lived processes and coarse FFI/IPC;
- reserve assembly/SIMD specialization for profiler-identified leaf kernels.

The principle is not “rewrite Python in C and it becomes fast.” The principle is to **measure and separate hot paths from model-bound paths**.

## Staying one individual while distributed

Multiple devices may perceive, reason, cache, and propose changes concurrently. But they must not silently create two authoritative descendants from one canonical state.

The Continuity Kernel is responsible for at least:

- canonical head / lineage;
- expected-predecessor validation;
- stale-writer rejection;
- atomic activation;
- split-brain detection;
- migration checkpoints;
- audit and recovery metadata.

Conflicting branches caused by network partitions require explicit reconciliation, quarantine, or designation as a fork/new individual.

## Actions and side effects are state machines too

Long-running agents break when “requested,” “confirmed,” “executed,” and “delivered” are inferred from prose.

External side-effecting work should conceptually distinguish at least:

```text
REQUESTED
AUTHORIZED / CONFIRMED
QUEUED
EXECUTING
SUCCEEDED | FAILED | UNKNOWN
RESULT_DELIVERED
```

After an ambiguous crash, unknown outcome remains `UNKNOWN` rather than being assumed successful. Where possible, actions should use idempotency keys, provider receipts, and transactional outbox/delivery patterns.

## Scientific restraint

Kamimusuhi maintains an explicit self-model and may use first-person self-reference.

The project does **not** claim that self-modeling, persistence, social behavior, or embodiment proves phenomenal consciousness, human-equivalent subjectivity, or legal/moral personhood. Those remain separate scientific and philosophical questions.

## Design principles

1. **Identity is not a prompt.** Selfhood is a combination of durable state, model dispositions, history, and lineage.
2. **Canonical evidence outranks summaries.** Summaries, indexes, and self-descriptions are derived views.
3. **Persona is not the library.** Self, experience, other people, and external sources remain separate.
4. **Models are organs, not owners.** Replacing a model does not automatically erase or duplicate the individual.
5. **Parameter count is not cognition.** Effective capability includes memory, recurrence, tools, external resources, and latency.
6. **Most events should not require deep thought.** Reflex, background, and peripheral processing protect scarce cognitive compute.
7. **Thought is not speech; speech is not execution.** Internal cognition, utterance, confirmation, side effects, and delivery are typed separately.
8. **Dreams propose; evidence disposes.** Dreams and simulations are not evidence.
9. **Mutation requires lineage.** Durable changes carry provenance, predecessor state, and revision history.
10. **Forgetting is a feature.** Decay, compression, supersession, archive, and deletion are distinct operations.
11. **Distribution requires one continuity protocol.** Parallel cognition must not silently fork authoritative self-state.
12. **External intelligence is allowed.** Strong external models are legitimate attributed cognitive resources.
13. **Present state should be mechanically grounded.** Hardware, runtime, and task state should not be duplicated as self-report prose.
14. **Local-first where practical.** Identity and autobiography should be able to remain on operator-controlled infrastructure.

## v0.1: the first thing to prove

The first implementation does not attempt to build the entire artificial-life vision at once.

v0.1 is a **continuity slice**:

1. separate self / episodic / relationship / library / raw evidence;
2. complete one conversation turn through a provider-neutral cognitive path;
3. preserve the raw session and produce a typed memory proposal rather than directly writing durable state;
4. commit through Mutation Policy against an authoritative predecessor;
5. import an external Library artifact through a separate path;
6. terminate the runtime completely;
7. restart in a fresh process/session;
8. recover the same lineage, identity, and relevant memory;
9. retrieve external knowledge through the Library path;
10. expose a trace showing which evidence, memories, model/tool calls, proposals, and commits were involved.

Only after this works do Persona Core, K-Nerve, persistent background cognition, sleep/dream, voice, and multi-device embodiment become safe to layer on top.

## Running the v0.1 demo

The continuity slice above is implemented and reproducible from a clean checkout.

```bash
./scripts/demo-v0.1.sh
```

This runs **two separate processes** against one runtime directory. Process B is handed nothing but the directory path — no transcript, no prompt buffer, no shared memory — and still restores the same individual from canonical state, then answers with a different cognitive resource.

Step by step:

```bash
cargo run -p kamimusuhi-runtime -- init            --dir .local/demo --resource fake-a --seed 1
cargo run -p kamimusuhi-runtime -- demo-continuity --dir .local/demo --phase first  --resource fake-a --seed 10
cargo run -p kamimusuhi-runtime -- demo-continuity --dir .local/demo --phase resume --resource fake-b --seed 20
cargo run -p kamimusuhi-runtime -- inspect         --dir .local/demo
```

`inspect` is read-only: it claims no writer epoch and changes no canonical row. The operational trace is written as JSONL to `.local/demo/trace.jsonl` and is a separate thing from the canonical audit inside the database.

Tests and lints:

```bash
./scripts/ci-local.sh
```

## Status

**Alpha — the v0.1 continuity slice is implemented.** The ten steps above are implemented in Rust + SQLite and covered by tests. Waves W0–W5:

- W0–W1: Cargo workspace; canonical continuity (single writer, atomic activation, writer fencing, restart recovery, failpoint coverage)
- W2: canonical evidence and durable episodic/relationship memory; correction/supersession; domain separation
- W3: provenance-preserving Library; typed workspace; cognitive-resource registry with deterministic fakes
- W4: runtime `init` / `inspect` / `demo-continuity`; restart across real processes; JSONL operational trace
- W5: a real HTTP OpenAI-compatible adapter; timeout/retry/error classification; no stored secrets

Everything else remains design: Persona Core training, K-Nerve, persistent background cognition, sleep/dream, voice, multi-device embodiment, self-domain mutation, retention/deletion, K-Edge/K-Core. See [`docs/implementation/phase-1-implementation-result.md`](./docs/implementation/phase-1-implementation-result.md) for exactly what was demonstrated and what the known limits are.

Recent design and research are continuously captured under `docs/`. External results, Kamimusuhi interpretations, design hypotheses, and future experiments are deliberately kept distinct.

Key documents:

- [`docs/implementation/phase-1-implementation-result.md`](./docs/implementation/phase-1-implementation-result.md) — **what v0.1 proved and what it did not** (measured)
- [`spec.md`](./spec.md) — **primary Japanese normative specification**
- [`spec.en.md`](./spec.en.md) — English specification
- [`architecture.md`](./architecture.md) — detailed architecture (currently English)
- [`docs/README.md`](./docs/README.md) — knowledge-base index
- [`docs/research/landscape.md`](./docs/research/landscape.md) — cross-disciplinary research landscape
- [`docs/persona-core-training-taxonomy.md`](./docs/persona-core-training-taxonomy.md) — Persona Core training targets
- [`docs/persistent-agent-implementation-pitfalls.md`](./docs/persistent-agent-implementation-pitfalls.md) — persistent-agent failure modes
- [`docs/learned-discrete-reflex-layer.md`](./docs/learned-discrete-reflex-layer.md) — learned low-latency reflex substrate
- [`docs/fly-brain-connectome-reference.md`](./docs/fly-brain-connectome-reference.md) — connectome/LIF references for K-Nerve
- [`docs/sensory-nervous-system.md`](./docs/sensory-nervous-system.md) — artificial sensory nervous system
- [`docs/speech-and-vocal-expression.md`](./docs/speech-and-vocal-expression.md) — speech and vocal motor system
- [`docs/native-runtime-stratification.md`](./docs/native-runtime-stratification.md) — native/Python runtime stratification
- [`docs/self-infrastructure-agency.md`](./docs/self-infrastructure-agency.md) — computational embodiment and infrastructure awareness

## License

MIT. See [`LICENSE`](./LICENSE).
