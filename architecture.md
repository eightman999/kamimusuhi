# Kamimusuhi Architecture

Status: **Draft / pre-alpha**

This document describes the initial component boundaries for Kamimusuhi. It is intentionally implementation-agnostic where possible.

## 1. Architectural thesis

Kamimusuhi treats an LLM as a replaceable cognitive engine inside a larger persistent runtime.

The durable system state lives outside the model context window. A request is reconstructed from explicit state, selected memories, selected library knowledge, current session context, policy, and tool capabilities.

```text
Input
  │
  ▼
Interface Adapter
  │
  ▼
Runtime / Session Manager
  │
  ├──────────────► Policy
  │
  ├──────────────► Router ─────────────► Model Adapter(s)
  │
  ├──────────────► Persona Retrieval
  │
  ├──────────────► Library Retrieval
  │
  ▼
Context Builder
  │
  ▼
Cognition / Generation
  │
  ├──────────────► Tool Gateway ────────► Tools
  │
  ▼
Response
  │
  └──────────────► Reflection
                         │
                         ▼
                   Memory Proposals
                         │
                         ▼
                   Mutation Policy
                         │
                         ▼
                   Durable Stores
```

## 2. Major components

### 2.1 Runtime / Session Manager

Responsibilities:

- create and close sessions;
- assign correlation IDs;
- coordinate one interaction turn;
- enforce top-level policy boundaries;
- make dependencies explicit;
- ensure failures do not partially mutate durable state.

The runtime should contain little domain intelligence. It orchestrates components rather than becoming a monolith.

### 2.2 Router

The Router selects a model or execution backend for a task.

Input may include:

- task classification;
- required capabilities;
- privacy requirements;
- context size;
- latency target;
- budget/cost ceiling;
- availability/health;
- quality benchmark metadata.

Output should include both the selected backend and a machine-readable routing reason.

The first implementation may be a deterministic rules engine. Learned routing is deferred.

### 2.3 Model adapters

Model-specific details belong behind adapters.

```text
ModelClient
├── generate(request) -> response
├── capabilities() -> CapabilitySet
├── estimate_cost(request) -> estimate
└── health() -> status
```

Adapters may target local inference servers or remote APIs. No adapter owns durable persona or memory state.

### 2.4 Persona subsystem

The Persona subsystem represents durable continuity and self/relationship state.

It contains:

- explicit persona definition;
- durable persona memories;
- retrieval logic;
- memory confidence/provenance metadata;
- revision and supersession relationships.

Persona storage must remain logically separate from the Library even if both share a physical database.

### 2.5 Library subsystem

The Library is the second-brain/reference-knowledge layer.

Responsibilities:

- ingest source artifacts;
- preserve source identity and metadata;
- chunk/index material;
- retrieve relevant passages;
- return provenance with every passage;
- support re-indexing without changing source truth.

Potential sources include notes, documents, code, imported chat archives, and other user-owned material.

### 2.6 Context Builder

The Context Builder is the only component responsible for assembling model-facing context.

Conceptual order:

```text
1. Runtime/system policy
2. Persona definition
3. Retrieved persona memories
4. Retrieved library evidence
5. Current-session history
6. Tool definitions/state
7. Current task/input
```

Each section should retain attribution so debug output can explain why it was included.

The Context Builder should enforce token budgets per section rather than allowing one source to consume the full context window.

### 2.7 Cognition layer

The Cognition layer performs task execution around model calls.

Initial responsibilities:

- request planning when required;
- model invocation;
- tool-call loop;
- response generation;
- handing completed interaction state to Reflection.

The first version should remain deliberately simple. Complex autonomous planning is not required for persistence.

### 2.8 Tool Gateway

Tools are explicit capabilities, not arbitrary code embedded in prompts.

The gateway should distinguish:

- read-only vs mutating tools;
- local vs networked tools;
- low-risk vs consequential actions;
- synchronous request/response behavior vs future asynchronous architecture.

Every invocation produces an observable event.

### 2.9 Reflection

Reflection turns ephemeral session material into candidate durable state.

Reflection does **not** directly write memory.

It emits proposals such as:

```text
CREATE preference
UPDATE relationship fact
MERGE duplicate episodic memories
SUPERSEDE outdated fact
DELETE explicitly removed memory
NO_OP
```

This boundary prevents a model hallucination from automatically becoming permanent truth.

### 2.10 Mutation Policy

The Mutation Policy evaluates proposals before committing durable changes.

Checks may include:

- schema validity;
- provenance presence;
- confidence threshold;
- duplicate detection;
- contradiction detection;
- domain authorization;
- operator/user approval rules;
- retention/deletion policy.

All decisions should be auditable.

### 2.11 Observability

Observability is a first-class subsystem, not an afterthought.

A single turn should be reconstructable as a trace:

```text
session.turn.started
router.selected
persona.retrieve
library.retrieve
context.built
model.request
model.response
tool.called            # zero or more
reflection.proposed
memory.policy.decided
memory.committed       # zero or more
session.turn.completed
```

Payload logging must support redaction.

## 3. Storage boundaries

A simple initial deployment may use one database, but logical schemas should remain separate.

```text
storage
├── sessions
│   ├── sessions
│   └── turns
├── persona
│   ├── memories
│   ├── revisions
│   └── relations
├── library
│   ├── artifacts
│   ├── chunks
│   └── indexes
├── runtime
│   ├── routing_events
│   └── tool_events
└── audit
    └── mutation_events
```

Vector indexes are derived data. They should be rebuildable from canonical records.

## 4. Write path

Durable writes use a staged flow.

```text
Interaction
   │
   ▼
Raw session record
   │
   ▼
Reflection
   │
   ▼
MemoryProposal[]
   │
   ▼
Mutation Policy
   │
   ├── reject ─────────► audit only
   │
   └── accept
          │
          ▼
      transaction
          │
          ├── canonical record
          ├── revision/supersession edge
          └── audit event
```

The system should never require parsing past natural-language logs to determine the current canonical state.

## 5. Read path

```text
User input
   │
   ├──► task classification
   │
   ├──► persona query ──────► persona candidates
   │
   └──► library query ──────► source passages
                                 │
                ┌────────────────┘
                ▼
          Context Builder
                │
                ▼
             Model
```

Persona and Library retrieval may run in parallel but their results remain labeled and independently budgeted.

## 6. Suggested package structure

```text
src/kamimusuhi/
├── runtime/
│   ├── session.py
│   └── turn.py
├── cognition/
│   ├── engine.py
│   └── reflection.py
├── router/
│   ├── router.py
│   └── policy.py
├── models/
│   ├── base.py
│   └── adapters/
├── persona/
│   ├── schema.py
│   ├── store.py
│   └── retrieval.py
├── memory/
│   ├── proposal.py
│   ├── mutation.py
│   └── conflict.py
├── library/
│   ├── ingest.py
│   ├── store.py
│   └── retrieval.py
├── context/
│   ├── builder.py
│   └── budget.py
├── tools/
│   ├── gateway.py
│   └── registry.py
├── policy/
│   └── engine.py
└── observability/
    ├── events.py
    └── trace.py
```

This is a boundary proposal, not a commitment to Python or these exact filenames.

## 7. Mythological naming

The project name comes from Kamimusuhi. Internal codenames may optionally borrow from the three *Zōka Sanshin* while keeping technical API names explicit:

- **Minakanushi** — possible codename for the central runtime/router layer;
- **Takamimusubi** — possible codename for planning/orchestration/cognition;
- **Kamimusuhi** — project-level identity and the memory/continuity theme.

These names should remain optional aliases rather than obscuring the component's technical purpose.

## 8. Trust boundaries

Kamimusuhi should assume model output is untrusted until validated.

Key boundaries:

```text
External input ──[validate]──► Runtime
Library text  ──[label]─────► Context
Model output  ──[parse]─────► Actions / proposals
Tool request  ──[policy]────► Tool Gateway
Memory prop.  ──[policy]────► Durable state
```

Prompt injection in retrieved material must not grant permission to mutate persona memory or invoke consequential tools.

## 9. Evolution strategy

### Phase 0 — contracts

- schemas;
- interfaces;
- deterministic test doubles;
- trace format.

### Phase 1 — persistent vertical slice

- one model adapter;
- persona store/retrieval;
- library ingest/retrieval;
- context builder;
- proposal + mutation policy;
- restart persistence test.

### Phase 2 — routing and observability

- multiple model adapters;
- cost/capability-aware routing;
- richer traces;
- debugging UI/CLI.

### Phase 3 — consolidation

- conflict resolution;
- memory merging;
- confidence evolution;
- retention/forgetting;
- scheduled reflection.

### Phase 4 — continuous presence

- voice/perception interfaces;
- device synchronization;
- longer-running agency under explicit policy controls.

## 10. Architectural rule of thumb

When adding a feature, ask:

> If the current LLM provider disappeared tomorrow, would Kamimusuhi still know what it is, what it remembers, where its knowledge came from, and how to continue?

If the answer is no, too much durable state has leaked into the model layer.
