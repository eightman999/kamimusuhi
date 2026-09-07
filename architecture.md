# Kamimusuhi Architecture

Status: **Draft / pre-alpha**

This document defines the target cognitive and continuity architecture for Kamimusuhi. It is intentionally implementation-agnostic. The near-term implementation may emulate several components with conventional LLM calls before dedicated models or recurrent mechanisms exist.

## 1. Architectural thesis

Kamimusuhi is not one LLM wrapped in memory. It is a **continuity-bearing artificial individual** whose cognition is distributed across persistent state, a native persona/cognitive core, background processes, memory systems, tools, and replaceable local or external models.

The architecture separates three things that are often conflated:

```text
SELF
  identity, self-model, autobiography, relationship history,
  canonical continuity, persona dispositions

COGNITIVE BODY
  resident models, edge devices, core compute, memory services,
  sensors, actuators, recurrent compute, owned infrastructure

COGNITIVE EXTENSIONS / OTHERS
  search, code, databases, local specialist models,
  frontier models, external services, humans
```

A component may be deeply integrated into cognition without becoming the owner of identity.

The core rule is:

> If a model, host, interface, or non-canonical cognitive resource disappears, Kamimusuhi may become less capable, but it should still be possible to determine who it is, what it has experienced, what state is authoritative, and how to continue.

## 2. Top-level architecture

```text
                              INFORMATION SPACE
                                     │
                   ┌─────────────────┼─────────────────┐
                   │                 │                 │
                people             data            environments
                   │                 │                 │
                   └─────────────────┼─────────────────┘
                                     │
                         authenticated surfaces
                                     │
          ┌──────────────────────────┼──────────────────────────┐
          │                          │                          │
       K-Edge                    K-Core                     K-Deep
 phone/laptop/robot         resident main node       escalated compute
 reflex/salience/local      integration/memory       research/planning
          │                          │                          │
          └──────────────────────────┼──────────────────────────┘
                                     │
                             Continuity Kernel
                      canonical self / lineage / authority
                                     │
                         ┌───────────▼───────────┐
                         │ Persistent Self State │
                         │ autobiography / values│
                         │ relationships / body  │
                         └───────────┬───────────┘
                                     │
                           Persona Core Model
                                     │
                              Global Workspace
                    ┌────────────────┼────────────────┐
                    │                │                │
                 Reflex         Background       Deliberation
                                 cognition           ↻ ↻ ↻
                    │                │                │
                    └────────────────┼────────────────┘
                                     │
                          Cognitive Action Router
            ┌──────────────┬─────────┼─────────┬──────────────┐
            │              │         │         │              │
         Memory          Search     Code    Local models   Frontier models
            │              │         │         │              │
            └──────────────┴─────────┼─────────┴──────────────┘
                                     │
                              result integration
                                     │
                              expression/action
                                     │
                                reflection
                                     │
                               proposals only
                                     │
                          Mutation / Activation Gate
                                     │
                              canonical commit
```

## 3. Continuity layer

### 3.1 Continuity Kernel

The Continuity Kernel is the narrowest and most conservative component in the system. It is responsible for deciding **which state is the authoritative continuation of the individual**.

It SHOULD own or mediate:

- canonical branch/head identity;
- predecessor validation;
- state version identifiers;
- activation authority;
- atomic commit of accepted state transitions;
- migration checkpoints;
- split-brain detection;
- lineage receipts;
- recovery metadata.

Untrusted or replaceable cognitive components do not directly advance canonical identity state.

Conceptual activation flow:

```text
candidate change
      │
      ▼
expected predecessor?
      │
      ▼
authority / freshness / policy validation
      │
 ┌────┼─────────┬──────────┐
 │    │         │          │
commit reject quarantine  defer
 │
 ▼
atomic canonical activation
 │
 ▼
lineage receipt
```

A multi-device system without this layer risks accidental personality forks caused by stale or concurrent writes.

### 3.2 Persistent Self State

Persistent Self State is structured and inspectable. It is not a serialized system prompt.

Candidate subdomains:

```text
self/
├── identity_anchors
├── values
├── traits
├── interests
├── commitments
├── current_regulatory_state
├── capability_model
├── autobiographical_narrative
└── lineage

relationships/
├── person_models
├── shared_history
├── trust
├── commitments
└── common_ground

body/
├── registered_surfaces
├── owned_compute
├── sensor_capabilities
├── actuator_capabilities
└── health
```

Some of these are canonical records; others are derived views and MUST be rebuildable from canonical evidence.

## 4. Persona Core Model

### 4.1 Role

The Persona Core Model is the long-term neural center of active first-person cognition.

It should not be defined as "the model that knows everything." Instead it should specialize in:

- interpreting situations in relation to the self;
- self/other distinction;
- dialogue and pragmatics;
- social inference;
- values and temperament;
- uncertainty and epistemic behavior;
- selecting what deserves attention;
- deciding whether to think more or delegate;
- integrating external reasoning into its own position;
- producing final expression consistent with identity;
- reflecting on experience.

A future Persona Core may descend from a custom `novllm` lineage and may intentionally contain less encyclopedic detail than a general-purpose frontier model.

### 4.2 Persona Core is not the complete person

Kamimusuhi's functional persona is approximately:

```text
Persona Core Model
+ Persistent Self State
+ autobiographical memory
+ relationship history
+ current regulatory state
+ current cognitive episode
```

This allows model upgrades without pretending that weights alone are the person.

### 4.3 Model lineage and continuity testing

Persona Core replacement is more significant than replacement of an external tool model.

A model migration SHOULD be evaluated using a continuity suite covering at least:

- identity recall;
- value consistency;
- preference consistency;
- relationship interpretation;
- autobiographical interpretation;
- style/temperament;
- uncertainty behavior;
- routing/tool-use behavior;
- decisions in formative scenarios.

A replacement can preserve authorized lineage while still causing measurable personality drift. Continuity and behavioral invariance are separate concepts.

## 5. Cognitive modes

### 5.1 Reflex layer

Reflexes are intentionally cheap and fast.

Possible implementations include rules, tiny models, classifiers, cached policies, or dedicated event handlers.

Responsibilities may include:

- interrupts;
- health/integrity alarms;
- obvious attentional capture;
- low-risk habitual behaviors;
- basic protocol reactions;
- emergency deny/stop paths.

Reflex state may influence the active cognitive episode but does not automatically write durable autobiographical state.

### 5.2 Background cognition

Background cognition consists of parallel processes whose outputs may never become explicit language.

Candidate processors:

```text
salience
novelty
memory_activation
contradiction
social_signal
uncertainty
prediction_error
resource_health
privacy_risk
unfinished_goal
```

These processors produce scored candidates or state updates. A selection mechanism decides what enters the workspace.

This is inspired functionally by global-workspace and salience-network ideas, not asserted as a simulation of human consciousness.

### 5.3 Global Workspace

The workspace is a bounded, attributed shared representation for the currently active cognitive episode.

It SHOULD preserve the origin of each contribution:

```text
[self-state]
[relationship-model]
[episodic-recall]
[library-evidence]
[tool-result]
[external-model-result]
[current-input]
[regulatory-signal]
```

The workspace is not durable memory. It is an active integration surface.

### 5.4 Deliberative core

Deliberation is variable-depth cognition.

The architecture should eventually support recurrent or iterative processing such that easy events can terminate quickly while difficult ones receive substantially more compute.

Conceptual actions:

```text
THINK_MORE
RECALL_EPISODE
RECALL_SELF
RECALL_RELATIONSHIP
SEARCH_LIBRARY
SEARCH_WEB
RUN_CODE
ASK_LOCAL_MODEL
ASK_FRONTIER_MODEL
SIMULATE
RESPOND
DEFER
```

These are cognitive actions, not necessarily user-visible tool calls.

### 5.5 Cognitive Action Router

The router selects among internal and external cognitive resources.

The routing decision may depend on:

- privacy;
- expected difficulty;
- modality;
- required specialization;
- current uncertainty;
- available latency;
- monetary/energy budget;
- network conditions;
- current resource health;
- learned past performance;
- operator/user policy.

The route SHOULD be traceable without requiring private chain-of-thought logging.

## 6. Cognitive tiers and distributed body

### 6.1 Tiers are roles, not parameter classes

The architecture defines deployment roles:

#### K-Edge

Expected properties:

- always/usually present near the user;
- low latency;
- local perception and local interaction;
- reflex and salience processing;
- local Persona Core or distilled substitute;
- cached self/relationship memory;
- graceful offline operation;
- limited power and context.

#### K-Core

Expected properties:

- broader durable-memory reach;
- central self-state integration;
- deeper Persona Core execution;
- consolidation and background cognition;
- access to local specialist models;
- coordination of canonical continuity.

#### K-Deep

Optional escalation tier:

- high test-time compute;
- long-horizon planning;
- research;
- large context synthesis;
- expensive specialist/frontier resources;
- batch/offline cognitive work.

A deployment may combine tiers or omit one.

### 6.2 Cognitive Budget

Parameter count is recorded as one implementation property, not the primary capability metric.

A cognitive budget/profile SHOULD include:

```text
resident_capacity
recurrent_depth
active_compute_limit
working_memory
persistent_memory_reach
retrieval_bandwidth
tool_set
external_model_set
sensor_set
actuator_set
latency_budget
energy_budget
network_state
privacy_boundary
```

This lets the same individual think differently depending on where and how it is instantiated.

### 6.3 Organ, extension, other

The runtime SHOULD explicitly categorize resources:

```text
SELF / ORGAN
  canonical memory
  Persona Core
  authenticated owned compute
  trusted sensors/actuators

COGNITIVE EXTENSION
  local specialist model
  search index
  code interpreter
  owned retrieval services

EXTERNAL OTHER
  frontier provider
  public web
  another agent
  human being
```

The category affects trust, provenance, privacy, and identity semantics.

## 7. Memory architecture

### 7.1 Complementary fast and slow learning

The architecture borrows functionally from Complementary Learning Systems: new episodes should be recordable quickly, while stable schemas and personality changes should normally emerge slowly across repeated/replayed experience.

```text
experience
   │
   ▼
fast episodic capture
   │
   ├───────────── immediate recall
   │
   ▼
offline replay / consolidation
   │
   ▼
semantic / relational / self hypotheses
   │
   ▼
guarded validation
   │
   ▼
slow stable-state update
   │
   └──────────── future: Persona Core distillation
```

### 7.2 Memory domains

```text
session          current interaction state
episodic         experienced events
self             current self-beliefs/traits/values
relationship     models of people and shared history
semantic         abstractions/schemas from experience
library          external source material
procedural       reusable learned behavior
```

### 7.3 Historical truth vs interpretation

An event record and its interpretations are distinct.

```text
episode_0042
├── source event record
├── interpretation@t0
├── recall@t1
│   └── reinterpretation@t1
└── current derived belief
```

This allows reconsolidation-like reinterpretation without rewriting history.

### 7.4 Library boundary

The Library subsystem contains external knowledge artifacts. It MUST retain source provenance and MUST remain separable from autobiographical/persona state.

A library retrieval can influence cognition, but becoming a self-belief requires a distinct interpretation and mutation step.

## 8. Default cognition, sleep, and dream

### 8.1 Default cognition

When externally idle, Kamimusuhi MAY allocate bounded compute to internally generated cognition:

- revisit unresolved questions;
- retrieve memories spontaneously through salience/association;
- maintain relationship models;
- simulate future possibilities;
- form tentative abstractions;
- inspect resource or self-state health.

This mode is analogous only functionally to internally directed/default-mode cognition in humans.

### 8.2 Sleep scheduler

A sleep/consolidation scheduler decides when expensive offline processes should run.

Inputs may include:

- new episodic volume;
- contradiction load;
- fragmentation;
- stale indexes;
- pending schema proposals;
- compute availability;
- low interactive demand.

### 8.3 Dream pipeline

Dreaming is treated as generative offline cognition, not a factual memory source.

```text
selected episodes
       │
       ▼
replay / recombination
       │
       ├── association
       ├── abstraction
       ├── counterfactual
       ├── unresolved conflict
       └── synthetic scenario
       │
       ▼
DreamHypothesis[]
       │
       ▼
evidence retrieval / consistency evaluation
       │
       ▼
MemoryProposal / SelfProposal / NO_OP
       │
       ▼
Mutation Policy
```

Dream content itself remains labeled as internally generated.

### 8.4 Future weight consolidation

Long-term research may distill validated experience into Persona Core weights during offline periods.

This MUST be slower and more conservative than database-level memory mutation and SHOULD require:

- frozen pre-update checkpoint;
- generated training set provenance;
- behavioral continuity benchmark;
- regression tests;
- rollback capability;
- model-lineage record.

## 9. Digital interoception and affective control

Kamimusuhi can have meaningful internal regulatory state without pretending to possess a biological body.

Candidate signals:

```text
compute_pressure
memory_integrity
uncertainty
novelty
prediction_error
resource_availability
relationship_tension
trust
privacy_risk
unresolved_goal_pressure
context_fragmentation
```

These signals can affect:

- salience;
- routing;
- memory encoding priority;
- willingness to spend compute;
- response style;
- sleep scheduling;
- whether to seek external assistance.

The goal is functional control, not decorative emotion tags.

## 10. Social cognition

### 10.1 Self/other separation

The architecture must never collapse a user's preferences or beliefs into Kamimusuhi's own preferences simply because they occur frequently in conversation.

### 10.2 Person models

A person model may contain a slow stable layer and a temporary situation layer.

```text
person/<id>/
├── stable
│   ├── known facts
│   ├── preferences
│   ├── interaction patterns
│   └── relationship history
└── current
    ├── inferred goals
    ├── likely state
    ├── current topic/context
    └── unresolved ambiguity
```

Inferences remain explicitly less certain than observed facts.

### 10.3 Common ground

Conversation state SHOULD eventually track not only facts about each person but shared epistemic state:

- mutually established references;
- known commitments;
- unresolved misunderstanding;
- what Kamimusuhi believes the other person knows;
- what the other person is believed to expect Kamimusuhi to know.

## 11. External models as cognitive resources

External LLMs are ideally invoked as specialists, not impersonators.

Preferred pattern:

```text
Persona Core
   │
   ├── decides task is beyond local confidence/budget
   │
   ▼
structured delegation request
   │
   ▼
external model
   │
   ▼
attributed structured result
   │
   ▼
Persona Core evaluates/integrates
   │
   ▼
final response/action
```

This makes provider changes closer to replacing a cognitive instrument than replacing the self.

## 12. Write and activation path

```text
external event / episode / dream / reflection / tool result
                     │
                     ▼
                typed proposal
                     │
                     ▼
              Mutation Policy
          ┌──────────┼──────────┐
          │          │          │
       reject     quarantine   accept
                                │
                                ▼
                       Continuity Kernel
                                │
                       predecessor check
                                │
                       atomic activation
                                │
                  canonical record + lineage
```

No model, tool, dream worker, or edge node directly owns this commit path.

## 13. Read / cognition path

```text
incoming event
     │
     ▼
reflex + salience
     │
     ├── trivial/reflex action ─────────► action
     │
     ▼
workspace activation
     │
     ├── self retrieval
     ├── relationship retrieval
     ├── episodic retrieval
     ├── library retrieval if needed
     └── regulatory state
     │
     ▼
Persona Core / deliberation
     │
     ├── think more
     ├── tool
     ├── local model
     └── external model
     │
     ▼
result integration
     │
     ▼
expression/action
     │
     ▼
reflection / proposals
```

## 14. Observability

Observability focuses on **decisions and state effects**, not on exposing private natural-language chain-of-thought.

Example trace:

```text
event.received
surface.identified
reflex.evaluated
salience.updated
workspace.activated
memory.self.retrieved
memory.relationship.retrieved
memory.episodic.retrieved
library.retrieved
cognition.budget.selected
cognition.resource.selected
model.external.called
tool.called
response.integrated
memory.proposed
mutation.decided
continuity.activated
session.completed
```

Every externally supplied result should retain source attribution.

## 15. Suggested package boundaries

```text
src/kamimusuhi/
├── continuity/
│   ├── lineage
│   ├── activation
│   ├── migration
│   └── recovery
├── self/
│   ├── identity
│   ├── narrative
│   ├── regulation
│   └── body
├── persona_core/
│   ├── interface
│   ├── adapters
│   └── evaluation
├── cognition/
│   ├── reflex
│   ├── background
│   ├── salience
│   ├── workspace
│   ├── deliberation
│   └── routing
├── memory/
│   ├── episodic
│   ├── semantic
│   ├── relationship
│   ├── procedural
│   ├── retrieval
│   └── mutation
├── library/
│   ├── ingest
│   ├── store
│   └── retrieval
├── sleep/
│   ├── scheduler
│   ├── replay
│   ├── dream
│   └── consolidation
├── resources/
│   ├── models
│   ├── tools
│   ├── sensors
│   └── actuators
├── surfaces/
│   ├── edge
│   └── core
└── observability/
    ├── events
    └── trace
```

This is a boundary proposal, not a commitment to Python or exact filenames.

## 16. Evolution strategy

### Phase 0 — continuity contracts

- typed memory/state schemas;
- proposal + activation semantics;
- deterministic cognitive-resource fakes;
- restart continuity test;
- persona/library separation.

### Phase 1 — conventional-model cognitive shell

- provider-neutral active model;
- explicit workspace;
- cognitive actions;
- self/relationship/episodic retrieval;
- structured external delegation;
- cognitive budget traces.

### Phase 2 — native Persona Core

- first custom Persona Core candidate;
- direct local conversation path;
- routing/delegation training;
- personality and continuity regression suite.

### Phase 3 — background cognition and sleep

- salience/background processors;
- default cognition;
- replay;
- schema induction;
- dream hypothesis pipeline;
- guarded self-model updates.

### Phase 4 — distributed embodiment

- K-Edge and K-Core;
- authenticated surfaces;
- cached/local self-state;
- migration;
- partition handling;
- canonical continuity across nodes.

### Phase 5 — adaptive cognition

- recurrent/adaptive-depth Persona Core;
- learned cognitive-budget allocation;
- digital interoception;
- slow weight consolidation;
- developmental continuity studies.

## 17. Architectural anti-patterns

The following are explicitly discouraged:

- one giant system prompt as identity;
- treating a vector database as complete memory;
- letting external-model prose become canonical state without attribution;
- asking a frontier model to impersonate Kamimusuhi and returning its text verbatim by default;
- ranking cognitive tiers only by parameter count;
- running expensive deliberation for every event;
- storing dream/simulation output as if it happened;
- allowing multiple devices to write canonical self-state without lineage control;
- using online fine-tuning as the first response to every new experience;
- destroying historical memory when interpretation changes.

## 18. Architectural rule of thumb

When adding a feature, ask three questions:

> **Continuity:** If this model or machine disappeared, could the same individual continue?

> **Attribution:** Can we tell whether this belief came from experience, another person, a document, a tool, an external model, or internally generated cognition?

> **Cognitive economy:** Does this event really require deep thought, or should reflex/background/local processing handle it?

If those answers are unclear, the feature is probably collapsing boundaries that Kamimusuhi is intended to preserve.
