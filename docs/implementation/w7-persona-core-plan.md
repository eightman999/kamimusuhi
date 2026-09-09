# W7 — Persona Core boundary hardening and a real Persona model

W6 gave the runtime a router and secure transport, so a task can now reach a
real model. What it did not settle is *who speaks*. Today every user-facing
string still comes from a deterministic fixture, and the only real model in the
system is a generic cognitive resource — something the runtime delegates a
subtask to, not something that answers as Kamimusuhi.

W7 fixes that, and the point is not that answers get better.

---

## The claim

> Whatever an external cognitive resource produces is attributed cognitive
> material. The user-facing expression is always the output of the Persona Core
> path, and replacing the Persona Core implementation — fixture for real model,
> or one model for another — does not disturb `IndividualId`, continuity,
> durable memory, the Library, or resource attribution.

Two failure modes this exists to prevent:

**The passthrough.** A provider returns text and the runtime hands it to the
user. Once that shortcut exists, the "Persona Core" is decoration and the
individual's voice is whatever endpoint was configured last.

**The promotion.** A model says something and it becomes self-state. Phase 1
built a guarded mutation contract precisely so that nothing reaches durable
state without evidence, a proposal and a policy decision. A Persona backend is
not an exception to it, and W7 must not create one.

---

## Scope

**1. Real Persona Core backend.** At least one model-backed implementation of
the existing `PersonaCore` contract, speaking to an OpenAI-compatible endpoint
so that llama.cpp, Ollama or LM Studio can run it locally.

It shares HTTP and TLS transport with the resource adapter and shares nothing
else. A Persona backend is **not** a cognitive resource that happens to be
called for the final turn: different crate, different type, different config
namespace, different trace attribution. Calling an OpenAI-compatible endpoint
does not make something a Persona Core; implementing the Persona Core contract
does.

**2. A typed input envelope.** What reaches the Persona Core is sectioned and
attributed:

```text
CURRENT_INPUT
DURABLE_SELF / MEMORY
RELATIONSHIP
LIBRARY_EVIDENCE
EXTERNAL_RESOURCE_RESULT
SESSION_WORKING_STATE
```

Each section keeps its provenance and domain. A resource response is never
spliced into an unmarked system prompt, and neither Library text nor resource
output is promoted to "fact" or to "self" on the way in. The representation
stays typed right up to the serialization step at the backend boundary — which
is the only place the distinction could be lost, and therefore the only place
worth guarding.

**3. The full delegation path, end to end.** In a real process:

```text
user input → Persona Core → delegate request → Router
    → external cognitive resource → EXTERNAL_RESOURCE_RESULT
    → workspace → Persona Core → FINAL_EXPRESSION
```

No shortcut returns a resource string as the user response. That the final
expression is the product of a Persona invocation has to be *testable*, not
merely intended, and the fake Persona takes the same path as a real one.

**4. Trace and attribution.** One turn correlates the Persona invocation, the
routing decision, the selected resource, the resource call, the workspace
material and the final-expression event. Persona backend and external resource
are distinguishable in the trace.

Metadata only. No prompt text, no response text, no token, no `Authorization`,
no Library body, no relationship body, no durable-memory body.

**5. Failure semantics.** Persona-endpoint timeout, connection failure, TLS
failure, malformed response, provider error and authentication failure are all
controlled errors. In every case: no panic, no new individual, no head
movement, no change to durable self/memory or the Library. A Persona that fails
to generate is never a reason to mutate canonical state.

**6. Replacement.** Two tests:

- external resource A → B with the Persona backend fixed: identity and schema
  unchanged, only resource attribution differs;
- Persona backend replaced across a full restart — fixture → real, or one model
  → another: `IndividualId`, root commit, continuity head and durable memory
  unchanged.

Persona model weights are explicitly **not** defined as canonical identity in
W7. Whether they ever should be is a real question and not this wave's.

**7. Config namespaces.** Persona backend and routable resources are registered
separately. The Persona backend is never implicitly added to the router's
candidate set; a resource slot is never used as the Persona execution backend.
W6's "one candidate in practice" gap is relieved only as far as the end-to-end
test needs.

---

## Architectural invariants (must not weaken)

Carried forward from W1–W6 and re-asserted here:

- canonical mutation only through evidence → proposal → `MutationPolicy` →
  Continuity Kernel;
- domain separation decided structurally — origin, ownership, domain — never by
  reading payload text;
- Library and resource output are `ExternalMaterial` and confer no authority;
- `LocalOnly` routing refuses rather than downgrades;
- no certificate or hostname verification written in this repository;
- no credential in config, database, trace or error message;
- one logical resource call per question, however many physical attempts;
- `inspect` is read-only and claims no writer epoch;
- a runtime that cannot restore its individual fails closed and never mints a
  replacement.

New for W7:

- **the final user-facing expression is produced by a Persona Core
  invocation**, and no path returns external material directly;
- **a Persona backend cannot propose canonical change on its own authority** —
  its drafts go through the same policy as any other draft;
- **a Persona backend is not a router candidate.**

---

## Acceptance criteria

- [ ] Fake and real Persona Cores go through the same `PersonaCore` boundary.
- [ ] One turn succeeds against a real local OpenAI-compatible endpoint.
- [ ] The typed envelope reaches the backend with each section's provenance and
      domain intact, and is only flattened at serialization.
- [ ] Persona → delegation → router → external resource → workspace → Persona →
      final expression runs in a real process boundary test.
- [ ] No code path returns external material as the user-facing response.
- [ ] The final expression is demonstrably the output of a Persona invocation.
- [ ] Trace distinguishes external material from final expression.
- [ ] Trace distinguishes the Persona backend from resource backends.
- [ ] Persona invocation, routing decision, selected resource, resource call,
      workspace material and final expression correlate within one turn.
- [ ] Persona failures — timeout, connection, TLS, malformed, provider error,
      authentication — are controlled, classified, and leave `IndividualId`,
      head, durable memory and Library untouched.
- [ ] External resource replacement changes only resource attribution.
- [ ] Persona backend replacement across a restart changes no canonical state.
- [ ] Persona backend and routable resources live in separate config
      namespaces, and the Persona backend never appears as a router candidate.
- [ ] No secret, prompt or response body in database, trace or error.
- [ ] W1–W6 tests all pass, unweakened.
- [ ] At least two mutation tests confirm the new tests bite.
- [ ] `scripts/ci-local.sh` green from a clean clone.
- [ ] README documents how to run the real Persona smoke test.

---

## Non-goals — do not start these in W7

- belief/claim graph, state→state dependency, automatic belief formation;
- **any path where "the Persona model said it" becomes durable self-state** —
  self-modification stays inside the guarded mutation contract;
- retention/deletion closure;
- background or default cognition, autonomous goals;
- streaming, tool calling, embeddings;
- learned or adaptive routing;
- automatic health observation, cost or quality measurement;
- provider failover;
- K-Edge/K-Core, multi-agent, distributed runtime;
- online learning, LoRA, weight adaptation;
- optimising for personality or answer quality on any benchmark.

W7 ends when the acceptance criteria are met. W8 needs its own plan.
