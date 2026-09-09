# W7 — Persona Core boundary hardening and a model-capable Persona backend

> Verification correction (2026-09-10): transport and process-boundary tests use
> scripted endpoints. No actual LLM-server smoke run is recorded. Historical
> counts below describe the W7 baseline; subsequent fixes are documented in the
> [spec/implementation audit](./2026-09-10-spec-implementation-audit.md).

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

- [x] Fake and HTTP-backed Persona Cores go through the same `PersonaCore` boundary.
- [x] One turn succeeds through a local scripted OpenAI-compatible HTTP endpoint.
- [ ] One turn succeeds against an actual local LLM server, with model/version and
      run evidence recorded. This is not established by an HTTP fixture.
- [x] The typed envelope reaches the backend with each section's provenance and
      domain intact, and is only flattened at serialization. Complete continuity,
      session, authority, freshness, and source fields were repaired on 2026-09-10;
      the original W7 renderer did not preserve all of them.
- [x] Persona → delegation → router → external resource → workspace → Persona →
      final expression runs in a real process boundary test.
- [x] No code path returns external material as the user-facing response.
- [x] The final expression is demonstrably the output of a Persona invocation.
- [x] Trace distinguishes external material from final expression.
- [x] Trace distinguishes the Persona backend from resource backends.
- [x] Persona invocation, routing decision, selected resource, resource call,
      workspace material and final expression correlate within one turn.
- [x] Persona failures — timeout, connection, TLS, malformed, provider error,
      authentication — are controlled, classified, and leave `IndividualId`,
      head, durable memory and Library untouched.
- [x] External resource replacement changes only resource attribution.
- [x] Persona backend replacement across a restart changes no canonical state.
- [x] Persona backend and routable resources live in separate config
      namespaces, and the Persona backend never appears as a router candidate.
- [x] Credentials and raw provider-error/prompt bodies are not copied into
      diagnostics. Canonical user evidence and attributed successful resource
      material are intentionally stored through their own evidence paths;
      this is not a claim that the whole database contains no text.
- [x] W1–W6 tests all pass, unweakened.
- [x] At least two mutation tests confirm the new tests bite.
- [x] `scripts/ci-local.sh` green from a clean clone.
- [x] README documents how to run the real Persona smoke test.

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


---

## W7 implementation result

Implemented at the commit that added this section. Measured, not planned.

### What was built

`kamimusuhi-persona-http` — a separate crate from the resource adapter, sharing
HTTP/TLS transport and, since 2026-09-10, bounded provider-error classification.
`OpenAiCompatiblePersona` implements `PersonaCore`
and is addressed by `PersonaBackendId`, a different type from `ResourceId`, so
a Persona backend cannot be handed anywhere a resource is expected.

`PersonaEnvelope` in core groups an assembled workspace into sections —
continuity, durable self, relationship, episodic, library, external results —
plus `SessionWorkingState`. Grouping is by `WorkspaceDomain` alone; no item's
content is read. The one place sections become text is `render_envelope`, which
now preserves complete item metadata as JSON records (2026-09-10 correction).
`durable_self`
is always empty in phase 1: the self domain is reserved and nothing can write
it, and the section exists so a backend never has to infer self-state from
relationship state.

Configuration has two namespaces, `persona` and `resources`. The registry is
built only from `resources`, so the Persona backend is structurally unable to
appear as a router candidate.

### Measured

A CLI turn against a scripted local OpenAI-compatible endpoint (not an actual LLM):

```text
persona backend    openai-compatible / local / 0bb9ce41…
prompt sections    [CURRENT_INPUT] [RELATIONSHIP_MEMORY] [LIBRARY_EVIDENCE]
                   [EXTERNAL_RESOURCE_RESULT]
expression         scripted Japanese reply mentioning the stored preference
individual         …0003, unchanged
head               generation 1 before and after
memory             {"preference":"ほうじ茶"}, read from the database
```

Historical W7 baseline tests: **266 passing, 0 failed, 0 ignored** (from 246 at W6, +20). New:
`crates/kamimusuhi-persona-http` (12 unit) and
`crates/kamimusuhi-runtime/tests/persona_boundary.rs` (8, across real process
boundaries with two distinct HTTP endpoints — one Persona, one resource).

The passthrough test uses a Persona reply the resource never returns and a
resource reply the Persona never produces, so a shortcut is immediately
visible. It asserts the resource's text reaches the Persona *as input*, under
its own section heading, and is not what the user was told.

Mutation testing: returning external material as the response failed 5 of the 8
boundary tests; leaking the Persona backend into the router's candidate set
failed all 8. Both reverted, CI green after.

### Still not done

The W7 non-goals remain non-goals. Additionally, from this implementation:

- the model-backed Persona drafts nothing. `proposals` is always empty, because
  phase 1 has no path from generated text to durable state and building one
  here would put self-modification outside the guarded mutation contract. What
  a Persona-originated draft should even look like is a W8 question;
- the system instruction is fixed configuration. Nothing about how Kamimusuhi
  should sound has been designed;
- one turn, no conversation history within a session. `SessionWorkingState`
  carries counters, not messages;
- the smoke test has been run against a scripted local endpoint, not against a
  real model server. The wire format is what llama.cpp, Ollama and LM Studio
  serve, and `scripts/persona-smoke.sh` points at them, but no run against an
  actual model is recorded here;
- `PersonaBackendId` derived from a `--persona-url` is a digest of endpoint and
  model. Two different models behind one URL and name would collide.
