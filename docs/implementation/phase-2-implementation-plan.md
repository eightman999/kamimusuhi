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

- [x] Resources declare capability metadata: locality/privacy, modality, context
      capacity, latency/cost, availability/health, quality.
- [x] A routing request carries task class, privacy constraint, urgency,
      required depth, context size and cost budget.
- [x] The router is deterministic: identical inputs yield an identical decision,
      asserted by test.
- [x] Every decision carries a stable reason code, including refusals.
- [x] A `LocalOnly` privacy constraint never selects an external resource, and
      refuses when no local candidate qualifies.
- [x] A candidate that fails a hard constraint (context capacity, cost budget,
      health) is excluded with a reason, not silently ranked last.
- [x] The Persona Core delegation path runs through the router end to end, and
      the result still arrives as `EXTERNAL_RESOURCE_RESULT` with resource and
      call attribution.
- [x] Routing decision, reason, requirement and considered candidates appear in
      the JSONL trace, correlated by turn and resource IDs.
- [x] The trace still contains no secret, no prompt and no response body.
- [x] TLS is provided by `rustls`; no certificate or hostname verification is
      hand-written anywhere in this repository.
- [x] `https://` endpoints work; `http://` local endpoints keep working.
- [x] Tested against a local TLS fixture server: valid certificate, invalid
      certificate, hostname mismatch, handshake failure, timeout.
- [x] TLS failures classify distinctly and carry no certificate or response
      content into logs or errors.
- [x] Retry and error attribution behave for TLS exactly as for plain HTTP: one
      logical call, attempts counted, `resource_calls` correlated to its turn.
- [x] Provider failure leaves `IndividualId`, root commit, head and durable
      memory unchanged — re-asserted with a router in the path.
- [x] Provider or config replacement changes no self/memory schema.
- [x] W1–W5 tests all still pass, unweakened.
- [x] Mutation testing confirms the new tests fail when the behaviour they
      describe is broken.
- [x] `scripts/ci-local.sh` green from a clean checkout.

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

- **W7** — Persona Core boundary hardening and a real model behind it. *Done;
  see [`w7-persona-core-plan.md`](./w7-persona-core-plan.md).*
- **W8A** — the Persona-originated draft contract.
- **W8B** — the claim/belief layer.
- **W9** — retention and deletion closure, including derived artifacts.
- **W10** — background/default cognition under an explicit budget.

Each needs its own plan document and its own acceptance criteria before any
code is written for it.

### Why W8 became W8A and W8B

W8 was "belief/claim layer: state→state dependency, re-evaluation on
retraction". Building W7 made it clear that this is two waves, and that doing
them as one would get the order wrong.

W7 ends with a model-backed Persona that drafts **nothing**: `proposals` is
always empty, because phase 1 has no path from generated text to durable state
and inventing one inside W7 would have put self-modification outside the
guarded mutation contract. So the first missing piece is not a belief graph. It
is the far smaller question of what a Persona-originated draft even *is* — how
generated text becomes a candidate for canonical change without the generating
being the authority for it.

That question is answerable on its own, and answering it first means the belief
layer is built on top of a draft path that already exists rather than being
designed alongside it.

**W8A — Persona-originated draft contract.** A typed `PersonaDraft` carrying:
a draft ID; the proposed domain and operation; a candidate *structured* value,
not prose; supporting `EvidenceId`s; supporting `MemoryId`s; Library and
resource attribution for anything the draft leaned on; `origin =
PersonaInference`; and turn/session correlation.

The load-bearing rule: **the model's output is not evidence.** A draft points
at evidence that already exists; it does not create any by being produced. A
draft whose support does not hold up is rejected, and rejection is the normal
case rather than a failure. The draft joins the existing path —
draft → validation → proposal → `MutationPolicy` → Continuity Kernel →
activation — at the point where a draft already enters it today, so W8A adds an
origin, not a second route into canonical state.

No belief graph, no state→state dependency, no re-evaluation: W8A is the
contract only.

**W8B — claim/belief layer.** Only after W8A. State that depends on other
state, dependency recorded rather than inferred, and re-evaluation when
something underneath is retracted or corrected. This is where "the individual
concluded X, and X rested on Y, and Y is now withdrawn" becomes a thing the
system can act on — and it needs W8A's draft contract to have a well-defined
way for a conclusion to arise at all.

Neither is authorized by this document. W8A needs its own plan before any code
is written for it.

---

## W6 implementation result

Implemented at the commit that added this section. Measured, not planned.

### Router

`kamimusuhi-core::routing`. Capability metadata is `ResourceCapabilities`
(locality, modalities, context capacity, latency, cost, quality, health);
a requirement is `RoutingRequest` (task class, privacy, urgency, required
depth, context size, cost budget, modality).

`RuleRouter` applies hard constraints in a fixed order — privacy first, so a
privacy refusal is never masked by a cheaper complaint about cost — then picks
among survivors by cheapest, then healthy-before-degraded, then highest
declared quality, then fastest, then slot name. The last tie-break makes the
order total, so there is always exactly one answer.

Refusal is an error, not a fallback: `LocalOnly` with no local candidate
returns `NoEligibleResource` carrying a verdict per candidate. The integration
test asserts the fixture server received **zero** requests in that case — the
refusal happens before anything leaves the process.

Every decision carries `considered`: one verdict per candidate, in slot order,
with a stable reason code. `SELECTED`, `NOT_PREFERRED`, `PRIVACY_EXCLUDED`,
`MODALITY_UNSUPPORTED`, `CONTEXT_TOO_LARGE`, `COST_OVER_BUDGET`,
`TOO_SLOW_FOR_URGENCY`, `QUALITY_BELOW_DEPTH`, `UNHEALTHY`,
`NO_ELIGIBLE_RESOURCE`.

### TLS

`rustls` with the `ring` provider. **No certificate or hostname verification
is written in this repository** — `crates/kamimusuhi-resource-http/src/tls.rs`
chooses trust anchors and classifies `rustls`'s errors, and contains no
verification logic. There is no option to disable verification.

Trust anchors are the bundled `webpki-roots` set, or a PEM file for a private
CA; a private-CA setting *replaces* the public set rather than adding to it, so
pointing at an internal CA does not silently keep trusting the public web. A
missing or unusable PEM is an error rather than a fallback to the defaults.

Failures classify as `certificate`, `hostname_mismatch`, `handshake` or
`trust_anchors`, and are **not retried** — a certificate that does not validate
will not validate on the next attempt, and retrying would blunt a security
signal. `http://` endpoints keep working; the scheme decides the transport, so
neither direction is ever silently changed.

Three real bugs surfaced while building this, all found by tests rather than
by reading:

- socket deadlines were being set on a socket obtained through `StreamOwned`'s
  `Deref`, so they never reached the real connection;
- only the first resolved address was tried, which breaks any dual-stack name
  whose AAAA is unreachable — `localhost` on this machine;
- accepted sockets inherit the listener's non-blocking flag on macOS, which
  made fixture reads return `EAGAIN` and looked like a client that sent
  nothing. That one was latent in the W5 plain-HTTP fixture too.

A peer closing without TLS `close_notify` is tolerated. **Correction, 2026-09-10:**
the original claim that truncation necessarily surfaced as a parse failure was
incorrect: a truncated body could still be valid JSON, and Content-Length was
not checked. The hardened transport now requires exact Content-Length or a
complete chunked message; EOF alone is not a completeness signal. See the
[regression audit](./2026-09-10-spec-implementation-audit.md).

### Acceptance

Every criterion in the W6 list above is met. Tests: **246 passing, 0 failed,
0 ignored** (from 215 at v0.1 closeout). New suites:
`crates/kamimusuhi-resource-http/tests/tls_provider.rs` (7, against a real
`rustls` server with per-test `rcgen` certificates) and
`crates/kamimusuhi-runtime/tests/routing_delegation.rs` (7, across real process
boundaries).

Mutation testing confirmed the tests bite: removing the privacy constraint
failed exactly the three routing tests and the local-only integration test;
misclassifying a hostname mismatch failed exactly the classification unit test
and the wrong-host integration test. Both reverted, CI green after.

### Still not done

The W6 non-goals above are all still non-goals. In addition, from this
implementation specifically:

- capability metadata is declared, never observed. Health does not change
  because a provider started failing; nothing measures latency, cost or
  quality, and a provider that lies about being local is believed;
- there is exactly one slot in the runtime configuration, so routing currently
  chooses among one candidate in the demo path. The router is exercised with
  several candidates in unit tests;
- `context_size` is the utterance length in bytes, which is not tokens;
- TLS client certificates, SNI overrides, and proxy support are absent.
