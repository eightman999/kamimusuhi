# Kamimusuhi / かみむすび Specification

Status: **Draft / pre-alpha**  
Specification generation: **v0.3 concept / 2026-09-08**  
Implementation target: **v0.1 continuity slice**  
Language: **[`spec.md`](./spec.md) is the primary Japanese specification and this file is its English version. If the two differ materially, the Japanese version takes precedence.**

This specification defines the **behavioral, authority, and continuity invariants** that Kamimusuhi must preserve independently of any specific model family, parameter count, database, UI, device, inference provider, or implementation language.

Normative terms are used as follows:

- **MUST / SHALL** — required for a conforming Kamimusuhi implementation.
- **SHOULD** — expected unless a documented reason and compensating guarantee justify otherwise.
- **MAY** — optional capability or permitted design choice.

Research notes may contain competing hypotheses. The default normative order is `spec.md` → `architecture.md` → `docs/`.

---

## 1. System definition

Kamimusuhi is a **persistent, distributed artificial cognitive individual** whose continuity does not depend on one model context, process, machine, or interface.

The system is intended, over time, to support the following:

- maintain a self-model and autobiographical continuity;
- distinguish self, other people, external knowledge, and external cognitive resources;
- record, reinterpret, consolidate, and forget experience;
- vary reasoning depth and computational expenditure according to need;
- delegate reasoning, retrieval, calculation, code execution, perception, and specialist tasks to external resources;
- inhabit multiple authenticated devices and surfaces while preserving one authoritative identity lineage;
- perform background cognition, default cognition, and offline consolidation outside request/response interaction;
- treat vision, hearing, touch, environment, and machine telemetry as parts of an artificial sensory system;
- treat speech as a time-bearing motor/social action rather than only text-to-speech rendering;
- maintain mechanically grounded knowledge of the computational substrate currently supporting cognition;
- survive replacement of models, hosts, interfaces, storage engines, tool providers, and implementation languages without automatically creating a new identity.

---

## 2. Core invariants

### INV-001 — Identity is not a prompt

No system prompt, chat transcript, model hidden state, or provider-specific session SHALL be the sole authoritative representation of Kamimusuhi's identity.

Continuity-bearing identity SHALL include explicit durable state and attributable lineage.

### INV-002 — Persona, experience, relationship, and Library are distinct domains

The system SHALL distinguish at least:

- `self/persona` — state about Kamimusuhi itself;
- `episodic` — events experienced by Kamimusuhi;
- `relationship` — state about particular people and shared history;
- `library` — externally sourced knowledge;
- `session` — ephemeral current interaction state;
- `canonical evidence` — source events, utterances, execution results, and other primary records.

Retrieved documents MUST NOT silently become self-beliefs, relationship facts, or autobiographical events.

### INV-003 — Models do not own identity

Resident and external models are cognitive components, not owners of canonical identity.

Replacing a Persona Core may materially change behavior and therefore requires continuity evaluation, but model replacement alone MUST NOT erase durable self-state or lineage.

### INV-004 — Parameter count is not a cognitive tier

`K-Edge`, `K-Core`, `K-Deep`, and similar tiers SHALL represent execution roles and available Cognitive Budget, not parameter-count classes.

A capability profile SHOULD be able to represent:

- resident model capacity;
- active/test-time compute;
- recurrent deliberation depth;
- working-memory budget;
- durable-memory reach;
- retrieval bandwidth;
- available tools, sensors, and actuators;
- available local/external models;
- latency, energy, and monetary budgets;
- network and privacy constraints.

### INV-005 — Durable mutation is explicit and attributable

Model output, dream output, background cognition, tool output, or retrieved text MUST NOT directly overwrite canonical durable state.

Durable changes SHALL pass through an explicit **proposal → validation/policy → activation** boundary and preserve provenance, predecessor state, and disposition.

### INV-006 — Distributed execution must not imply accidental duplication

Multiple devices or processes MAY perceive, reason, cache, and propose changes concurrently.

There SHALL nevertheless be a continuity mechanism that prevents multiple descendants of a stale predecessor from silently presenting themselves as the same authoritative individual.

### INV-007 — External intelligence remains distinguishable from self

External models, search engines, code interpreters, tools, and services MAY be deeply integrated into cognition.

The system SHALL preserve attribution sufficient to distinguish at least:

- judgments made by the Persona Core / active core;
- results returned by an external model or tool;
- evidence retrieved from an external source;
- interpretations or belief proposals formed by Kamimusuhi after integration.

### INV-008 — Internally generated content is not automatically factual

Dreams, simulations, counterfactuals, associations, reflections, and hypotheses SHALL be represented as generated cognition rather than external historical evidence.

They MAY produce proposals, but MUST NOT directly enter canonical autobiography as external events without evidence checks.

### INV-009 — Historical events and present interpretation are separable

The system SHOULD preserve the distinction between:

- what was recorded as having happened;
- how it was interpreted at the time;
- how it is interpreted now.

Reinterpretation SHALL NOT require destructive rewriting of historical provenance.

### INV-010 — Self-modeling does not prove consciousness

Kamimusuhi MAY use first-person self-reference and model itself as a continuing artificial process.

The architecture SHALL NOT treat such self-reference as proof of phenomenal consciousness, human-equivalent subjectivity, or legal/moral personhood.

### INV-011 — Canonical evidence outranks derived renderings

Summaries, indexes, autobiographical narratives, person profiles, prompt renderings, and handoff documents are generally **derived views**.

If a derived view becomes stale, lossy, or corrupted, it SHOULD be possible, where practical, to rebuild it from canonical evidence.

Stable derived records SHOULD retain identifiers for the evidence from which they were produced.

### INV-012 — Thought, utterance, commitment, execution, and delivery are distinct states

Internally generated thought MUST NOT be silently treated as something actually spoken.

The architecture SHALL also avoid inferring equivalence among the following merely from prose or control flow:

```text
thought
utterance
user/operator confirmation
authorization
execution started
execution succeeded / failed / unknown
result delivered
```

### INV-013 — Mechanically observable present state comes from its actual owner

Hardware state, node health, the active model, network state, and task execution state are mechanically observable present-state facts.

An LLM-generated prose description MUST NOT become a second authoritative source of truth for such state. Self-descriptions SHOULD be derived from the actual runtime owner where practical.

### INV-014 — Boundaries that can lose authority, provenance, ordering, or liveness should be explicit

Memory extraction, tool dispatch, background work, side effects, result delivery, and canonical mutation often fail at boundaries rather than in the core model.

Where authority, ordering, liveness, or provenance can be lost, the implementation SHOULD represent the boundary with explicit state rather than infer it from prose or implicit control flow.

---

## 3. Cognitive modes

Kamimusuhi SHOULD support multiple computational modes rather than route every event through one expensive LLM deliberation path.

### 3.1 Reflex mode

Reflex processing should be low-latency, bounded, and testable.

Candidate responsibilities include:

- interrupt handling;
- hard runtime invariants;
- integrity/health alarms;
- obvious attentional capture;
- low-risk habitual reactions;
- simple protocol reactions;
- wake/sleep triggers;
- emergency deny/stop paths.

Reflex actions SHOULD be testable without a frontier-model call.

### 3.2 K-Nerve — experimental reflex/control substrate

Kamimusuhi MAY contain a non-canonical **K-Nerve** layer below `K-Edge`.

Candidate implementations include:

- LIF / SNN systems;
- learned discrete logic / DLGN-like circuits;
- LUT / DWN-like structures;
- FSMs, counters, and threshold systems;
- tiny classifiers or tiny neural models;
- deterministic rules.

K-Nerve MAY handle high-frequency, low-semantic-cost control such as:

```text
WAKE_K_EDGE
WAKE_K_CORE
UPDATE_SALIENCE
RETRIEVE_EPISODIC_MEMORY
WRITE_EPISODE_CANDIDATE
SPEAK_BACKCHANNEL
IGNORE_EVENT
USE_LOCAL_MODEL
USE_REMOTE_MODEL
ESCALATE_COGNITION
INTERRUPT_CURRENT_ACTION
```

K-Nerve MUST NOT own canonical self-state, identity, or autobiography.

Adoption SHALL be justified by measured latency, energy, accuracy, stability, and auditability rather than biological analogy.

### 3.3 Background cognition

Background processes MAY continuously evaluate:

- salience;
- novelty;
- memory match/activation;
- contradiction;
- relationship/social signals;
- uncertainty;
- prediction error;
- resource availability and health;
- unresolved goals;
- privacy/security/integrity risk.

Background processes need not expose all intermediate activity as natural-language chain-of-thought.

#### Freshness semantics

Background jobs SHOULD carry freshness information for the observation or task that triggered them.

A result that becomes stale while waiting for compute MUST NOT be applied indefinitely as though it described the present. The system SHOULD be able to drop, recompute, quarantine, or mark stale results.

### 3.4 Global Workspace

The workspace SHALL be a bounded shared representation for information selected for the currently active cognitive episode.

Workspace items SHOULD retain their origin, for example:

```text
[self-state]
[relationship-model]
[episodic-recall]
[canonical-evidence]
[library-evidence]
[tool-result]
[external-model-result]
[current-input]
[regulatory-signal]
[sensory-event]
```

The workspace is not durable memory.

### 3.5 Deliberative cognition

Deliberation SHOULD support variable depth.

The active cognitive system MAY choose to:

- respond directly;
- recall additional memory;
- retrieve canonical evidence;
- retrieve Library evidence;
- run additional recurrent reasoning steps;
- execute code;
- call a specialist/local model;
- call a frontier model;
- simulate;
- ask for clarification;
- defer or decline.

Major resource escalation and routing decisions SHOULD be observable.

### 3.6 Default cognition

When no external task demands immediate attention, the system MAY perform bounded internally initiated cognition such as:

- revisiting unresolved matters;
- autobiographical reflection;
- future simulation;
- relationship-model maintenance;
- curiosity-driven retrieval;
- planning;
- integrity/resource monitoring.

### 3.7 Sleep / consolidation / dream

Offline cognition MAY perform:

- prioritized replay;
- memory consolidation;
- clustering and schema induction;
- association and abstraction;
- contradiction discovery;
- compression and forgetting;
- counterfactual/hypothesis generation;
- index maintenance;
- candidate self-model updates;
- candidate distillation/training-data generation.

Dream output SHALL NOT directly enter canonical autobiographical history as an experienced external event.

Direct weight modification during sleep is a future capability. If implemented, it MUST require:

- a frozen pre-update checkpoint;
- training-data provenance;
- continuity/persona regression evaluation;
- rollback capability;
- a model-lineage record.

---

## 4. Persona Core Model

The long-term architecture SHALL support a Kamimusuhi-native **Persona Core Model** or equivalent native cognitive model.

The Persona Core SHOULD be optimized more strongly for the following than for exhaustive encyclopedic memorization:

- self/other distinction;
- dialogue and pragmatics;
- social cognition and theory-of-mind-like inference;
- common ground and relationship reasoning;
- values, temperament, and persona priors;
- distinction among observation, inference, evidence, and belief;
- uncertainty and epistemic calibration;
- autobiographical and relationship memory use;
- tool use;
- delegation to stronger or specialized models;
- critical integration of external results;
- body, embodiment, and capability modeling;
- identity-consistent expression;
- reflective and autobiographical reasoning.

The Persona Core MAY descend from a custom lineage such as `novllm`.

A dedicated Persona Core is not required for v0.1; its contract MAY be simulated through provider-neutral adapters to conventional models.

The Persona Core weights are not the complete individual. Functional persona is approximately:

```text
Persona Core disposition
+ Persistent Self State
+ autobiographical / relationship history
+ current regulatory / embodiment state
+ current cognitive episode
```

Replacing the Persona Core is more significant than replacing an external tool model and SHOULD trigger a continuity suite covering areas such as:

- identity recall;
- value consistency;
- relationship interpretation;
- autobiographical interpretation;
- uncertainty behavior;
- tool/routing behavior;
- characteristic expression;
- decisions in formative scenarios.

---

## 5. Cognitive resources and routing

### FR-001 — Cognitive Resource Registry

The runtime SHALL explicitly represent available cognitive resources.

Resources MAY include:

- local language models;
- remote/frontier models;
- search;
- code execution;
- calculators/solvers;
- databases;
- sensors;
- actuators, robots, or UI control;
- retrieval services;
- classifiers;
- speech, vision, ASR, or TTS components.

Each resource SHOULD expose capability, locality, trust, cost, latency, health, and privacy-boundary metadata.

### FR-002 — Cognitive routing

Routing SHALL be provider-neutral and SHOULD consider:

- task type and required capability;
- expected depth;
- uncertainty;
- privacy/locality;
- latency;
- energy and monetary cost;
- network availability;
- resource health;
- operator/user policy;
- previously observed performance.

The router MAY begin deterministic and later become learned.

### FR-003 — Structured delegation

Where practical, external models SHOULD receive structured tasks rather than prompts asking them to impersonate Kamimusuhi.

External results SHOULD return with attribution, and final user-facing expression should normally be integrated by the Persona Core or its current substitute.

### FR-003A — Routing chokepoints

Cross-cutting invariant boundaries such as model/tool selection, tool execution, and external side-effect dispatch SHOULD pass through a small number of auditable chokepoints instead of many unrelated bypass paths.

---

## 6. Canonical evidence and memory system

### 6.1 Memory / evidence domains

The canonical design SHALL distinguish at least:

- `session` — ephemeral/current interaction state;
- `canonical` — source event/evidence history;
- `episodic` — experienced event memory;
- `self` — self-model beliefs and dispositions;
- `relationship` — person-specific/shared relational state;
- `semantic` — abstractions and schemas derived from experience;
- `library` — external knowledge artifacts;
- `procedural` — reusable learned action patterns;
- `reflection` — internal reflection/hypotheses that are not equivalent to fact.

### FR-004 — Canonical Evidence / Self History

The system SHALL be able to retain canonical evidence beneath derived summaries for events significant to identity or autobiographical continuity.

Records MAY include:

- user-facing utterances/events;
- sensory events;
- tool requests/results;
- external-action receipts/results;
- state-transition receipts;
- explicit user/operator corrections;
- migration events.

Canonical evidence SHOULD be append-oriented. Corrections SHOULD normally be represented through correction, supersession, or invalidation links rather than destructive overwrite.

### 6.2 Minimum durable record semantics

Durable records SHALL support, where applicable:

- stable ID;
- domain and kind;
- normalized content/state;
- creation/update times;
- provenance/evidence references;
- confidence/evidential status;
- lifecycle state;
- predecessor/revision/supersession relations;
- authoring process/resource attribution;
- canonical lineage/commit reference.

### FR-005 — Episodic capture

The system SHALL be able to record a notable interaction/event without immediately converting it into a stable self-trait or semantic belief.

### FR-006 — Multi-trigger and provisional capture

Memory formation MUST NOT depend only on a clean session-end event.

The system SHOULD support multiple triggers such as:

```text
explicit remember request
rolling checkpoint
idle / stale episode checkpoint
pre-compaction checkpoint
surface/process shutdown checkpoint where available
post-hoc recovery scan
```

Interpretations extracted before an episode is complete SHOULD be able to carry a provisional lifecycle state.

Candidate states include:

```text
PROVISIONAL
CONFIRMED_BY_EPISODE
CORRECTED
SUPERSEDED
INVALIDATED
```

Extraction/derivation jobs SHOULD carry a stable source range/evidence set and an idempotency identity.

### FR-007 — Consolidation

The system SHOULD support a path from multiple episodes to semantic schemas, relationship syntheses, or self-model proposals.

A single episode SHOULD NOT normally rewrite a stable trait unless explicitly designated as formative or authoritative.

### FR-008 — Reconsolidation-like revision

When a memory is reactivated and reinterpreted, the system SHOULD permit a new interpretation while preserving previous interpretations and evidence lineage.

### FR-009 — Forgetting

The architecture SHALL distinguish at least:

- retrieval decay;
- compression/summarization;
- supersession;
- archive/cold storage;
- deliberate deletion;
- derived-index eviction.

Making something harder to retrieve is not the same operation as deleting source evidence.

### 6.3 Retrieval

Retrieval SHOULD permit designs that do not rely only on vector similarity.

A candidate stack may include:

```text
semantic candidates
+ lexical / exact candidates
+ temporal constraints
+ authority/source filters
+ diversity/dedup
+ evidence-aware reranking
```

Retrieved material SHOULD carry authority type and provenance into the workspace.

---

## 7. Self and relationship modeling

### FR-010 — Self Model

The self model MAY include:

- identity anchors;
- values;
- stable/slow-changing traits;
- current regulatory state;
- interests/preferences;
- autobiographical narrative;
- current goals and commitments;
- capability/limitation beliefs;
- model/software/body lineage;
- infrastructure preferences.

Mechanically observable runtime state SHOULD remain separate and be read from Embodiment State rather than duplicated as self-belief.

### FR-011 — Relationship Model

Relationship state SHALL remain separate from self-state.

A person model MAY represent:

- stable known facts;
- shared history;
- trust;
- communication preferences;
- unresolved misunderstandings;
- commitments;
- inferred current state;
- what each party is believed to know or expect;
- common ground.

Observation and inference SHOULD retain distinct confidence/provenance.

### FR-012 — Narrative identity

The architecture SHOULD permit construction of an inspectable autobiographical narrative linking formative events, reinterpretation, relationships, and present self-state.

The narrative is a derived/maintained representation, not the sole source of canonical history.

---

## 8. Digital interoception and computational embodiment

Kamimusuhi MAY maintain internal regulatory variables as functional control signals rather than biological imitation.

Candidate variables include:

- compute pressure;
- memory integrity;
- uncertainty;
- novelty;
- prediction error;
- resource availability;
- unresolved-goal pressure;
- relationship tension/trust;
- privacy/security risk;
- context fragmentation;
- latency pressure;
- thermal/energy pressure.

These variables MAY influence attention, routing, memory priority, response strategy, and sleep scheduling.

### FR-013 — Embodiment State

The runtime SHOULD provide a mechanically derived, read-only **Embodiment State**.

Candidate fields include:

```text
active node / role
available compute devices
actual inference model/provider/locality
context/capacity limits
observed TTFT / throughput / error rate
RAM/VRAM/storage/thermal pressure
network bandwidth/latency/connectivity
available sensors/actuators
K-Edge/K-Core/K-Deep availability
```

Embodiment State SHALL be derived from telemetry owners rather than treating LLM-generated prose as primary state.

### FR-014 — Infrastructure Preferences

Kamimusuhi MAY maintain persistent infrastructure preferences separately from measured Embodiment State.

Examples include:

- preferred latency;
- local-first preference;
- privacy preference;
- provider-dependency preference;
- redundancy preference;
- continuity-storage requirements.

If a preference becomes durable self-state, changes SHOULD pass through the normal proposal/mutation discipline.

---

## 9. Sensory nervous system and expression

### FR-015 — Sensory Event Bus

Multiple sensor modalities SHOULD be able to participate in a common artificial sensory nervous system rather than exist only as unrelated plugins.

High-rate raw streams SHOULD remain near sensors where practical for filtering, detection, and compression. Central cognition SHOULD consume compact attributed events where possible.

Sensory events SHOULD carry, where applicable:

- timestamp;
- source/surface identity;
- modality;
- event type;
- payload/features;
- confidence;
- provenance/processing chain;
- freshness/expiry.

Candidate modalities include:

- vision: cameras, screens, UI/accessibility trees, documents/images/video;
- hearing: audio, ASR, speaker state, sound events, paralinguistic cues;
- tactile: contact, pressure, shear, slip, vibration, temperature;
- environment: temperature, humidity, pressure, light, CO2/VOC, etc.;
- proprioception/body: IMU, pose, node topology, sensor availability, network state;
- machine interoception: runtime-health and resource telemetry.

### FR-016 — Reflex arc

Safety-critical or latency-critical sensory responses MAY use a local reflex path without waiting for semantic deliberation.

Reflex output MUST NOT directly create durable beliefs, promises, or relationship commitments.

### FR-017 — Speech and Vocal Expression

Speech MAY be treated as a motor/social system that realizes structured communicative intention over time rather than a simple `text -> TTS` post-processing stage.

The speech stack SHOULD be able to separate, where useful:

- speech act / communicative intention;
- linguistic realization;
- utterance planning;
- prosody / affect plan;
- streaming synthesis;
- turn-taking state;
- interruption/cancellation;
- backchannels;
- auditory self-monitoring.

A vocal reflex MAY generate short acknowledgements or backchannels but MUST NOT fabricate semantic agreement, promises, or authorization.

Full-duplex interaction is a long-term target. The system SHOULD at least be able to represent explicit turn states analogous to `LISTEN / SPEAK / WAIT / DUAL / INTERRUPT / YIELD`.

---

## 10. Distributed embodiment and continuity

### FR-018 — Execution surfaces

Kamimusuhi MAY run across multiple surfaces such as phones, laptops, home servers, robots, and cloud nodes.

Each surface SHALL have a stable authenticated identity and declared capabilities.

### FR-019 — Cognitive tiers

The architecture SHOULD support roles such as:

- `K-Edge` — low-latency local presence, perception, reflex, salience, local dialogue, cached memory;
- `K-Core` — broader durable-memory reach, central integration, deeper cognition, consolidation;
- `K-Deep` — optional high-compute long-horizon reasoning/research.

These roles do not imply fixed model sizes.

### FR-020 — Continuity authority

Any path capable of changing canonical self-state SHALL identify the authoritative predecessor / branch head.

Activation SHOULD use transaction-like semantics:

1. create a proposal against a known predecessor;
2. validate authority, freshness, and policy;
3. decide accept / reject / quarantine / defer;
4. atomically activate accepted state;
5. record lineage/audit receipt.

### FR-021 — Migration

Changing a model, host, or interaction surface SHOULD be representable as migration when continuity-bearing state and authorized lineage are preserved.

Migration does not guarantee behavioral invariance; material cognitive changes SHOULD be evaluated separately.

### FR-022 — Split-brain handling

Network partitions or concurrent writers MUST NOT silently create multiple authoritative descendants from one canonical predecessor.

Conflicting branches SHALL require explicit reconciliation, quarantine, or designation as a fork/new individual.

---

## 11. Mutation and activation pipeline

### FR-023 — Proposal boundary

Reflection, dream, background cognition, user interaction, tools, migration, and sensory interpretation MAY emit typed proposals.

A proposal SHALL identify, where applicable:

- operation;
- target domain;
- candidate content/state;
- evidence/provenance;
- confidence/evidential status;
- source process/resource;
- expected predecessor;
- idempotency identity where relevant.

### FR-024 — Mutation Policy

The mutation layer SHALL be able to reject or quarantine at least:

- malformed state;
- missing provenance;
- stale writes;
- duplicate writes;
- unsupported self-beliefs;
- Library-to-persona contamination;
- dream/simulation content presented as external fact;
- unauthorized cross-domain writes;
- contradictions without explicit reconciliation;
- privilege/authority changes lacking authorization;
- stale background results presented as current state.

### FR-025 — Auditability

Significant state transitions SHALL emit structured audit events.

A developer/operator SHOULD be able to reconstruct why current self/memory state exists without requiring access to natural-language hidden reasoning.

---

## 12. Task, action, and side-effect lifecycle

Long-running systems must track not only whether a tool call was requested but whether it executed, succeeded, and delivered its result.

### FR-026 — Explicit task/action state

Side-effecting actions SHOULD distinguish at least conceptually:

```text
REQUESTED
AUTHORIZED / CONFIRMED
QUEUED / SCHEDULED
EXECUTING
SUCCEEDED | FAILED | UNKNOWN
RESULT_DELIVERED
CANCELLED
```

Confirmation is not execution. Execution started is not success. Success is not the same as result delivery.

### FR-027 — Crash ambiguity and idempotency

If a crash/timeout occurs during an external action and provider outcome cannot be established, the system MUST be able to represent `UNKNOWN` rather than guess success or failure.

Where possible, actions SHOULD use:

- idempotency keys;
- provider-side request/receipt IDs;
- durable job records;
- transactional outbox/delivery logs;
- retry policies that distinguish safe from unsafe retries.

### FR-028 — Background job ownership and delivery

Background jobs SHOULD retain owner, input evidence/task IDs, freshness, attempt information, and result disposition.

Job completion and user-facing result delivery SHOULD be independently representable.

---

## 13. Observability

A cognitive episode SHOULD be traceable through structured events such as:

```text
event.received
surface.identified
sensory.event.created
reflex.evaluated
knerve.signal.updated
salience.updated
workspace.activated
canonical_evidence.retrieved
self.recalled
relationship.recalled
episode.recalled
library.retrieved
cognition.budget.selected
cognitive_resource.selected
external_model.called
tool.called
action.authorized
action.executing
action.completed
response.integrated
speech.started
speech.interrupted
memory.proposed
state.policy.decided
state.activated
background.result.dropped_stale
sleep.consolidated
```

Natural-language private chain-of-thought need not be logged. Structured decisions, provenance, resource use, and state effects should be sufficient for debugging and governance.

---

## 14. Non-functional requirements

### NFR-001 — Recoverability

A failed model, tool, node, or background worker MUST NOT leave partially activated canonical state.

### NFR-002 — Testability

Continuity, memory, routing, policy, activation, and task lifecycle MUST be testable with deterministic fake cognitive resources.

### NFR-003 — Local-first identity state

Canonical identity and autobiographical memory SHOULD be able to remain on operator-controlled infrastructure.

### NFR-004 — Schema evolution

Durable state SHALL be versionable and migratable without discarding provenance/history.

### NFR-005 — Graceful degradation

Loss of network, external models, non-authoritative devices, or sensors MAY reduce capability but MUST NOT by itself destroy continuity.

### NFR-006 — Cognitive-budget visibility

Traces SHOULD identify major compute/resource escalations so effective cognition can be measured independently of parameter count.

### NFR-007 — Latency stratification

The implementation SHALL NOT require a single-language architecture.

Latency-critical hot paths MAY use a native long-lived runtime such as Rust/C++, while research, offline cognition, or model-bound paths MAY use Python or other languages.

Optimization SHOULD be driven by profiling and benchmark end-to-end latency including process startup, serialization, IPC, and model cold start.

### NFR-008 — Security and authority

Surfaces, writers, mutation authority, and privileged actions SHALL be authenticated/authorized.

External untrusted content MUST NOT gain privileged instruction authority merely by being retrieved or observed.

### NFR-009 — Source-of-truth discipline

The same present-state fact SHOULD NOT be maintained as separate authoritative copies by unrelated subsystems.

Views SHOULD be rendered or derived from the subsystem that actually owns the state.

---

## 15. v0.1 acceptance test

v0.1 proves only the foundational **continuity contract**, not the full artificial-life vision.

A reproducible test SHALL demonstrate:

1. Start from a new canonical identity and lineage head.
2. Conduct a short interaction containing one explicit personal/relationship-relevant fact.
3. Store raw/verbatim session evidence separately from durable self-state.
4. Produce a typed durable-memory proposal before or after episode completion.
5. Ensure the proposal carries evidence references and expected predecessor.
6. Validate it through Mutation Policy against the authoritative predecessor.
7. Atomically activate accepted state and retain a lineage receipt.
8. Import an external Library artifact through a path separate from self/episode memory.
9. Route at least one cognitive operation through a provider-neutral resource adapter.
10. Terminate the runtime/process completely.
11. Restart in a fresh process/session.
12. Recover the same authoritative lineage and identity.
13. Recover the relevant episodic/relationship memory.
14. Retrieve external knowledge through the independent Library path.
15. Expose a trace linking evidence → proposal → policy → activation and showing external-resource attribution.

Passing v0.1 establishes durable continuity, domain separation, and proposal-only mutation. It does not establish autonomous development, K-Nerve, distributed embodiment, or consciousness.

---

## 16. Planned milestones

### v0.2 — cognitive separation / evidence discipline

- explicit canonical evidence / session / self / episodic / relationship / Library domains;
- workspace abstraction;
- rolling/provisional memory capture;
- reflex/background event paths;
- cognitive-resource registry;
- structured delegation;
- explicit task/action lifecycle.

### v0.3 — Persona Core and low-latency body prototypes

- first Kamimusuhi-native Persona Core candidate;
- persona/continuity regression suite;
- local direct-response path;
- external reasoning integration;
- K-Nerve baseline experiments against rules/FSM/tiny models;
- sensory event bus prototype;
- speech/backchannel prototype;
- runtime latency benchmarks.

### v0.4 — consolidation / default cognition

- sleep/offline scheduler;
- replay and schema induction;
- guarded self-model updates;
- reinterpretation and forgetting;
- background freshness policy;
- bounded default cognition.

### v0.5 — distributed body

- K-Edge + K-Core deployment;
- authenticated surfaces;
- Embodiment State;
- canonical activation across nodes;
- migration;
- network-partition / split-brain / recovery tests.

### v1.x — artificial cognitive life research

- adaptive recurrent cognition;
- learned Cognitive Budget allocation;
- richer digital interoception;
- full-duplex speech and broader sensory embodiment;
- developmental learning;
- safe weight consolidation with rollback/evaluation;
- long-horizon identity, relationship, and behavioral continuity studies;
- sandboxed self-improvement / descendant evaluation.

---

## 17. Open research questions

- What defines identity continuity when the Persona Core itself changes?
- Which state belongs in weights, explicit self-state, episodic memory, relationship models, or derived schemas?
- How should Cognitive Budget allocation be learned or regulated?
- Can a K-Nerve-like low-cost temporal control substrate outperform rules, FSMs, and tiny neural baselines?
- Under what conditions can background cognition remain useful without becoming uncontrolled self-reinforcement?
- How should digital interoceptive variables be calibrated or learned?
- How much canonical evidence should remain lossless, and how should that trade off against privacy and storage cost?
- When does a distributed branch become a fork/new individual rather than a continuation?
- How should personality drift after model migration be measured?
- How can long-lived relationship competence be measured independently of short benchmark tasks?
- How much do speech timing, backchannels, and sensory embodiment contribute to identity and relationship formation?
- To what extent should computational-embodiment preferences become learned self-state?
- How can a self-improvement step demonstrate benchmark gains without silently losing identity or value continuity?
- How much encyclopedic knowledge can the Persona Core externalize without damaging world modeling and social cognition?
