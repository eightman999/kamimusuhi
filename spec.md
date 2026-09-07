# Kamimusuhi Specification

Status: **Draft / pre-alpha**  
Target: **v0.1 vertical slice**

This document defines the behavior Kamimusuhi should provide independently of a specific model, database, UI, or deployment environment.

## 1. Product definition

Kamimusuhi is a persistent AI runtime whose durable identity and memory exist outside any individual model context window.

A compliant implementation must be able to terminate, restart, change model providers, and begin a new session while preserving intentional durable state.

## 2. Core invariants

The following are architectural invariants rather than optional features.

### 2.1 Persona and library are separate domains

**Persona memory** represents facts and state that participate in continuity of the agent/user relationship: preferences, prior interactions, commitments, learned interaction patterns, and explicit self-model state.

**Library knowledge** represents external material: documents, notes, books, source code, imported conversations used as reference material, web captures, and other knowledge artifacts.

A library document MUST NOT silently become persona memory merely because it was retrieved or discussed.

### 2.2 Durable memory is structured

Every durable memory item MUST support, at minimum:

- stable ID;
- memory domain/type;
- normalized content;
- creation time;
- provenance/source reference;
- confidence or evidential status;
- lifecycle state;
- revision/supersession relation where applicable.

### 2.3 Durable mutation is explicit

Model output alone MUST NOT directly overwrite durable state.

Memory creation, update, merge, supersession, and deletion MUST pass through a mutation boundary that can apply policy and emit an audit event.

### 2.4 Models are replaceable

No core state may depend on hidden state inside one LLM provider. Model-specific adapters are permitted, but durable identity, memory, routing policy, and provenance MUST remain portable.

### 2.5 Retrieval is bounded

A request MUST receive only context selected for that request. The system MUST NOT depend on replaying the entire lifetime history.

## 3. Functional requirements

### FR-001 Session lifecycle

The runtime SHALL:

1. create a session;
2. accept one or more interaction turns;
3. persist session metadata;
4. end the session cleanly;
5. restore relevant durable state in a later session.

A process restart between steps 4 and 5 MUST NOT destroy durable state.

### FR-002 Model routing

The router SHALL expose a provider-neutral request interface.

Routing MAY consider:

- capability;
- privacy/locality;
- latency;
- monetary cost;
- context length;
- modality;
- task class;
- explicit user/operator policy;
- observed model performance.

The first implementation may use static rules, but the routing decision MUST be observable.

### FR-003 Persona memory retrieval

The system SHALL retrieve persona memories independently of library retrieval.

Retrieval SHOULD combine structured filters and semantic relevance rather than vector similarity alone.

### FR-004 Library retrieval

The system SHALL ingest and retrieve external knowledge artifacts without treating them as identity state.

Each retrieved passage SHALL retain enough provenance to identify its source artifact.

### FR-005 Context construction

A Context Builder SHALL compose the model input from independently addressable sections such as:

- system/runtime policy;
- persona definition;
- retrieved persona memory;
- retrieved library knowledge;
- current session context;
- tool state;
- task-specific instructions.

The resulting context SHOULD be inspectable in debug mode.

### FR-006 Memory proposal

After an interaction, the cognition/reflection layer MAY emit zero or more memory proposals.

A proposal SHALL contain:

- proposed operation (`create`, `update`, `merge`, `supersede`, `delete`);
- target domain;
- candidate content;
- source/provenance;
- confidence;
- rationale or machine-readable evidence reference.

### FR-007 Memory policy

The mutation layer SHALL validate proposals before they reach durable storage.

Policy MUST be capable of rejecting at least:

- low-confidence guesses represented as facts;
- duplicate memories;
- unsupported persona mutations caused only by retrieved library text;
- invalid cross-domain writes;
- malformed records.

### FR-008 Conflict and supersession

Contradictory durable facts SHOULD NOT be resolved by destructive overwrite by default.

The store SHALL support superseding an older fact while preserving its history and provenance.

### FR-009 Forgetting and deletion

The architecture SHALL support deliberate removal of durable information.

Deletion MUST be distinguishable from retrieval decay/ranking. Implementations SHOULD support tombstones or equivalent audit-safe semantics where appropriate.

### FR-010 Auditability

The runtime SHALL emit structured events for significant decisions, including:

- model routing;
- retrieval;
- tool invocation;
- memory proposal;
- memory mutation decision;
- durable mutation;
- failures.

Secrets and unnecessary private payloads SHOULD NOT be copied into logs.

### FR-011 Tool execution

Tools SHALL be exposed through explicit capability adapters.

A tool invocation SHALL identify:

- tool/capability;
- supplied arguments or a safe redacted representation;
- result status;
- call/session correlation ID.

Write-capable or externally consequential tools SHOULD be distinguishable from read-only tools.

### FR-012 Provider independence

At least two model adapters SHOULD be supportable without changing the memory schema or application-facing runtime API.

The v0.1 implementation only needs one working adapter if interfaces and tests prove substitutability.

## 4. Memory model

The initial schema should distinguish at least the following logical categories.

### Persona domain

- `identity` — stable persona/self-description;
- `preference` — interaction/user preferences;
- `relationship` — durable relationship context and commitments;
- `episodic` — notable past interactions/events;
- `procedure` — learned recurring ways of doing things.

### Library domain

- `artifact` — source document/file/note;
- `chunk` — addressable excerpt;
- `metadata` — title, author, timestamps, tags, URI/hash;
- `embedding/index` — retrieval representation, replaceable without altering source truth.

### Session domain

Session history is not automatically durable persona memory. It is a working record from which durable memory proposals may later be derived.

## 5. Minimum data contract

A conceptual durable memory record:

```json
{
  "id": "mem_...",
  "domain": "persona",
  "kind": "preference",
  "content": "...",
  "status": "active",
  "confidence": 0.9,
  "created_at": "...",
  "updated_at": "...",
  "source": {
    "type": "conversation_turn",
    "ref": "..."
  },
  "supersedes": [],
  "tags": []
}
```

The exact storage schema may change; these semantics should remain.

## 6. Non-functional requirements

### NFR-001 Inspectability

A developer must be able to answer, for a generated response:

- which model was selected and why;
- which memories were retrieved;
- which library passages were retrieved;
- which tools ran;
- what durable mutations were proposed and accepted/rejected.

### NFR-002 Recoverability

A failed model or tool call MUST NOT leave partially committed durable state.

### NFR-003 Testability

Core memory, routing, context construction, and policy behavior MUST be testable without calling a paid remote model.

### NFR-004 Local operation

The architecture SHOULD permit local storage and local-model adapters. Cloud services may be optional integrations rather than architectural requirements.

### NFR-005 Data migration

Durable state SHOULD carry schema versions, and migrations MUST be possible without discarding provenance/history.

## 7. v0.1 acceptance test

The first milestone is complete when an automated or reproducible demo proves all of the following:

1. Start Kamimusuhi with an empty profile.
2. Conduct a short conversation containing one explicit durable preference.
3. Store the raw session separately from durable persona memory.
4. Produce a memory proposal from that conversation.
5. Approve/store the proposal through the mutation policy.
6. Terminate the runtime completely.
7. Restart it in a new session.
8. Retrieve the durable preference when relevant.
9. Retrieve an imported library document through the independent library path.
10. Show an audit trace proving which memory and source passages influenced the response.

Passing this test establishes persistence and separation of concerns; it does not imply autonomous self-development or human-equivalent memory.

## 8. Deferred questions

These are intentionally left open for later design work:

- memory consolidation schedule and decay model;
- graph memory vs relational/document storage;
- embedding model and re-index strategy;
- learned model routing;
- multi-agent or multi-persona support;
- voice/perception and continuous presence;
- background reflection scheduling;
- user-facing memory inspection/editing UX;
- cryptographic integrity or signed provenance;
- synchronization across multiple devices.
