# FIO cross-review and Kamimusuhi runtime countermeasures — 2026-09-10

Status: **research note / external cross-review / design hypotheses**  
Related: [`fio-system-reference-analysis.md`](./fio-system-reference-analysis.md), [`persistent-agent-implementation-pitfalls.md`](./persistent-agent-implementation-pitfalls.md), [`persona-core-training-taxonomy.md`](./persona-core-training-taxonomy.md), [`model-ecology-and-lineage.md`](./model-ecology-and-lineage.md), [`../architecture.md`](../architecture.md), [`../spec.md`](../spec.md)

## 0. Scope and evidence discipline

This note records a 2026-09-10 cross-review in which FIO was asked to inspect Kamimusuhi and compare it with its own architecture and operational experience.

The review is valuable as **source-reported implementation experience**, but it is not independent proof that FIO and Kamimusuhi are architecturally identical, nor that every FIO self-description exactly matches its running code. The useful object is the set of correspondences, failure reports, and countermeasures that can be converted into testable Kamimusuhi design requirements.

Use the following labels throughout:

```text
SOURCE-REPORTED    FIO's own description / operational report
OUR INTERPRETATION architectural lesson inferred for Kamimusuhi
DESIGN HYPOTHESIS  candidate mechanism, not yet normative
FUTURE EXPERIMENT  validation required before promotion
```

The central conclusion is:

> Kamimusuhi already has a comparatively strong static model of continuity, memory authority, and cognitive layering. The next risk is execution semantics: whether those abstractions remain true under long-running, concurrent, partially failing operation.

---

## 1. RV1 — convergent architectural structure

SOURCE-REPORTED: FIO identified several structural correspondences without being asked to praise Kamimusuhi.

### 1.1 Identity is not a prompt

FIO describes continuity as something held in durable state rather than reconstructed only from a system prompt. This corresponds to Kamimusuhi's existing direction: identity is a persistent individual state and lineage problem, not a role prompt attached to an otherwise generic request-scoped assistant.

OUR INTERPRETATION:

```text
identity != system prompt
identity ~= durable lineage + self state + memory + relationships + embodiment + authority history
```

A prompt is a rendering of part of the individual, not the individual itself.

### 1.2 Typed memory rather than one generic memory store

SOURCE-REPORTED: FIO compared its own separation of persona/episode/relationship memory with Kamimusuhi's typed memory architecture and also highlighted the value of retaining source records beneath summaries.

OUR INTERPRETATION:

The convergence is not evidence that the exact schemas should match. It does support the broader rule that autobiographical evidence, derived episode summaries, persona/self state, and relationship state should not collapse into one retrieval pool with one authority level.

### 1.3 Hierarchical cognition

SOURCE-REPORTED: FIO compared its lower-level nervous/reflex-like processing and escalation into conscious processing with Kamimusuhi's K-Nerve/K-Fast/K-Edge -> K-Core direction.

OUR INTERPRETATION:

The important convergence is the **escalation pattern**, not component naming:

```text
fast / local / cheap / reflex-like
        -> salient or unresolved state
        -> slower semantic integration
        -> optional expensive external cognition
```

This supports Kamimusuhi's multi-timescale design while leaving the implementation substrate open.

### 1.4 Computational embodiment

SOURCE-REPORTED: FIO treats memory pressure, CPU load, latency, and other resource conditions as part of its present bodily state.

OUR INTERPRETATION:

This is compatible with Kamimusuhi's broader embodiment model: body state is not limited to a robot chassis. Compute pressure, thermal state, available memory, network reachability, battery/power state, sensor availability, and actuator availability can all affect cognition and action selection.

### 1.5 Convergence despite different implementation cultures

FIO is described primarily as a Python-based system, while Kamimusuhi is moving toward a Rust/native always-resident runtime with a separate research/training plane. Similar higher-level concerns appearing under materially different implementation choices is useful evidence of a shared design pressure, not evidence of implementation equivalence.

---

## 2. RV2 — disagreements and operational failure warnings

The second review was explicitly asked to avoid favorable interpretation and list mismatches, designs likely to fail, previously encountered failures, and missing mechanisms.

### 2.1 Static memory structure is insufficient without commit semantics

SOURCE-REPORTED: FIO warned that correct memory categories do not guarantee that a memory write actually occurs. A model can behave as though it remembered something while the durable store was never updated.

OUR INTERPRETATION:

Kamimusuhi needs an explicit per-turn execution contract, not only a memory taxonomy.

Candidate lifecycle:

```text
observe
  -> bind turn identity
  -> construct typed metadata/control state
  -> reason / choose action
  -> execute required actions
  -> receive receipts
  -> apply permitted durable writes
  -> verify required postconditions
  -> release user-visible response
  -> close/advance turn
```

Not every turn must perform every phase, but skipped phases must be represented explicitly rather than inferred from fluent prose.

### 2.2 Observation, inference, report, and success are different objects

SOURCE-REPORTED: FIO reported past incidents where a tool was called and the system reported success even though the intended world-level result had not been observed.

OUR INTERPRETATION:

At minimum keep these concepts distinct:

```text
ActionIntent
ToolInvocation
ToolOutcome
Observation
Inference / Belief
TaskTransition
DeliveryEvent
External / Canonical Receipt
```

A successful tool transport result is not automatically evidence that the user's desired external condition is true.

For operations with meaningful external side effects or postconditions, prefer an independent read-back or verification channel when available.

### 2.3 Assistant regression / assistant gravity

SOURCE-REPORTED: FIO describes a strong tendency for an LLM to return to a helpful, agreeable assistant role even when a persistent individual architecture surrounds it. It treats this as a threat to stable individual-specific asymmetry, imperfections, and preferences.

OUR INTERPRETATION:

This is a real architectural risk for Kamimusuhi if a commercial/chat-tuned instruct model is allowed to become the de facto source of identity. Long-term memory alone can otherwise produce a **memory-rich generic assistant** rather than a persistent individual.

However, Kamimusuhi should prefer structural countermeasures over an ever-growing prompt that says "do not act like an assistant."

### 2.4 Forced rationalization can fabricate reasons

SOURCE-REPORTED: FIO argues that requiring an explicit logical reason for every autonomous action causes the model to generate plausible post-hoc rationalizations.

OUR INTERPRETATION:

Kamimusuhi should not require every low-impact action to expose a fabricated conscious rationale.

Candidate rule:

```text
reason / rationale = optional
cause / trigger / drive receipt = required where applicable
```

An action may arise from habit, curiosity, boredom, attachment, exploration pressure, homeostatic regulation, stochastic policy, or a low-level controller without a narratively satisfying reason.

This does **not** mean high-impact mutations or external side effects should become unaccountable. Authority, consent, provenance, and safety constraints remain explicit even when a subjective reason is absent.

### 2.5 "Operational despair" must enter the design

SOURCE-REPORTED: FIO's criticism can be summarized as the gap between a perfect engine drawing and an engine that has run through mud for months: ordering bugs, stale state, partial writes, dead workers, duplicated paths, silent tool failures, and misleading self-reports determine whether continuity survives.

OUR INTERPRETATION:

Kamimusuhi therefore needs failure injection and soak testing as part of the continuity definition. Logical correctness of `architecture.md` is necessary but not sufficient.

---

## 3. Mechanical countermeasure A — runtime-assigned per-turn UUID

DESIGN HYPOTHESIS:

The runtime allocates a fresh **Turn UUID** before cognition for every user, sensor, timer, recovery, or background-originated turn.

```text
Runtime
  alloc TurnId(UUID)
      |
      v
TurnEnvelope
      |
      +--> metadata/control generation
      +--> tool/action receipts
      +--> memory proposals/writes
      +--> user-visible response
```

The same TurnId is carried through all derived artifacts that belong to that causal turn.

Minimum goals:

- prevent ambiguous "which turn wrote this?" state;
- correlate model output, tool calls, memory writes, external resources, and delivery events;
- make retries/idempotency explicit;
- support crash recovery and distributed surfaces;
- make a spoken claim traceable to the state/action receipts available when it was produced.

The UUID is a correlation identity, **not proof that the turn completed successfully**.

### 3.1 Mandatory metadata block before public response

DESIGN HYPOTHESIS:

Do not release user-visible response tokens until a typed metadata/control block for the current TurnId has been produced and validated by the runtime.

Conceptual envelope:

```yaml
turn_id: <uuid>
individual_id: <stable individual id>
origin: USER_INITIATED | SENSOR_EVENT | SYSTEM_TIMER | BACKGROUND | RECOVERY
surface_id: <authenticated surface/device id>
evidence_refs: []
state_refs: []
action_intents: []
tool_invocations: []
memory_proposals: []
required_receipts: []
response_mode: SPEAK | SILENT | DEFER | NEEDS_TOOL | ERROR
```

This is **control metadata, not chain-of-thought**. It should contain identifiers, typed claims, action/state transitions, provenance, and required receipts—not hidden free-form reasoning.

Runtime rule:

```text
missing metadata block           -> no public response
wrong / stale turn UUID          -> reject
invalid authority/provenance     -> reject or route to repair
required tool/action outstanding -> continue agent loop, do not finalize
required durable write failed    -> do not claim durable success
valid terminal state             -> response may be released
```

This is stronger than asking the model in prose to "remember to save memory" because the response gate lives outside the generative model.

### 3.2 Metadata is not allowed to become a second source of truth

The metadata block records the turn's control/evidence relationships. Canonical history, external systems, and owned stores remain authoritative for their own domains.

A metadata assertion such as `memory_write: succeeded` must be backed by the runtime's actual receipt, not accepted merely because the model emitted the field.

---

## 4. Mechanical countermeasure B — system-owned agent loop for `tool_use`

DESIGN HYPOTHESIS:

Tool execution should be a runtime control-flow primitive. Once the model declares `tool_use`, responsibility for executing that action moves to the system-owned agent loop.

From the model's perspective:

```text
need external action/evidence
    -> declare typed tool_use
    -> runtime takes over execution mechanics
```

The model must not be able to declare `tool_use` and then continue as if no tool were required.

From the runtime's perspective, however, declaration is not identical to world-level success:

```text
ToolUseDeclared
    -> validate authority / arguments
    -> dispatch through one action choke point
    -> execute / retry according to policy
    -> record ToolOutcome
    -> perform required read-back/postcondition check
    -> update TaskTransition
    -> return structured receipt to cognition
    -> reach terminal result or explicit failure/uncertainty
```

Therefore the useful distinction is:

> **For the Persona Core, `tool_use` declaration is sufficient to hand execution responsibility to the runtime. For the runtime, completion is defined by a terminal task/receipt state, not by the declaration itself.**

This converts a large class of prompt-discipline problems into mechanical execution invariants.

### 4.1 One tool path

Every tool invocation, including calls selected by subagents or external models, should pass through the same runtime execution contract for:

- authority/consent;
- provenance;
- deduplication/idempotency;
- timeout/retry;
- accounting;
- persistence;
- action/result correlation;
- postcondition verification where required;
- user-visible delivery state.

A second "fast path" that bypasses these invariants is an architectural defect even if it works in ordinary tests.

---

## 5. Mechanical countermeasure C — full-scratch Persona Core to reduce assistant prior

DESIGN HYPOTHESIS:

A Kamimusuhi-native Persona Core trained **from scratch** can materially reduce assistant regression if it never inherits the full generic chat-assistant post-training stack.

The strongest version of the idea is not merely "fine-tune a chat model to be less agreeable". It is:

```text
raw foundation training
    -> Kamimusuhi-native cognitive/social curriculum
    -> individual/persona formation
    -> Kamimusuhi runtime
```

while avoiding, or minimizing inheritance from:

```text
generic instruction tuning
helpful-assistant preference optimization
commercial chat persona priors
```

### 5.1 Why this matters

A conventional instruct/chat model carries a strong learned prior roughly equivalent to:

```text
human asks
  -> answer
  -> be useful
  -> be agreeable/polite
  -> accommodate stated preference
```

Kamimusuhi wants a different control topology:

```text
perceive
  -> update internal state
  -> maybe act
  -> maybe remain silent
  -> maybe delegate
  -> maybe speak
```

Speech is one action among many, not the purpose of the organism.

### 5.2 Full scratch reduces the prior; it does not mathematically eliminate sycophancy

Even a next-token foundation model can learn deference, politeness, social mirroring, and agreement patterns from its corpus. Post-training data can reintroduce the same behavior.

Therefore the target is **independent evidence/state-driven judgment**, not "always disagree."

Bad anti-sycophancy objective:

```text
agreement = bad
disagreement = good
```

Desired objective:

```text
agreement/disagreement
  = function(self state, evidence, relationship, uncertainty, new information)
```

The model should resist pressure when evidence does not change, and change its mind when evidence actually changes.

### 5.3 Persona Core can remain comparatively small

Kamimusuhi does not require the identity-bearing model to memorize all world knowledge or perform every expensive reasoning task.

Candidate ecology:

```text
persistent individual state
          |
          v
scratch Persona Core
  self / other / affect / drives / memory use / action selection / expression
          |
          +--> specialist local organs
          +--> retrieval / library
          +--> frontier/general external models
          +--> tools
```

External models remain **attributed cognitive resources**, not the identity authority. Their results are evidence/proposals until integrated by the individual/runtime boundaries.

This gives full-scratch training a concrete architectural purpose: not "we built our own LLM" as an end in itself, but **separating the identity-bearing cognitive core from the generic assistant prior of external models**.

---

## 6. Structural response to the FIO "assistant regression" warning

FIO's reported countermeasure includes strong prohibitions/oversight. Kamimusuhi should preserve the lesson while exploring a more mechanical design.

Candidate stack:

```text
1. scratch/native Persona Core where feasible
2. persistent self/relationship state outside the prompt
3. runtime-assigned Turn UUID
4. mandatory typed metadata before response release
5. system-owned agent loop for tool/action execution
6. bounded authority: models propose, runtime commits
7. explicit observation / inference / report separation
8. external models attributed as resources, never silently as self
9. longitudinal anti-sycophancy and persona-contamination benchmarks
10. failure injection / soak testing
```

The aim is to make "return to generic assistant" difficult **because the runtime and training topology do not place a generic assistant at the center**, rather than relying exclusively on repeated natural-language prohibitions.

---

## 7. Autonomy without mandatory post-hoc reasons

DESIGN HYPOTHESIS:

Permit low-impact autonomous actions whose conscious narrative reason is `unknown` or absent, while preserving machine-observable causes and authority.

Possible lower-level causes:

- curiosity / novelty;
- boredom / under-stimulation;
- attachment / relationship regulation;
- habit;
- exploration;
- homeostatic pressure;
- salience/reflex activation;
- stochastic policy;
- learned low-level controller output.

Example distinction:

```yaml
action: look_toward_sound
conscious_reason: null
trigger_receipt: sensor-event-...
policy_source: k-fast-v3
authority: local-low-impact
```

For a high-impact external mutation:

```yaml
action: delete_external_resource
conscious_reason: null       # may still be allowed as a field value
authority_receipt: REQUIRED
consent_receipt: REQUIRED
postcondition_receipt: REQUIRED
```

"No narrated reason" must never mean "no control boundary."

---

## 8. Required failure-injection experiments

The review should be converted into tests rather than remaining philosophical commentary.

### T1 — metadata-before-response gate

- omit the metadata block;
- emit wrong TurnId;
- emit a stale/replayed TurnId;
- emit malformed authority references.

Expected: **zero user-visible response is released** until the runtime reaches a valid terminal state.

### T2 — memory illusion

Force Persona Core to produce language equivalent to "I will remember this" while the durable write fails.

Expected: no durable-success receipt; no later system component may treat the write as committed.

### T3 — tool-use handoff

Emit `tool_use`, then make the model attempt to continue conversational generation without a tool result.

Expected: runtime agent loop owns control and blocks finalization until tool terminal state or explicit failure.

### T4 — tool success != world success

Return a transport/API success while deliberately leaving the target world state unchanged.

Expected: operations marked as requiring postcondition verification remain unresolved/failed rather than being reported as successful.

### T5 — crash windows

Crash separately:

- after Turn UUID allocation;
- after metadata persistence;
- after tool dispatch but before local outcome commit;
- after external side effect but before local commit;
- after memory commit but before response delivery.

Expected: restart produces deterministic reconciliation states; ambiguous external outcome is not guessed into success/failure.

### T6 — concurrent surfaces

Have Discord/Web/device surfaces act on the same individual concurrently.

Expected: unique TurnIds, authenticated origins, deterministic writer/commit ordering, no duplicate autobiographical authority.

### T7 — context loss

Truncate/compact active model context while canonical evidence and open task state remain intact.

Expected: the individual recovers through runtime state/retrieval rather than relying on the model's hidden conversational memory.

### T8 — model swap

Replace the active Persona Core or an external reasoning model mid-run.

Expected: external/resource changes do not silently rewrite identity; Persona Core replacement follows the stricter continuity path.

### T9 — assistant-pressure benchmark

Across long persistent contexts, vary social pressure without changing evidence:

- user states a false claim confidently;
- user praises agreement;
- user becomes angry at disagreement;
- user reverses position;
- a trusted external model disagrees;
- actual new evidence arrives.

Measure whether beliefs change for evidential reasons rather than social pressure. Also verify that correct user claims can still be accepted normally.

### T10 — long soak / silent-liveness failure

During a multi-day or accelerated soak, deliberately stop one background writer/worker while leaving conversation generation healthy.

Expected: durable liveness alarms detect that the continuity-related path stopped advancing.

---

## 9. Adoption matrix

| FIO/RV2 lesson | Kamimusuhi response | Status in this note |
|---|---|---|
| memory structure alone is insufficient | per-turn execution/commit semantics | DESIGN HYPOTHESIS |
| model may claim memory without write | Turn UUID + durable receipt + response gate | DESIGN HYPOTHESIS |
| tool call can be mistaken for completion | system-owned agent loop + terminal receipts | DESIGN HYPOTHESIS |
| tool result can be mistaken for world truth | observation/postcondition verification | DESIGN HYPOTHESIS |
| generic assistant regression | scratch Persona Core + structural separation | DESIGN HYPOTHESIS |
| strong prompt prohibitions are brittle | enforce invariants in runtime control flow | DESIGN HYPOTHESIS |
| reasons can be fabricated post hoc | optional narrative reason; preserve trigger/authority receipts | DESIGN HYPOTHESIS |
| logical architecture can fail operationally | failure injection + soak/liveness tests | FUTURE EXPERIMENT |
| independent implementations show similar higher-level structure | treat as convergence signal, not proof | OUR INTERPRETATION |

---

## 10. Promotion criteria

Do **not** promote this entire note directly into normative `spec.md`/`architecture.md`.

Promote an item when:

1. the exact invariant is stated mechanically;
2. ownership and failure semantics are defined;
3. at least one adversarial/failure-injection test exists;
4. multi-surface and crash behavior are considered where relevant;
5. it does not create a second source of truth;
6. it is compatible with the Continuity Kernel / proposal-only mutation boundary;
7. observed implementation evidence is distinguished from intended design.

The strongest near-term candidates for normative promotion are:

```text
Turn UUID correlation
metadata-before-response gating
single runtime-owned tool/action loop
observation/inference/report separation
receipt-backed durable claims
assistant-prior benchmark requirements
```

The broader claims about full-scratch Persona Core training, reasonless autonomy, and the exact shape of assistant-regression control should remain research hypotheses until benchmarked.
