# Phase 2 implementation plan

Phase 1 (W0–W5) proved the identity boundary: a persistent self survives
process termination and cognitive-resource replacement, and provenance is
decided structurally rather than by reading text. See
[`phase-1-implementation-result.md`](./phase-1-implementation-result.md) for
what that did and did not establish.

Phase 2 starts where that left a gap. Phase 1 has exactly one cognitive
resource, in one slot, chosen by editing a config file. There is no way to say
"this task must not leave the machine", or "this one is trivial, do not spend a
remote call on it", or "this needs depth". Every such decision is currently
made by a human editing JSON.

This document plans **W6 only**. Later waves are named for orientation and are
not authorized by this document.

---

## W6 — Cognitive Resource Router and secure transport

### Why this wave, and why now

Two things force each other. Routing without transport security is a router
that can only choose between local endpoints, which is not a choice. Transport
security without routing is a better pipe to the single provider a human
picked. The wave that makes either useful is the one that does both:

- a **deterministic rule-based router** that picks a resource from declared
  capabilities and a declared task requirement, and records why;
- **TLS**, so "route this to a remote provider" is a thing that can safely
  happen at all, and so "route this locally only" becomes a meaningful refusal
  rather than the only option.

The router is the *policy* boundary for delegation, in the same sense that
`MutationPolicyV0` is the policy boundary for mutation: deterministic, auditable,
and refusing rather than guessing. Nothing about it is learned in W6.

### Scope

**1. Capability metadata.** Each registered resource declares what it is:

- locality/privacy class (local process, local network, external service);
- modality (text now; the field exists so adding one is not a schema change);
- context capacity (tokens or an equivalent bound);
- latency and cost class;
- availability/health, as a declared and observed state;
- quality/benchmark metadata, declared by configuration, not measured by us.

Declared metadata is configuration, not truth. A provider claiming to be local
does not make it local; the *locality class* is asserted by the operator in the
runtime config, which is the same trust model as the endpoint URL itself.

**2. Routing input.** A request to the router carries what the task needs:

- task class;
- privacy constraint;
- urgency;
- required depth;
- context size;
- cost budget.

**3. Deterministic routing.** Given candidates and a requirement, the router
produces a decision: a chosen resource, or a refusal, plus a stable reason
code. Same inputs, same decision. No scoring model, no learning, no
randomness, no time-dependence beyond declared health.

**4. Privacy-constrained routing.** A `LocalOnly` requirement must never select
an external resource. Failing to find a local candidate is a **refusal**, never
a downgrade to a remote one. This is the routing equivalent of fail-closed
mutation: the wrong answer is worse than no answer.

**5. The delegation path.** Persona Core → router → resource → attributed
material → Persona Core, with the material entering the workspace as
`EXTERNAL_RESOURCE_RESULT` exactly as in W3–W5. Routing changes *which*
resource answers, never what kind of thing an answer is.

**6. Routing decisions in the trace.** The decision, the reason, the candidates
considered and the requirement are operational trace, not canonical state — a
routing decision is not something the individual believes. Secrets, prompts and
response bodies stay out, as in W4/W5.

**7. TLS at the adapter boundary.** Introduce an audited TLS stack (`rustls`)
in `kamimusuhi-resource-http`. **We do not implement TLS or certificate
validation ourselves.** Certificate verification, hostname verification and the
handshake belong to the library. Plain-HTTP local endpoints must keep working
unchanged.

**8. Invariance under failure and replacement.** Everything W5 established
continues to hold with a router in the path: a provider that times out, refuses,
rate-limits or disappears does not move the head, does not mint an individual,
and does not alter durable memory; swapping providers or rewriting the config
does not change the self or memory schema.

### Acceptance criteria

- [ ] Resources declare capability metadata: locality/privacy, modality, context
      capacity, latency/cost, availability/health, quality.
- [ ] A routing request carries task class, privacy constraint, urgency,
      required depth, context size and cost budget.
- [ ] The router is deterministic: identical inputs yield an identical decision,
      asserted by test.
- [ ] Every decision carries a stable reason code, including refusals.
- [ ] A `LocalOnly` privacy constraint never selects an external resource, and
      refuses when no local candidate qualifies.
- [ ] A candidate that fails a hard constraint (context capacity, cost budget,
      health) is excluded with a reason, not silently ranked last.
- [ ] The Persona Core delegation path runs through the router end to end, and
      the result still arrives as `EXTERNAL_RESOURCE_RESULT` with resource and
      call attribution.
- [ ] Routing decision, reason, requirement and considered candidates appear in
      the JSONL trace, correlated by turn and resource IDs.
- [ ] The trace still contains no secret, no prompt and no response body.
- [ ] TLS is provided by `rustls`; no certificate or hostname verification is
      hand-written anywhere in this repository.
- [ ] `https://` endpoints work; `http://` local endpoints keep working.
- [ ] Tested against a local TLS fixture server: valid certificate, invalid
      certificate, hostname mismatch, handshake failure, timeout.
- [ ] TLS failures classify distinctly and carry no certificate or response
      content into logs or errors.
- [ ] Retry and error attribution behave for TLS exactly as for plain HTTP: one
      logical call, attempts counted, `resource_calls` correlated to its turn.
- [ ] Provider failure leaves `IndividualId`, root commit, head and durable
      memory unchanged — re-asserted with a router in the path.
- [ ] Provider or config replacement changes no self/memory schema.
- [ ] W1–W5 tests all still pass, unweakened.
- [ ] Mutation testing confirms the new tests fail when the behaviour they
      describe is broken.
- [ ] `scripts/ci-local.sh` green from a clean checkout.

### Explicit non-goals — do not proceed past W6

W6 stops when the criteria above are met. The following are **not** in W6, and
finding that one of them would be convenient is not a reason to start it:

- learned or adaptive routing, scoring models, bandit selection, any routing
  that changes behaviour based on past outcomes;
- autonomous goal pursuit, background or default cognition, any loop that runs
  without a turn;
- streaming responses;
- tool calling / function calling;
- embeddings, vector storage, semantic retrieval;
- cost-optimisation learning or budget forecasting;
- multi-agent orchestration, K-Edge/K-Core, distributed handoff, multi-writer;
- sophisticated provider failover: circuit breakers, hedged requests,
  automatic multi-provider fallback chains;
- advanced retry policy — `Retry-After` handling, jitter, adaptive backoff.
  If wanted, these get their own issue;
- Claim/Belief graph, state→state dependency graph;
- self-domain mutation;
- retention/deletion enforcement;
- speech, sensors, actuators, an Executor;
- private or online weight learning.

If a later-wave type is strictly necessary to compile W6, add the smallest
boundary declaration and no implementation.

### Known debt carried forward

Not to be fixed opportunistically in W6:

- Persona Core is a deterministic fixture, not a model;
- retrieval is lexical, with no measured behaviour at corpus scale;
- trace keeps one rotated generation and no retention policy;
- ID seeds are operator-managed;
- `state_records.kind` vocabulary is derived from the operation.

---

## Later waves (orientation only, not authorized here)

- **W7** — Persona Core boundary hardening and a real model behind it.
- **W8** — belief/claim layer: state→state dependency, re-evaluation on
  retraction.
- **W9** — retention and deletion closure, including derived artifacts.
- **W10** — background/default cognition under an explicit budget.

Each needs its own plan document and its own acceptance criteria before any
code is written for it.
