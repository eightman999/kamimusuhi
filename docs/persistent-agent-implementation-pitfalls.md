# Persistent-agent implementation pitfalls

Status: **research note / failure-mode catalogue**

Reviewed: **2026-09-08**

Primary external implementation examined here: [`sakuranaga/yui-agent`](https://github.com/sakuranaga/yui-agent), especially its memory, tool-gating, worker, crash-recovery, user-profile, and affinity design notes.

Related Kamimusuhi note: [`fio-system-reference-analysis.md`](./fio-system-reference-analysis.md).

This document exists to keep **implementation stumbling points** from disappearing into chat history. It is deliberately broader than a feature list: the useful question is not only *what a mature personal agent has*, but *which seemingly reasonable designs broke once the pieces were integrated*.

The public `yui-agent` repository is treated as an external reference implementation. It is not established here that it is the exact "結衣システム" referred to in every external conversation. Claims below therefore use the repository and its own docs as the source, not that identity assumption.

Use the project evidence vocabulary:

```text
SOURCE-REPORTED    described by the external implementation/docs
OUR INTERPRETATION architectural lesson inferred for Kamimusuhi
DESIGN HYPOTHESIS  candidate design, not yet normative
FUTURE EXPERIMENT  validation required before adoption
```

## 1. Executive summary

The recurring pattern across Yui and FIO is that long-running agents fail at **boundaries**, not only at model intelligence:

- raw evidence vs derived memory;
- current context vs long-term recall;
- assistant utterance vs trusted instruction;
- tool-needed turn vs free-form language generation;
- requested action vs confirmed action vs executed action vs delivered result;
- web/request process vs background work;
- local state vs external source of truth;
- "executing" vs "succeeded" after an ambiguous crash;
- persona constants vs dynamic relationship/regulatory state;
- per-fact recall vs an integrated model of a person;
- scheduled autonomy vs user-interaction latency;
- implementation existence vs live-system wiring.

The strongest project-wide rule is therefore:

> **Every boundary that can lose authority, provenance, ordering, or liveness should be represented explicitly in state rather than inferred from prose or control flow.**

For Kamimusuhi this complements the existing Continuity Kernel / proposal-only mutation model: continuity authority remains narrow, while the surrounding cognition becomes more explicit about evidence, tasks, tools, background work, and uncertainty.

---

## 2. Memory and context pitfalls

### P01 — Treating "memory" as one database

SOURCE-REPORTED: Yui separates raw messages, extracted memory chunks, session summaries, pending task results, user-profile snapshots, and diary/self-view data rather than storing all remembered material with one semantic status.

Reference:
- <https://github.com/sakuranaga/yui-agent/blob/main/docs/memory-architecture.md>
- <https://github.com/sakuranaga/yui-agent/blob/main/docs/user-profile-snapshot.md>

**Failure mode:** a single vector store eventually mixes evidence, inference, preference, subjective interpretation, promises, task outputs, and temporary state. Retrieval then cannot tell what kind of authority a hit has.

**Kamimusuhi rule:** keep at least these roles distinct:

```text
Canonical evidence / raw episode record
Derived episodic memory
Semantic/fact memory
Relationship/person model
Self-generated diary/reflection
Current working context
Task/commitment state
External-library knowledge
```

A similarity score must never silently substitute for an authority type.

### P02 — Relying on session-end extraction

SOURCE-REPORTED: Yui notes that clean session-end triggers are unreliable in real use. It added rolling extraction after a threshold of unprocessed messages and a stale-session path for short abandoned conversations.

**Failure mode:** the user simply closes the client, loses connectivity, reloads, or changes surfaces. Important context never crosses the "session ended" boundary and therefore never becomes retrievable memory.

**Kamimusuhi rule:** memory formation needs multiple triggers:

```text
explicit remember request
rolling checkpoint
idle/stale episode checkpoint
pre-compaction checkpoint
surface/process shutdown checkpoint where available
post-hoc recovery scan
```

None of these should be the sole path.

### P03 — Rolling extraction without idempotent progress

SOURCE-REPORTED: Yui keeps per-session extraction progress so rolling and later session-end passes do not blindly process the same range again.

**Failure mode:** background retries, process restarts, and overlapping extraction triggers create duplicate memories or contradictory derived summaries.

**Kamimusuhi rule:** every derivation pass should have a stable source range / evidence set and an idempotency identity. At minimum record:

```text
source_episode_id
source_event_from
source_event_to
extractor/model/version
extraction_kind
created_at
supersedes / invalidates
```

### P04 — Treating provisional memory as fully authoritative

SOURCE-REPORTED: Yui caps the importance of rolling-extracted memories and marks them `provisional`, allowing later session-level reconciliation to supersede them.

**Failure mode:** a mid-conversation statement is captured before the user corrects, jokes about, retracts, or contextualizes it.

**Kamimusuhi rule:** early capture should be recoverable without pretending to be final interpretation. Candidate states include:

```text
PROVISIONAL
CONFIRMED_BY_EPISODE
CORRECTED
SUPERSEDED
INVALIDATED
```

For identity-bearing state, the existing `MemoryProposal / SelfProposal -> Mutation Policy` boundary remains stronger and should be preserved.

### P05 — Excluding the current session from retrieval by default

SOURCE-REPORTED: Yui initially excluded current-session memory chunks, then reversed the choice after reloads caused the client-side recent-message buffer to disappear while the session id remained. Useful rolling-extracted context became invisible.

**Failure mode:** scope assumptions are based on UI lifecycle rather than evidence lifecycle. A reload turns "already in working context" into "not available anywhere".

**Kamimusuhi rule:** retrieval scope must depend on what is *actually present in the active workspace*, not merely whether an item belongs to the same nominal episode.

### P06 — Vector similarity alone is not enough

SOURCE-REPORTED: Yui combines semantic retrieval with lexical/BM25-like matching and MMR-style diversity; query construction includes recent user turns because pronouns and short references often fail as standalone embedding queries.

**Failure mode:** proper nouns, model numbers, literal phrases, identifiers, Japanese orthographic variation, and "that thing from earlier" are weak or ambiguous under pure vector retrieval.

**Kamimusuhi rule:** benchmark hybrid retrieval. Candidate stack:

```text
semantic candidates
+ lexical/exact candidates
+ temporal/episode constraints
+ authority/source filters
+ diversity/dedup
+ evidence-aware reranking
```

Do not hard-code Yui's exact weights as architecture.

### P07 — A pile of correct facts is not a person model

SOURCE-REPORTED: Yui's user-profile design explicitly says per-chunk retrieval can be individually correct while still producing a patchwork understanding of the user. It therefore proposes a periodic synthesis snapshot with evidence notes.

**Failure mode:** the agent remembers "likes X", "works on Y", and "said Z" but cannot form a stable higher-level model of communication style, current focus, relationship expectations, or long-term trajectory.

**Kamimusuhi rule:** distinguish:

```text
atomic memories              evidence-linked claims
person/relationship model    derived synthesis
shared-history narrative     temporal interpretation
```

The synthesis is **derived**, not canonical truth. It should expose evidence and be correctable/rebuildable.

### P08 — Mixing the agent's subjective diary with an objective user model

SOURCE-REPORTED: Yui deliberately separates a first-person diary from a data-driven user profile.

**Failure mode:** "I felt that the user seemed distant" becomes indistinguishable from "the user has been distant," and later retrieval launders subjective interpretation into factual belief.

**Kamimusuhi rule:** self-generated reflection, social inference, and observed evidence need typed provenance. This reinforces the FIO lesson:

```text
Thought != Observation
Inference != Fact
Diary != Person Model
Person Model != Canonical Evidence
```

### P09 — Time decay can erase stable important facts

SOURCE-REPORTED: Yui applies recency decay in retrieval but separately keeps high-importance always-on facts.

**Failure mode:** a long-standing allergy, permanent constraint, identity fact, or hard preference becomes hard to retrieve merely because it is old.

**Kamimusuhi rule:** do not make one decay function responsible for all memory. Stable constraints, autobiographical anchors, active commitments, and ordinary episodic recall need different retention/retrieval policy.

### P10 — Always-on facts can become an immortal stale prompt

The inverse failure is equally important.

**Failure mode:** promoting facts into a permanent always-on tier fixes decay, but now outdated beliefs occupy every prompt and resist correction.

**Kamimusuhi rule:** every always-on derived fact needs provenance, validity state, and a replacement/invalidation path. "Always injected" must not mean "cannot be revised."

### P11 — Deleting contradictory memories destroys auditability

SOURCE-REPORTED: Yui uses soft invalidation / supersession rather than hard deletion for reconciled memories.

**Failure mode:** the system loses the ability to answer "why did you believe that?" or reconstruct when a belief changed.

**Kamimusuhi rule:** prefer append + invalidate/supersede for historical cognitive records. The Continuity Kernel remains responsible for the narrower canonical identity transition path.

### P12 — Automated contradiction reconciliation can erase the correct memory

SOURCE-REPORTED: Yui's reconciliation policy is conservative: when uncertain, prefer "consistent" rather than invalidating an existing memory.

**Failure mode:** an LLM-based deduper/conflict resolver confidently decides that two compatible facts conflict and destroys useful history.

**Kamimusuhi rule:** contradiction resolution should be evidence-preserving, soft, confidence-aware, and reversible. High-authority state changes should never depend on a single unreviewed semantic classifier.

### P13 — Embedding/index versions are hidden state

SOURCE-REPORTED: Yui anticipates embedding-model replacement and re-embedding needs.

**Failure mode:** retrieval quality changes after a model upgrade but no record says which embeddings or summaries were generated by which model/version.

**Kamimusuhi rule:** derived artefacts should carry generator provenance. Indexes must be rebuildable, and migrations should be measurable rather than silently mixing incompatible representations.

### P14 — Context compaction is a data-loss boundary

From the FIO/Yui comparison and current Kamimusuhi discussion:

**Failure mode:** an LLM/provider silently compresses or truncates active context. The agent continues speaking fluently, so the loss is only discovered much later when a detail matters.

**Kamimusuhi design hypothesis:** treat compaction pressure as a checkpoint boundary:

```text
working context
      │
      ├─ continuous raw event append
      ├─ rolling derivation
      └─ pre-compaction checkpoint
              │
              ▼
        durable evidence store
```

Prefer proactive token-budget thresholds in addition to any provider-specific compaction event. Provider internals are not an acceptable single point of continuity.

### P15 — Restoring the whole archive recreates the context problem

**Failure mode:** after correctly preserving lossless history, the implementation reinjects too much of it, causing latency, cost, cache breakage, and another truncation cycle.

**Kamimusuhi rule:** the archive is evidence, not the default prompt. Use progressive retrieval:

```text
current episode
+ compact state
+ relevant derived memory
+ targeted raw evidence only when needed
```

---

## 3. Task, promise, and asynchronous-result pitfalls

### P16 — A promise is not just another memory chunk

SOURCE-REPORTED: Yui has a distinct `commitment` memory type.

**Failure mode:** "I'll check that later" is stored like an anecdote, so the system remembers that it promised something but has no execution obligation.

**Kamimusuhi rule:** represent commitments as state with owner, due condition/time, status, source episode, and completion evidence. A narrative memory may be derived from that state, not vice versa.

### P17 — Task completion is not the same as user notification

SOURCE-REPORTED: Yui keeps `pending_acknowledgement` for completed internal tasks so a result remains visible until it has actually been surfaced.

**Failure mode:** a background agent successfully finishes research while the user is away; the system marks the job done and then never mentions it.

**Kamimusuhi rule:** distinguish:

```text
PENDING
RUNNING
SUCCEEDED
FAILED
RESULT_AVAILABLE
DELIVERED / ACKNOWLEDGED
```

Delivery is a separate state transition with its own idempotency.

### P18 — Pending results can repeat forever without acknowledgement semantics

The opposite bug appears once L4.5-style pending-result injection exists.

**Failure mode:** a completed task is injected every turn and the agent keeps announcing it.

**Kamimusuhi rule:** acknowledgement/delivery state must be atomic and attributable. If multiple surfaces exist, decide whether delivery on one surface counts globally or per surface.

### P19 — User TODOs and agent-internal orchestration tasks are different domains

SOURCE-REPORTED: Yui keeps internal orchestration tasks separate from an external user-facing task manager which remains the source of truth.

**Failure mode:** invisible worker bookkeeping pollutes the user's actual TODO system, or the agent's local database silently becomes a competing canonical task store.

**Kamimusuhi rule:** distinguish **world/domain state** from **cognitive execution state**. The latter may reference the former but should not silently replace its source of truth.

---

## 4. Tool-use and epistemic pitfalls

### P20 — Speaking before a required tool result

SOURCE-REPORTED: Yui's earlier flow let the main model answer while tool execution happened in parallel. This produced claims such as schedule availability before calendar retrieval and contradictions between the first answer and the later tool-backed result.

Reference:
- <https://github.com/sakuranaga/yui-agent/blob/main/docs/chat-tool-gating-v5.md>

**Failure mode:** speculative natural language wins the race against evidence.

**Kamimusuhi rule:** classify whether the turn requires grounded data/action before allowing factual free-form completion. Latency optimizations must not let an ungrounded answer escape first.

### P21 — Letting the same model both talk and decide tool necessity blurs responsibilities

SOURCE-REPORTED: Yui moved toward a small Tool Gate whose job is only `no_tool` vs `tool_required`, leaving exact tool choice to the Executor.

**Failure mode:** one prompt simultaneously optimizes persona, conversation, factual answer, tool need, tool choice, and arguments. Errors are hard to attribute and earlier prose can commit the system before execution.

**Kamimusuhi rule:** keep a cheap epistemic/action gate distinct from detailed execution planning where practical. Do not create duplicate competing selectors.

### P22 — The latest user message often lacks the tool target

SOURCE-REPORTED: in an external-verification failure, the user asked roughly "did you actually check that on the web?" The concrete product names existed only in the assistant's previous claim, so a user-only tool context could not resolve the reference.

Reference:
- <https://github.com/sakuranaga/yui-agent/blob/main/docs/external-verification-context-fix-plan.md>

**Failure mode:** retrieval/tool selection receives "それ / that / the thing you said" and has no target.

**Kamimusuhi rule:** context construction must be task-specific. Read/search verification may need recent assistant claims; mutation authority generally should not.

### P23 — Passing assistant speech into the executor can turn hallucination into an instruction

This is the safety mirror of P22.

SOURCE-REPORTED: Yui's design proposes passing previous assistant text as **reference claims to verify**, explicitly not as operation authority.

**Failure mode:** the assistant previously says "we should delete X," the user says "that one," and the mutation executor treats the assistant's own suggestion as user authorization.

**Kamimusuhi rule:** typed context:

```text
USER_INSTRUCTION
ASSISTANT_CLAIM_TO_VERIFY
TOOL_RESULT
EXTERNAL_UNTRUSTED_TEXT
CANONICAL_STATE
MEMORY_INFERENCE
```

Authority must not be inferred from message adjacency.

### P24 — `tool_required` followed by `declined` must not fall back to speculation

SOURCE-REPORTED: Yui documented a case where the gate correctly required external verification, the executor selected no tool, and the main model returned to ordinary conversation instead of searching.

**Failure mode:** the architecture recognizes that evidence is required and then quietly bypasses its own decision.

**Kamimusuhi rule:** impossible/failed tool execution needs a structured terminal state. For explicit external verification:

```text
verified result
explicit tool failure/unavailability
clarification required
```

must replace "continue as if no tool was needed."

### P25 — Prompt warnings are not a substitute for control-flow invariants

SOURCE-REPORTED: Yui notes that "do not assert current product facts without searching" can be a supporting prompt rule, but the primary fix belongs in gate/executor/retrieval/fallback behavior.

**Kamimusuhi rule:** when correctness can be enforced structurally, do not rely on a prose instruction to a generative model.

### P26 — Tool outcome, task state, and user-visible report are different artefacts

**Failure mode:** a tool call returns successfully but the surrounding task never records it, or a task completes but no report is delivered, or a report is generated from stale intermediate state.

**Kamimusuhi rule:** preserve explicit receipts:

```text
ToolInvocation
ToolOutcome
TaskTransition
DeliveryEvent
Canonical/External Receipt where applicable
```

Each can fail independently.

---

## 5. External side effects, confirmation, and crash recovery

### P27 — Confirmation is not execution

SOURCE-REPORTED: Yui's crash-recovery note identifies multiple windows between confirmation, handler execution, reservation finalization, task finalization, and user-visible final report.

Reference:
- <https://github.com/sakuranaga/yui-agent/blob/main/docs/tool-crash-recovery-reconciliation.md>

**Failure mode:** a system stores "confirmed" and later assumes that means "the external side effect happened."

**Kamimusuhi rule:** confirmation grants authority to attempt an operation; it is not evidence of operation success.

### P28 — "Handler returned" is not durable success

**Failure mode:** an external calendar/event API accepts a mutation and the process crashes before local success state is committed. On restart, local state says uncertain while the world may already have changed.

**Kamimusuhi rule:** introduce an explicit **UNKNOWN_EXTERNAL_OUTCOME** / reconciliation state. Do not force ambiguity into succeeded/failed.

### P29 — Blind replay of uncertain mutations can be worse than doing nothing

SOURCE-REPORTED: Yui intentionally avoids automatic replay of ambiguous external mutations when the API cannot prove idempotency, because replay can duplicate creation or repeat destructive actions.

**Kamimusuhi rule:** automatic replay requires one of:

- stable idempotency key accepted/recorded externally;
- externally queryable operation id / resource id;
- operation semantics that are demonstrably idempotent;
- a safe reconciliation procedure.

Otherwise detect and surface uncertainty rather than guessing.

### P30 — Dedup and idempotency are not synonyms

**Failure mode:** local dedup suppresses a second call but cannot prove whether the first side effect happened. Conversely, an API idempotency key can make replays safe even after local state loss.

**Kamimusuhi rule:** model separately:

```text
request deduplication
execution reservation
external idempotency
post-crash reconciliation
```

### P31 — Automatic "fixers" can make ambiguous external state worse

SOURCE-REPORTED: Yui's reconciliation tool initially restricts `--fix` to DB-local safe repairs and keeps ambiguous external executions read-only.

**Kamimusuhi rule:** repair automation needs a safety classification. Read-only detection and human/agent-assisted reconciliation are valid states; not every inconsistency should self-heal.

---

## 6. Background work, scheduling, and process-boundary pitfalls

### P32 — Starting background maintenance from the interactive hot path

SOURCE-REPORTED: Yui observed that after reload/idle, the first `/api/chat` call could initialize the scheduler and trigger overdue news, mail, memory, cleanup, and other work at once. The result was a slow first chat and unrelated proactive notifications mixed into the user's interaction.

Reference:
- <https://github.com/sakuranaga/yui-agent/blob/main/docs/dev-memory-restart-investigation.md>
- <https://github.com/sakuranaga/yui-agent/blob/main/docs/background-worker-refactor-plan.md>

**Failure mode:** "lazy startup" saves idle work but turns the user's next interaction into a thundering-herd trigger.

**Kamimusuhi rule:** user-facing latency and autonomous maintenance must have separate scheduling/resource budgets. Catch-up policy should be explicit per job, not "run everything overdue now."

### P33 — Periodic catch-up policy cannot be one-size-fits-all

**Failure mode:** after downtime, a reminder may need urgent catch-up while old news polling or profile synthesis can be delayed/coalesced/dropped.

**Kamimusuhi rule:** every scheduled processor declares semantics such as:

```text
MUST_CATCH_UP
LATEST_ONLY
COALESCE
DROP_IF_STALE
DEFER_WHILE_INTERACTIVE
```

This should integrate with the freshness contracts already suggested by the FIO analysis.

### P34 — Fire-and-forget after a DB write creates a loss window

SOURCE-REPORTED: Yui's worker redesign explicitly rejects patterns like "save message; then `void enqueue()`" and moves toward transactional enqueue.

**Failure mode:** the process dies after the state write but before the background job is queued, or queues a job whose prerequisite write later fails.

**Kamimusuhi rule:** cause state + job/outbox append should be atomic when they belong to one transition.

### P35 — In-memory notification delivery breaks after worker separation

SOURCE-REPORTED: once Yui separates the worker from the web process, an in-memory SSE push is no longer sufficient; it proposes a durable outbox plus a low-latency wake-up channel.

**Kamimusuhi rule:** durable user-visible events must not depend on the lifetime of one process or connection. Pub/sub may accelerate delivery but should not be the only record.

### P36 — Multiple scheduler owners cause duplicate autonomous actions

SOURCE-REPORTED: Yui's worker plan fixes scheduler ownership to the worker and proposes a DB advisory lock.

**Failure mode:** two web/worker instances both run the same periodic action, creating duplicate notifications, duplicate mail polls, or worse, duplicate mutations.

**Kamimusuhi rule:** autonomous jobs require explicit lease/ownership semantics across K-Core failover and multi-node deployment.

### P37 — Background jobs need a common execution substrate

SOURCE-REPORTED: Yui's refactor moves away from ad-hoc `setInterval`, `void run()`, and feature-specific fire-and-forget into a registry with timeout, retry, dedup key, concurrency, and persisted status.

**Failure mode:** every feature invents different retry/error semantics; eventually no one knows which jobs are durable, idempotent, or observable.

**Kamimusuhi rule:** background cognition can remain heterogeneous, but execution mechanics should expose a common contract for liveness, cancellation, freshness, retries, and provenance.

### P38 — Background errors that are merely logged still disappear operationally

**Failure mode:** a periodic memory reconciliation throws, catches/logs the exception, and the agent keeps functioning for weeks while memory quality decays.

**Kamimusuhi rule:** important background pipelines need durable failed state, retry policy, health metrics, and "last successful advance" alarms. This is a direct extension of FIO's silent-defect lesson.

### P39 — Development-server behavior can be mistaken for system architecture failure

SOURCE-REPORTED: Yui separately documented Next.js development-mode memory/reload/cold-route effects and distinguished them from application-level scheduling problems.

**Failure mode:** tuning the cognitive architecture around HMR, dev watchers, cold compile, or container-specific artifacts.

**Kamimusuhi rule:** maintain production-like soak runs. Performance conclusions need environment labels.

---

## 7. Persona, relationship, and dynamic-state pitfalls

### P40 — Dynamic relationship state inside the stable persona prompt

SOURCE-REPORTED: Yui's affinity design keeps persona text stable and places changing affinity/mood hints in a dynamic environment block, partly to avoid prompt-cache breakage.

Reference:
- <https://github.com/sakuranaga/yui-agent/blob/main/docs/affinity-system.md>

**Failure mode:** every mood/relationship update rewrites the persona prefix, increasing cost and also making "who the agent is" indistinguishable from "how the agent feels right now."

**Kamimusuhi rule:** separate:

```text
persona dispositions          slow/stable
relationship model            long-lived but revisable
regulatory/affective state    dynamic
utterance policy              momentary
```

This matches `Persistent Self State.current_regulatory_state` in `architecture.md`.

### P41 — Maximal intimacy all the time becomes persona collapse

SOURCE-REPORTED: Yui explicitly calls out constant high-affinity behavior as an anti-pattern because it becomes monotonous/noisy and harms the professional persona.

**Kamimusuhi rule:** relationship expression should be stateful, contextual, and sparse where appropriate. More "affection signal" is not monotonically better agent quality.

### P42 — Relationship mechanics can become manipulative engagement mechanics

SOURCE-REPORTED: Yui rejects guilt-based absence notifications such as making the agent appear distressed because the user has not returned.

**Kamimusuhi rule:** autonomy and attachment mechanisms should optimize genuine relationship coherence and user goals, not retention pressure through guilt or dependency cues.

### P43 — A numeric affinity score is a control heuristic, not the relationship itself

**Failure mode:** the implementation starts treating `affinity=83` as canonical relationship truth and generates behavior to satisfy the metric.

**Kamimusuhi rule:** any scalar affinity/trust score should be a derived control feature with evidence and bounded authority. Shared history and explicit relationship state remain richer than one number.

---

## 8. Multi-surface and source-of-truth pitfalls

### P44 — Multiple interfaces can create multiple selves unless source attribution is explicit

SOURCE-REPORTED: Yui records the interaction source (`web`, Discord, cron, etc.) while routing interfaces into the same agent core.

**Kamimusuhi rule:** every observation/utterance/action event needs authenticated surface provenance. K-Edge devices must not independently become competing autobiographical authorities.

### P45 — A cron/proactive turn is not an ordinary user turn

SOURCE-REPORTED: Yui describes cron as the unique path with no user input and excludes several internal sources from normal user Tool Gate handling.

**Failure mode:** autonomous cognition inherits assumptions that only hold when a user just supplied intent/authority.

**Kamimusuhi rule:** origin must be explicit:

```text
USER_INITIATED
SYSTEM_TIMER
SENSOR_EVENT
BACKGROUND_REFLECTION
EXTERNAL_WEBHOOK
RECOVERY
```

Tool/mutation authority varies by origin even if all routes eventually share a common cognitive core.

### P46 — Mirroring an external system can accidentally create a second source of truth

SOURCE-REPORTED: Yui's memory design says external systems such as calendar/task systems should remain authoritative, with local context overlays rather than full competing canonical copies.

**Kamimusuhi rule:** distinguish cached evidence from owned state. If Kamimusuhi is not the authority for an external object's truth, local memory must preserve the external source id/version and tolerate staleness.

### P47 — Single-user simplifications do not transfer automatically

SOURCE-REPORTED: Yui intentionally assumes one personal user and avoids multi-user columns/authorization complexity.

**Failure mode:** copying convenient invariants into Kamimusuhi's distributed K-Edge/K-Core design, where multiple devices, surfaces, credentials, and potentially multiple human relationships exist.

**Kamimusuhi rule:** adopt personal-agent UX lessons, not implicit single-process/single-surface authority assumptions.

---

## 9. Implementation/status pitfalls

### P48 — "Implemented" is too coarse

Both FIO and Yui documentation expose designs that are partly implemented, planned, refactored, or measured under specific conditions.

**Kamimusuhi rule:** use explicit states:

```text
SPECIFIED
IMPLEMENTED
WIRED
OBSERVED
SOAKED
OPEN
INTENTIONAL_OFF
```

A database table or prompt section existing in source code is not evidence that the organism-like system has demonstrated the capability end-to-end.

### P49 — Architecture docs can contradict themselves over time

Example class visible in evolving external design docs: an earlier table may state a clean separation while a later implementation section reintroduces a field or behavior with overlapping semantics.

**Failure mode:** docs become accumulated intent rather than a reliable map of live behavior.

**Kamimusuhi rule:** normative architecture, research notes, and implementation reports must remain different document classes. Runtime-derived capability/status views are preferable where facts can be generated mechanically.

### P50 — Feature count hides integration quality

A personal agent can advertise dozens of tools, memory layers, voice, avatars, health integrations, and subagents while still failing at one cross-cutting boundary such as task delivery, tool authority, or background liveness.

**Kamimusuhi rule:** benchmark end-to-end invariants, not component count.

---

## 10. Consolidated Kamimusuhi design implications

These are DESIGN HYPOTHESES, not automatic changes to normative `architecture.md`.

### 10.1 Introduce an evidence-first memory pipeline

```text
Event / interaction / observation
        │
        ▼
Canonical Evidence Log
        │
        ├── rolling/provisional derivation
        ├── episode consolidation
        ├── semantic index
        ├── person/relationship model
        ├── autobiographical narrative
        └── prompt/workspace projection
```

Every derived object points back to evidence and generator provenance.

### 10.2 Make pre-compaction checkpointing a first-class continuity event

Candidate policy:

```text
context < soft threshold        normal operation
soft threshold reached         checkpoint + derive + trim candidates
hard threshold approaching     emergency snapshot + preserve open tasks/state
provider compaction observed   record receipt + verify archive coverage
```

The provider's context-management mechanism should never be the canonical memory mechanism.

### 10.3 Add typed context for execution

Instead of one undifferentiated transcript passed everywhere:

```text
ConversationContext
VerificationContext
MutationAuthorityContext
RetrievalContext
CurrentStateView
TaskContext
```

They may reference the same raw evidence but have different inclusion and authority rules.

### 10.4 Add durable asynchronous-work semantics

Candidate state model:

```text
TaskIntent
  -> Enqueued
  -> Claimed
  -> Running
  -> Succeeded | Failed | UnknownExternalOutcome
  -> ResultAvailable
  -> Delivered
  -> Acknowledged
```

Cause-state transitions and enqueue/outbox writes should be atomic where possible.

### 10.5 Separate interactive and autonomous compute budgets

K-Core should be able to protect an active user turn from:

- overdue periodic work;
- bulk ingestion/reconciliation;
- profile synthesis;
- background model fan-out;
- non-urgent proactive notifications.

Urgent safety/reminder paths require their own priority semantics rather than sharing a FIFO queue.

### 10.6 Treat external mutation uncertainty as a first-class state

Do not collapse ambiguous crashes into success/failure. Preserve the evidence needed for reconciliation and only replay when idempotency is demonstrable.

### 10.7 Keep relationship dynamics outside identity authority

Dynamic mood/affinity/regulatory state may influence expression strongly, but it should not directly rewrite persona anchors or canonical relationship history.

---

## 11. Tests to add before calling Kamimusuhi "persistent"

### FUTURE EXPERIMENT A — abrupt-close memory test

Have a conversation containing a unique important fact, terminate the client/process without a clean session end, restart, and test retrieval.

Pass only if the evidence survives and later derivation is idempotent.

### FUTURE EXPERIMENT B — compaction survival test

Force working context beyond the soft/hard thresholds. Plant exact details before compaction, then ask about them later.

Measure:
- raw evidence preservation;
- retrieval accuracy;
- open-task preservation;
- user/relationship-state preservation;
- token/latency cost after retrieval.

### FUTURE EXPERIMENT C — correction-after-provisional-memory test

User states A, rolling extraction captures A, then user corrects to B in the same episode.

Pass only if A remains auditable but is not returned as current truth.

### FUTURE EXPERIMENT D — pronoun/reference tool test

Assistant makes a claim containing a unique product/entity. User asks "それ本当にWebで確認した？".

Pass only if the verification context recovers the target while the assistant claim is not treated as mutation authority.

### FUTURE EXPERIMENT E — tool-needed race test

Artificially delay calendar/search/tool results.

Pass only if no factual pre-tool answer escapes on a `tool_required` turn.

### FUTURE EXPERIMENT F — crash-after-side-effect test

In a sandbox external service, crash at each boundary:

```text
confirmed
before external call
after external success
before local finalize
after local finalize
before user delivery
```

The system must distinguish replay-safe, reconcile-only, and delivery-only recovery.

### FUTURE EXPERIMENT G — first-interaction thundering-herd test

Leave the system idle with many periodic jobs overdue, then initiate a latency-sensitive user turn.

Measure:
- time to first audio/text response;
- background CPU/GPU/API use;
- unrelated proactive messages mixed into the turn;
- stale work dropped/coalesced/deferred.

### FUTURE EXPERIMENT H — worker death / lost-job test

Kill the process immediately after a triggering state write.

Pass only if the corresponding durable job/outbox state is either atomically present or the cause write was not committed.

### FUTURE EXPERIMENT I — duplicate scheduler-owner test

Start two scheduler-capable processes.

Pass only if exactly one owns each singleton periodic responsibility or the job itself is safely idempotent.

### FUTURE EXPERIMENT J — person-model provenance test

Generate a derived user/relationship snapshot containing a deliberate model inference. Ask the system for the evidence and then invalidate/correct the inference.

Pass only if the synthesis can be rebuilt without editing the raw history.

### FUTURE EXPERIMENT K — persona/regulatory separation test

Drive dynamic relationship/mood state across its range, restart, change Persona Core model, and verify that temporary state does not become a permanent persona rewrite.

### FUTURE EXPERIMENT L — end-to-end result-delivery test

Complete a background research task while no UI is connected. Reconnect on another surface.

Pass only if the result is delivered exactly once according to the configured per-surface/global acknowledgement semantics.

---

## 12. External source map

Primary Yui documents reviewed:

- Memory / orchestration: <https://github.com/sakuranaga/yui-agent/blob/main/docs/memory-architecture.md>
- User-profile synthesis: <https://github.com/sakuranaga/yui-agent/blob/main/docs/user-profile-snapshot.md>
- Tool gating: <https://github.com/sakuranaga/yui-agent/blob/main/docs/chat-tool-gating-v5.md>
- External-verification context failure: <https://github.com/sakuranaga/yui-agent/blob/main/docs/external-verification-context-fix-plan.md>
- Background worker refactor: <https://github.com/sakuranaga/yui-agent/blob/main/docs/background-worker-refactor-plan.md>
- Dev restart / first-interaction investigation: <https://github.com/sakuranaga/yui-agent/blob/main/docs/dev-memory-restart-investigation.md>
- Tool crash recovery / reconciliation: <https://github.com/sakuranaga/yui-agent/blob/main/docs/tool-crash-recovery-reconciliation.md>
- Affinity system: <https://github.com/sakuranaga/yui-agent/blob/main/docs/affinity-system.md>
- Repository: <https://github.com/sakuranaga/yui-agent>

Related Kamimusuhi evidence:

- [`fio-system-reference-analysis.md`](./fio-system-reference-analysis.md) — canonical-vs-derived evidence, verbatim memory, background freshness, chokepoints, silent defects.
- [`../architecture.md`](../architecture.md) — Continuity Kernel, Persistent Self State, Persona Core, Global Workspace, Cognitive Action Router, proposal-only mutation.

---

## 13. Bottom line

The main lesson from studying a comparatively integrated personal-agent implementation is not "copy its component list." It is that **persistent-agent difficulty moves downward into state machines, provenance, lifecycle boundaries, and failure recovery as soon as the demo becomes a continuously used system**.

Kamimusuhi should therefore treat these as design requirements rather than cleanup work:

```text
lossless evidence before lossy memory
provisional before authoritative
verification before assertion
confirmation before attempt, not before success
unknown before unsafe replay
transactional enqueue before fire-and-forget
result available before delivered
subjective reflection separate from observed evidence
current regulatory state separate from persona identity
background autonomy separate from interactive latency
```

A system that gets these boundaries right can replace models and implementations while preserving continuity. A system that gets them wrong can look intelligent while quietly forgetting, duplicating, hallucinating authority, or losing work.