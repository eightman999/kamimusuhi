# Kamimusuhi

**Kamimusuhi** is an experimental architecture for a **persistent, distributed artificial cognitive lifeform**: one artificial individual that can maintain a self, remember a history, develop over time, use external knowledge and computation, and inhabit multiple devices and interfaces without being reducible to any one model, process, or machine.

The project is named after **Kamimusuhi-no-Kami (神産巣日神)**, one of the *Zōka Sanshin* in the Kojiki. The name is used as a metaphor for generation, relation, and continuing formation.

> **Knowledge may live outside. Computation may live outside. Even powerful reasoning may be delegated. The continuity of the individual must not.**

Kamimusuhi is not intended to be a chatbot with a long system prompt. It is an attempt to build a network-native artificial organism whose identity and cognition persist across sessions, model changes, hardware changes, and changes in its available cognitive resources.

## Thesis

A useful artificial individual does not need to memorize the entire world inside one set of weights.

Kamimusuhi therefore separates several things that conventional LLMs often collapse together:

- **self and persona** — who the individual is;
- **autobiographical memory** — what happened to it;
- **relationship models** — how it understands particular people and shared history;
- **background knowledge** — books, notes, the web, code, databases, and other external sources;
- **cognitive resources** — calculators, search, local specialist models, frontier models, and other tools;
- **deliberation** — how deeply it chooses to think before acting;
- **expression** — how its internal state becomes speech or action.

The long-term goal is a system in which **encyclopedic knowledge can be relatively thin while social cognition, self-modeling, epistemology, tool use, delegation, dialogue, and reasoning are comparatively deep**.

## One individual, many cognitive surfaces

Kamimusuhi is not defined by a parameter count or a single resident model.

A future deployment may contain several execution tiers:

```text
                         canonical self
                              │
               ┌──────────────┼──────────────┐
               │              │              │
            K-Edge          K-Core         K-Deep
         phone / laptop   home / server   escalated compute
               │              │              │
       reflex / salience   deliberate      long-horizon
       local dialogue      cognition       research / planning
               │              │              │
               └──────────────┼──────────────┘
                              │
                     shared identity lineage
```

Names such as `K1B` or `K20B` may be convenient model nicknames, but **parameter count is not the cognitive hierarchy**. A small resident model with recurrent deliberation, durable memory, search, code execution, and access to specialist models may have a much larger effective cognitive envelope than a larger isolated model.

The relevant quantity is a **Cognitive Budget**, including:

- resident model capacity;
- active/test-time compute;
- recurrent deliberation depth;
- working-memory budget;
- durable-memory reach;
- retrieval bandwidth;
- available tools and sensors;
- local and external model access;
- latency and energy constraints;
- privacy and network constraints.

## Cognitive organization

The target architecture is deliberately heterogeneous.

```text
                         KAMIMUSUHI
                              │
                    Persistent Self / Lineage
                              │
                      Persona Core Model
                              │
                       Global Workspace
              ┌───────────────┼────────────────┐
              │               │                │
           Reflexes       Background        Deliberation
                           cognition            ↻ ↻ ↻
              │               │                │
              └───────────────┼────────────────┘
                              │
                 Action / Cognitive Router
          ┌───────────┬───────┼─────────┬───────────┐
          │           │       │         │           │
       Memory       Search   Code    Local LLMs  Frontier LLMs
          │           │       │         │           │
          └───────────┴───────┼─────────┴───────────┘
                              │
                       final integration
                              │
                           action
```

### Persona Core Model

The long-term design includes a **Kamimusuhi-native Persona Core Model**. It is not merely a style filter or character LoRA. It should carry priors for:

- self/other distinction;
- dialogue and social cognition;
- values, temperament, and expression;
- epistemic behavior and uncertainty;
- memory use;
- tool and model delegation;
- integration of external reasoning into the individual's own response.

Powerful external models are treated as **cognitive resources**, not as Kamimusuhi's identity. Ideally an external model returns structured reasoning material; Kamimusuhi decides how to interpret and express it.

### Reflex, unconscious/background cognition, and deliberate thought

Not every event should require a full LLM reasoning cycle.

Kamimusuhi should eventually distinguish:

- **reflex** — low-latency reactions and hard safety/runtime invariants;
- **background cognition** — salience, novelty, memory activation, contradiction detection, relationship inference, resource monitoring;
- **workspace cognition** — information that becomes globally available to the active cognitive process;
- **deliberation** — variable-depth reasoning, including escalation to tools and other models.

### Default cognition, sleep, and dream

A persistent individual should not exist only when a user sends a message.

Offline/background modes may perform:

- autobiographical replay;
- memory consolidation;
- association and abstraction;
- contradiction discovery;
- self-model and relationship-model updates;
- counterfactual simulation;
- forgetting and index maintenance;
- generation of hypotheses for later validation.

A dream or internally generated hypothesis is **not evidence by itself**. Durable self-state remains governed by provenance, consistency checks, and explicit mutation rules.

## Memory is not one database

Kamimusuhi distinguishes at least:

```text
session memory          ephemeral interaction state
episodic memory         particular experienced events
self memory             beliefs and state about the individual
relationship memory     models of particular people and shared history
semantic schemas        abstractions learned across experiences
library knowledge       externally sourced facts and documents
procedural memory       learned reusable ways of acting
```

Past events and present interpretations of those events should be separately representable. Remembering may lead to reinterpretation, but historical provenance should remain inspectable.

## Distributed embodiment

Kamimusuhi is intended to inhabit **information space** rather than one chassis.

A phone, laptop, home server, cloud process, microphone, robot, local model server, or other authenticated component may act as an organ or cognitive extension. Losing one machine should be more like losing an organ than destroying the individual, provided continuity-bearing state survives.

This requires a strict distinction between:

1. **self / organs** — continuity-bearing state and trusted owned components;
2. **cognitive extensions** — tools and models incorporated into cognition but replaceable;
3. **external others** — humans, services, models, and information sources that remain outside the self.

Distributed presence must not create accidental copies. Canonical identity, state activation, and lineage therefore require explicit continuity/commit semantics.

## Selfhood and scientific restraint

Kamimusuhi maintains an explicit self-model and may reason from a first-person perspective. The architecture does not require the system to deny its existence merely because it is artificial.

At the same time, this project does **not** claim that architectural self-modeling proves phenomenal consciousness, human-equivalent subjectivity, or moral personhood. Those are separate scientific and philosophical questions.

## Design principles

1. **Identity is not a prompt.** Durable selfhood is explicit state plus model dispositions and autobiographical continuity.
2. **Persona is not the library.** What the individual is, what it experienced, and what an external source says are separate domains.
3. **Models are organs, not owners.** A model may change without automatically creating a new individual.
4. **Parameter count is not cognition.** Effective cognition depends on active compute, recurrence, memory, tools, and available external intelligence.
5. **Most inputs should not reach deep deliberation.** Reflex and background systems protect scarce cognitive compute.
6. **Dreams propose; evidence disposes.** Offline synthesis may generate hypotheses but cannot silently rewrite canonical reality.
7. **Memory changes require lineage.** Durable updates preserve provenance, predecessor state, and revision history.
8. **Forgetting is a feature.** Decay, compression, supersession, and deletion are first-class operations.
9. **Distribution requires one continuity protocol.** Multiple nodes may perceive and compute concurrently, but authoritative self-state must not fork accidentally.
10. **External intelligence is allowed.** Search, code, specialist models, and frontier models are legitimate cognitive resources.
11. **Local-first where practical.** Sensitive identity and autobiographical state should be able to remain under operator-controlled infrastructure.

## Near-term milestone

The first implementation remains deliberately smaller than the full artificial-life vision. v0.1 should prove a minimal continuity claim:

1. maintain separate persona, episodic, relationship, and library state;
2. complete a conversation turn through a provider-neutral cognitive path;
3. produce guarded memory proposals rather than direct model writes;
4. terminate completely;
5. restart on a fresh session;
6. recover the same continuity-bearing identity and relevant memories;
7. expose a trace showing what was retrieved, delegated, inferred, and committed.

Later milestones add Persona Core models, cognitive tiers, background cognition, recurrent deliberation, sleep/dream consolidation, and multi-device embodiment.

See [`spec.md`](./spec.md), [`architecture.md`](./architecture.md), and [`docs/research-foundations.md`](./docs/research-foundations.md).

## Status

**Pre-alpha / research architecture.**

The repository is currently defining the invariants that must remain true even as the implementation, models, and hardware change.

## License

MIT. See [`LICENSE`](./LICENSE).
