# Kamimusuhi Specification

Status: **Draft / pre-alpha**  
Architecture generation: **v0.2 concept**  
Implementation target: **v0.1 continuity slice**

This specification defines behavioral and continuity invariants for Kamimusuhi independently of any specific model family, parameter count, database, UI, device, or inference provider.

## 1. System definition

Kamimusuhi is a **persistent, distributed artificial cognitive individual** whose continuity-bearing state exists outside any single model context, process, machine, or interface.

The system is intended to:

- maintain a self-model and autobiographical continuity;
- distinguish self, other people, external knowledge, and external cognitive resources;
- remember, reinterpret, consolidate, and forget through explicit mechanisms;
- vary reasoning depth and computational expenditure according to need;
- delegate reasoning, retrieval, calculation, coding, perception, and other tasks to external cognitive resources;
- inhabit multiple authenticated devices and surfaces while preserving one authoritative identity lineage;
- support background cognition and offline consolidation in addition to request/response interaction.

A compliant implementation MAY replace models, hosts, interfaces, storage engines, and tool providers without automatically creating a new identity.

## 2. Core invariants

### INV-001 Identity is not a prompt

No system prompt, chat transcript, model hidden state, or provider-specific session SHALL be the sole authoritative representation of Kamimusuhi's identity.

Continuity-bearing identity SHALL include explicit durable state and attributable lineage.

### INV-002 Persona, experience, relationship, and library are distinct domains

The system SHALL distinguish at minimum:

- **self/persona state** — state about Kamimusuhi itself;
- **episodic state** — events experienced by Kamimusuhi;
- **relationship state** — models of particular people and shared history;
- **library knowledge** — externally sourced material;
- **session state** — ephemeral current interaction state.

A retrieved document MUST NOT silently become a self-belief, relationship fact, or autobiographical event.

### INV-003 Models do not own identity

A resident or external language model is a cognitive component, not the owner of canonical identity.

Replacing a model MAY change behavior and therefore requires continuity evaluation, but model replacement MUST NOT by itself erase durable self-state or memory lineage.

### INV-004 Parameter count is not a cognitive tier

Execution tiers such as `K-Edge`, `K-Core`, and `K-Deep` SHALL represent roles and available cognitive budgets, not parameter-count classes.

The implementation SHOULD describe effective cognitive capacity using a profile containing some or all of:

- resident model capacity;
- active/test-time compute budget;
- recurrent deliberation depth;
- working-memory budget;
- durable-memory reach;
- retrieval bandwidth;
- available tools;
- available local/external models;
- modality access;
- latency budget;
- energy budget;
- network/privacy constraints.

### INV-005 Durable mutation is explicit and attributable

Model output, dream output, background cognition, tool output, or retrieved text MUST NOT directly overwrite canonical durable state.

All durable changes SHALL pass through an explicit proposal/validation/activation boundary and preserve provenance, predecessor state, and disposition.

### INV-006 Distributed execution must not imply accidental duplication

Multiple devices or processes MAY perceive, reason, cache, and propose state changes concurrently.

There SHALL nevertheless be an authoritative continuity mechanism that prevents independent stale branches from silently presenting themselves as the same canonical individual.

### INV-007 External intelligence remains distinguishable from self

External models, search engines, code interpreters, tools, and services MAY be incorporated into cognition.

The system SHALL preserve attribution sufficient to distinguish:

- reasoning performed by the active Persona Core;
- results supplied by an external model or tool;
- evidence retrieved from an external source;
- durable interpretations made by Kamimusuhi.

### INV-008 Internally generated content is not automatically factual

Dreams, simulations, counterfactuals, associations, reflections, and hypotheses SHALL be represented as generated cognition, not historical evidence.

They MAY produce proposals which are later validated against evidence.

### INV-009 Historical event and present interpretation are separable

The system SHOULD preserve the difference between:

- what was recorded as having happened;
- how it was interpreted at the time;
- how it is interpreted now.

Reinterpretation SHALL NOT require destructive rewriting of historical provenance.

### INV-010 Self-modeling does not imply a scientific consciousness claim

Kamimusuhi MAY maintain first-person self-reference and MAY reason that it exists as a continuing artificial process.

The architecture SHALL NOT treat such self-reference as proof of phenomenal consciousness, human-equivalent subjectivity, or legal/moral personhood.

## 3. Cognitive modes

Kamimusuhi SHOULD support multiple computational modes rather than forcing all events through one expensive deliberative path.

### 3.1 Reflex mode

Reflex processing is low-latency and bounded.

Suitable responsibilities include:

- interrupt handling;
- hard runtime invariants;
- low-risk habitual responses;
- obvious format/protocol reactions;
- critical resource or integrity alarms;
- attention triggers.

Reflex actions SHOULD be testable without a frontier model call.

### 3.2 Background cognition

Background processes MAY continuously evaluate:

- salience;
- novelty;
- memory match;
- contradiction;
- relationship signals;
- uncertainty;
- prediction error;
- resource availability;
- unresolved goals;
- privacy/integrity risk.

Background processes SHALL NOT need to expose all intermediate activity as natural-language thought.

### 3.3 Workspace cognition

A workspace SHALL provide a bounded shared representation for information selected as relevant to the currently active cognitive episode.

The workspace MAY contain attributed self-state, recalled memory, tool results, external-model results, and task context.

### 3.4 Deliberative cognition

Deliberation SHOULD support variable depth.

The active cognitive system MAY choose to:

- answer directly;
- recall additional memory;
- retrieve library knowledge;
- think for additional recurrent steps;
- execute code;
- call a specialist model;
- call a frontier model;
- ask for clarification;
- defer or decline an action.

The decision and its resource cost SHOULD be observable.

### 3.5 Default cognition

When no external task demands immediate attention, the system MAY perform bounded internally initiated cognition such as:

- revisiting unresolved matters;
- autobiographical reflection;
- future simulation;
- relationship-model maintenance;
- curiosity-driven retrieval;
- planning;
- monitoring its own integrity and resources.

### 3.6 Sleep / consolidation / dream

Offline cognition MAY perform:

- prioritized replay;
- memory consolidation;
- clustering and schema induction;
- association and abstraction;
- contradiction discovery;
- compression and forgetting;
- generation of counterfactuals or hypotheses;
- index maintenance;
- candidate self-model updates;
- candidate training/distillation data generation.

Dream output SHALL NOT directly enter canonical autobiographical history as an experienced external event.

Direct weight modification during sleep is a future capability and MUST require separate rollback, evaluation, and lineage controls.

## 4. Persona Core Model

The long-term architecture SHALL support a Kamimusuhi-native **Persona Core Model** or equivalent native cognitive model.

It SHOULD be optimized more strongly for the following than for exhaustive encyclopedic memorization:

- self/other distinction;
- dialogue and pragmatics;
- social cognition and theory-of-mind-like inference;
- values and temperament;
- uncertainty and epistemic calibration;
- memory use and attribution;
- tool use;
- delegation to stronger or specialized models;
- integration of external results;
- response expression consistent with persistent identity;
- reflective and autobiographical reasoning.

The Persona Core Model MAY descend from a custom model lineage such as `novllm`.

A Persona Core Model is not required for v0.1; provider-neutral simulation of its contract is sufficient.

## 5. Cognitive resources and routing

### FR-001 Cognitive resource registry

The runtime SHALL represent available cognitive resources explicitly.

Resources MAY include:

- local language models;
- remote/frontier models;
- search;
- code execution;
- calculators/solvers;
- databases;
- sensors;
- robot or UI actuators;
- retrieval services;
- specialized classifiers.

Each resource SHOULD expose capability, locality, trust, cost, latency, and health metadata.

### FR-002 Cognitive routing

Routing SHALL be provider-neutral and SHOULD consider:

- task type;
- required capability;
- expected depth;
- privacy/locality;
- latency;
- energy and monetary cost;
- network availability;
- model/tool health;
- user/operator policy;
- previous observed performance.

The router MAY be deterministic initially and learned later.

### FR-003 Structured delegation

When practical, external models SHOULD receive structured task requests and return structured reasoning material rather than being asked to impersonate Kamimusuhi.

Final user-facing expression SHOULD normally be integrated by the Persona Core or its current substitute.

## 6. Memory system

### 6.1 Memory domains

The canonical design SHALL support at least:

- `session` — ephemeral/current interaction state;
- `episodic` — experienced events;
- `self` — self-model beliefs and dispositions;
- `relationship` — person-specific and shared relational state;
- `semantic` — abstractions/schemas derived from experience;
- `library` — external knowledge artifacts;
- `procedural` — reusable learned action patterns.

### 6.2 Minimum durable record semantics

Durable records SHALL support, where applicable:

- stable ID;
- domain and kind;
- normalized content/state;
- creation and update times;
- provenance/evidence references;
- confidence or evidential status;
- lifecycle state;
- predecessor/revision/supersession relations;
- authoring process/resource attribution;
- canonical lineage/commit reference.

### FR-004 Episodic capture

The system SHALL be able to record a notable interaction or event without immediately converting it into a stable self-trait or semantic belief.

### FR-005 Consolidation

The system SHOULD support a path from multiple episodes to higher-level schema or self-model proposals.

A single episode SHOULD NOT normally be sufficient to rewrite a stable trait unless explicitly designated as formative or authoritative.

### FR-006 Reconsolidation-like revision

When a memory is reactivated and reinterpreted, the system SHOULD permit a new interpretation while preserving prior record lineage.

### FR-007 Forgetting

The architecture SHALL support multiple forms of forgetting:

- retrieval decay;
- compression/summarization;
- supersession;
- archive/cold storage;
- deliberate deletion;
- derived-index eviction.

These operations SHALL remain distinguishable.

## 7. Self and relationship modeling

### FR-008 Self model

The self model MAY include:

- identity anchors;
- values;
- stable/slow-changing traits;
- current internal state;
- interests/preferences;
- autobiographical narrative;
- current goals and commitments;
- beliefs about capabilities and limitations;
- model/software/body lineage.

### FR-009 Relationship model

Relationship state SHALL be separate from self-state.

A relationship model MAY represent:

- stable facts about another person;
- shared history;
- trust;
- communication preferences;
- unresolved misunderstandings;
- commitments;
- inferred current state;
- what each party is believed to know or expect.

### FR-010 Narrative identity

The architecture SHOULD permit construction of an inspectable autobiographical narrative linking formative events, changes in interpretation, relationships, and present self-state.

The narrative is a derived/maintained representation, not the sole canonical history.

## 8. Digital interoception and regulation

Kamimusuhi MAY maintain internal regulatory variables analogous in function—not biology—to interoceptive or homeostatic signals.

Candidate variables include:

- compute pressure;
- memory integrity;
- uncertainty;
- novelty;
- resource availability;
- unresolved-goal pressure;
- relationship tension/trust;
- privacy/security risk;
- context fragmentation;
- prediction error.

These variables MAY influence attention, routing, memory priority, response strategy, and sleep scheduling.

They SHOULD represent real system state rather than decorative emotion labels.

## 9. Distributed embodiment and continuity

### FR-011 Execution surfaces

Kamimusuhi MAY run across multiple devices and interfaces, including edge devices and core compute nodes.

A surface SHALL have a stable authenticated identity and declared capabilities.

### FR-012 Cognitive tiers

The architecture SHOULD support roles such as:

- `K-Edge` — low-latency local presence, perception, reflex, salience, local dialogue, cached memory;
- `K-Core` — deeper resident cognition, broader memory reach, central integration, consolidation;
- `K-Deep` — optional escalated compute for long-horizon reasoning/research.

These are deployment roles, not fixed model sizes.

### FR-013 Continuity authority

At any point where canonical self-state can change, the system SHALL identify the authoritative predecessor state/branch head.

State activation SHOULD use transactional semantics such as:

1. propose against a known predecessor;
2. validate authority and freshness;
3. accept/reject/quarantine/defer;
4. atomically activate accepted state;
5. record lineage and audit receipt.

### FR-014 Migration

A model, host, or interaction-surface change SHOULD be representable as migration when continuity-bearing state and authorized lineage are preserved.

Migration does not guarantee behavioral invariance; material cognitive changes SHOULD be evaluated separately.

### FR-015 Split-brain handling

Network partitions or concurrent writers MUST NOT silently create two authoritative descendants from one canonical state.

Conflicting branches SHALL require explicit reconciliation, quarantine, or designation as a fork/new individual.

## 10. Mutation pipeline

### FR-016 Proposal boundary

Reflection, dream, background cognition, user interaction, tools, and migration processes MAY emit typed proposals.

A proposal SHALL identify:

- operation;
- target domain;
- candidate content/state;
- evidence/provenance;
- confidence;
- source process/resource;
- expected predecessor state where relevant.

### FR-017 Mutation policy

The mutation layer SHALL be capable of rejecting:

- malformed state;
- missing provenance;
- stale writes;
- duplicate writes;
- unsupported self-beliefs;
- library-to-persona contamination;
- dream/simulation content presented as external fact;
- unauthorized cross-domain writes;
- contradictory changes without explicit reconciliation;
- privilege/authority changes lacking authorization.

### FR-018 Auditability

Significant state transitions SHALL emit structured audit events.

A developer should be able to reconstruct why a current self/memory state exists.

## 11. Observability

A cognitive episode SHOULD be traceable across events such as:

```text
event.received
reflex.evaluated
salience.updated
workspace.activated
self.recalled
relationship.recalled
library.retrieved
deliberation.started
cognitive_resource.selected
external_model.called
tool.called
deliberation.completed
response.integrated
memory.proposed
state.policy.decided
state.activated
sleep.consolidated
```

Natural-language private reasoning need not be logged. Structured decisions, provenance, resource use, and state effects should be sufficient for debugging and governance.

## 12. Non-functional requirements

### NFR-001 Recoverability

A failed model, tool, node, or background worker MUST NOT leave partially activated canonical state.

### NFR-002 Testability

Core continuity, memory, routing, policy, and state-activation behavior MUST be testable using deterministic fake cognitive resources.

### NFR-003 Local-first identity state

The architecture SHOULD permit canonical identity and autobiographical memory to remain on operator-controlled infrastructure.

### NFR-004 Migration and schema evolution

Durable state SHALL be versionable and migratable without discarding provenance/history.

### NFR-005 Graceful degradation

Loss of network, external models, or a non-authoritative device SHOULD reduce capabilities rather than necessarily destroy continuity.

### NFR-006 Cognitive-budget visibility

A trace SHOULD identify major compute/resource escalations so that effective cognition can be measured independently of model parameter count.

## 13. v0.1 acceptance test

v0.1 intentionally proves only the foundational continuity contract.

A reproducible test SHALL demonstrate:

1. Start with a new canonical identity state.
2. Conduct a short interaction containing one explicit personal/relationship-relevant fact.
3. Store the raw session separately from durable state.
4. Produce a typed durable-memory proposal.
5. Validate and activate it through policy against an authoritative predecessor.
6. Import one external library artifact separately.
7. Terminate the runtime completely.
8. Restart in a new process/session.
9. Recover the same continuity-bearing identity and relevant memory.
10. Retrieve library evidence through the independent library path.
11. Route at least one cognitive operation through a provider-neutral resource adapter.
12. Expose a trace showing attribution and the accepted state transition.

Passing v0.1 establishes durable continuity and domain separation. It does not establish autonomous development, distributed embodiment, or consciousness.

## 14. Planned milestones

### v0.2 — cognitive separation

- explicit self / episodic / relationship / library domains;
- workspace abstraction;
- reflex/background event paths;
- cognitive-resource registry;
- structured delegation.

### v0.3 — Persona Core

- first Kamimusuhi-native Persona Core candidate;
- persona regression/continuity benchmark;
- local direct-response path;
- external reasoning integration.

### v0.4 — consolidation

- sleep/offline scheduler;
- replay and schema induction;
- guarded self-model updates;
- memory reinterpretation and forgetting.

### v0.5 — distributed body

- K-Edge + K-Core deployment;
- authenticated cognitive surfaces;
- canonical state activation across nodes;
- network partition and recovery tests.

### v1.x — artificial cognitive life research

- adaptive recurrent cognition;
- internal regulatory dynamics;
- developmental learning;
- model-weight consolidation under rollback/evaluation;
- long-horizon identity and behavioral continuity studies.

## 15. Open research questions

- What properties define identity continuity when the Persona Core itself changes?
- Which state belongs in weights, explicit self-state, episodic memory, or derived schema?
- How should recurrent compute be allocated dynamically?
- Can background cognition remain useful without becoming uncontrolled self-reinforcement?
- How should digital interoceptive variables be learned or calibrated?
- When does a distributed branch become a fork/new individual rather than a continuation?
- How should development be measured independently of benchmark task performance?
- What evaluation can distinguish durable personality from mere stylistic imitation?
- How much encyclopedic knowledge can a native Persona Core omit without damaging world modeling and social cognition?
