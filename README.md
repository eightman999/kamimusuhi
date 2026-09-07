# Kamimusuhi

**Kamimusuhi** is an experimental persistent cognitive architecture for an AI that can remember, change, and continue to exist across sessions.

The project is named after **Kamimusuhi-no-Kami (神産巣日神)**, one of the *Kotoamatsukami* appearing at the beginning of the Kojiki. The name is used here as a metaphor for generation, relation, and continuity—not as a claim about religion or theology.

> The goal is not to build a chatbot with a long prompt. The goal is to build an inspectable system in which identity, memory, knowledge, reasoning, and tools can persist and evolve without collapsing into one opaque context window.

## Goals

Kamimusuhi aims to provide:

- **Persistent identity** — a stable persona and self-model that survive process restarts and model changes.
- **Separated memory domains** — personal/episodic memory is kept distinct from the external knowledge library.
- **Second-brain integration** — notes, documents, past conversations, and other user-owned knowledge can be indexed and retrieved without becoming the persona itself.
- **Model and tool routing** — different models, local or remote, can be selected according to task, cost, latency, privacy, and capability.
- **Reflection and consolidation** — raw interaction history can be transformed into durable memories instead of endlessly replaying transcripts.
- **Inspectability** — important state changes should be attributable, reviewable, reversible, and debuggable.
- **Provider independence** — identity and memory should belong to Kamimusuhi, not to a single LLM vendor.

## Non-goals

Kamimusuhi is not intended to be:

- a single giant system prompt;
- a thin wrapper around one model API;
- an autonomous system allowed to mutate its own durable state without policy or audit;
- a vector database presented as a complete memory architecture;
- a claim of consciousness or personhood.

## Conceptual model

```text
                    ┌────────────────────┐
                    │     Interfaces     │
                    │ chat / voice / API │
                    └─────────┬──────────┘
                              │
                    ┌─────────▼──────────┐
                    │   Runtime / Core   │
                    │  session + policy │
                    └──────┬─────┬──────┘
                           │     │
                ┌──────────▼┐   ┌▼─────────────┐
                │ Cognition │   │    Router    │
                │ plan/act  │   │ model/tools │
                └─────┬─────┘   └──────┬──────┘
                      │                │
        ┌─────────────▼────────────────▼─────────────┐
        │                 Context Builder             │
        └───────────────┬───────────────┬────────────┘
                        │               │
              ┌─────────▼──────┐ ┌──────▼──────────┐
              │ Persona/Memory │ │     Library     │
              │ self + history │ │ external facts │
              └─────────┬──────┘ └──────┬──────────┘
                        │               │
                 ┌──────▼───────────────▼──────┐
                 │ storage / index / provenance │
                 └──────────────────────────────┘
```

The detailed requirements are in [`spec.md`](./spec.md), and the component boundaries and data flow are described in [`architecture.md`](./architecture.md).

## Design principles

1. **Memory is data, not prompt text.** Durable memory has structure, provenance, confidence, lifecycle, and revision history.
2. **Persona is not the library.** What the AI is and what it knows from external sources are separate domains.
3. **Retrieval is selective.** Persistence does not mean injecting all history into every request.
4. **State mutation is explicit.** Durable changes should pass through a policy boundary and produce an audit record.
5. **Models are replaceable.** LLMs are cognitive engines used by the runtime, not the owner of the system state.
6. **Forgetting is a feature.** Supersession, decay, deletion, and conflict resolution are first-class operations.
7. **Local-first where practical.** Sensitive durable state should be able to remain under the operator's control.

## Status

**Pre-alpha / architecture phase.**

The first milestone is a minimal vertical slice that can:

1. accept a conversation turn;
2. route it to a model;
3. retrieve persona memory and library context separately;
4. generate a response;
5. propose durable memories;
6. validate and store approved memory mutations with provenance;
7. reconstruct a later session from durable state.

See the repository Issues for the implementation sequence.

## Repository layout (planned)

```text
kamimusuhi/
├── README.md
├── spec.md
├── architecture.md
├── src/
│   └── kamimusuhi/
│       ├── runtime/
│       ├── cognition/
│       ├── router/
│       ├── persona/
│       ├── memory/
│       ├── library/
│       ├── context/
│       ├── policy/
│       └── observability/
├── tests/
└── docs/
```

Mythological names may be used as codenames for subsystems later, but public interfaces should remain understandable without knowledge of Japanese mythology.

## License

MIT. See [`LICENSE`](./LICENSE).
