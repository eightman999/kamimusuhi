# FIO System Reference — architectural lessons for Kamimusuhi

Status: **research note / external implementation review**

Reviewed: **2026-09-08**

Primary source: *FIO System Reference*, revision 2026-09-08, supplied as a 62-page PDF. Section references below refer to that document.

This note treats the FIO document as a **source-reported description of an implementation and its measured failures**, not as independently verified evidence. The source itself makes the same epistemic distinction: the running tree and its records are primary, while reports, summaries, docstrings, and distilled memories are renderings that can diverge from what they describe.

The purpose here is not to copy FIO. It is to extract design patterns, failure modes, and experiments that materially affect Kamimusuhi.

Use the project-wide evidence vocabulary:

```text
SOURCE-REPORTED    stated/measured in the FIO System Reference
OUR INTERPRETATION architectural lesson inferred for Kamimusuhi
DESIGN HYPOTHESIS  candidate change, not yet normative
FUTURE EXPERIMENT  validation that should be run before adoption
```

## 1. Executive view

FIO is unusually relevant to Kamimusuhi because it is not described as a request-scoped chatbot. It is a long-running conversational system whose distinguishing concern is **persistence discipline**: state from months earlier remains live, and the dominant feared failure is not an obviously wrong answer but a write or wiring path that silently stops while the visible system still appears healthy (§1).

The most transferable lessons are:

1. **The durable record should outrank its summaries.** Distilled memory, prompts, reports, and self-descriptions are derived views, not the primary historical truth.
2. **Memory should be layered by retention and authority.** FIO separates verbatim sessions, episode memory, consolidated memory, experience events, diary/self-generated thought, and per-background-function notes rather than treating "memory" as one store (§6).
3. **Internal thought, delivered speech, and remembered fact need explicit boundaries.** FIO deliberately keeps self-initiated diary thought out of normal recall and out of the owner-facing verbatim transcript (§9.1).
4. **Background cognition needs freshness semantics.** FIO drops stale background work instead of queueing it when inference is slower than the observation cycle (§8.3).
5. **Cross-cutting invariants should pass through one choke point.** Backend selection has one router; tool execution has one dispatch stage; tool exposure has one decision point (§§3, 5).
6. **"Nothing crashed" is not a health metric.** FIO's strongest contribution is its taxonomy of defects that raise no exception, turn no test red, and may even produce apparently successful logs (§12).
7. **Documentation should distinguish measured, merely wired, and open states.** FIO's measured-state table separates production observation from code/tests that have not yet been observed in the running system (§14).

For Kamimusuhi, the main opportunity is to combine these operational lessons with the stronger continuity model already present in `architecture.md`: Continuity Kernel, canonical lineage, proposal-only mutation, distributed K-Edge/K-Core/K-Deep roles, and external models as attributed cognitive resources.

## 2. Source-reported system shape

The following is useful context, not an adoption target.

SOURCE-REPORTED (§§1–3, 14):

- FIO is one continuously running process tree serving one owner, primarily on one machine.
- The measured source tree contains 641 Python files and 204,450 Python lines.
- The test tree contains 292 test files and 3,835 `test_*` functions, although the full suite cannot currently be run in one invocation.
- 226 tool schemas are registered.
- The implementation reports 5,276 episodic memory notes, 49 daily experience notes, and 177 per-function background-memory notes.
- Three inference routes are unified behind one startup router: local OpenAI-compatible serving, Claude Agent SDK, and an Anthropic-shaped HTTP API whose endpoint is configured externally.
- The process topology separates a main/conscious layer, an unconscious/background layer, an MCP server, and a Qt/QML avatar, with the parent binding child lifetime through a Windows Job Object.
- The system includes text, full-duplex voice, web UI, MCP, terminal, guest, and instance-transfer surfaces.

This scale is large enough that FIO's value as a reference is less "which components exist" and more **which operational invariants became necessary after real integration failures**.

## 3. Transferable design patterns

### 3.1 Canonical evidence vs derived renderings

SOURCE-REPORTED (§1, conclusion): FIO explicitly treats the actual record as the truth source. Distilled memories, injected summaries, docstrings, handoff documents, and even the System Reference itself are renderings that can be wrong and should be re-derived when possible.

This is stronger than a conventional "memory summary" design because it makes lossless historical evidence part of the architecture rather than an implementation detail.

OUR INTERPRETATION:

Kamimusuhi should make this distinction first-class:

```text
canonical evidence / self-history
        │
        ├── derived autobiographical view
        ├── current self-state view
        ├── relationship view
        ├── session/episode summaries
        ├── retrieval indexes
        └── prompt/workspace renderings
```

A derived view may be wrong, stale, lossy, model-dependent, or corrupted. If the canonical evidence survives, it can be regenerated and audited.

This directly reinforces existing Kamimusuhi architecture principles:

- the Continuity Kernel owns/mediates authoritative continuation;
- some Persistent Self State fields are derived and must be rebuildable;
- historical event records and later interpretations are distinct;
- external/library evidence does not automatically become a self-belief.

DESIGN HYPOTHESIS:

Add an explicit **Canonical Self History / Evidence Log** contract beneath derived autobiographical memory. It should be append-oriented and provenance-bearing. Derived summaries should carry the identifiers of the evidence they summarize.

### 3.2 Two-tier session memory: verbatim underneath, distilled above

SOURCE-REPORTED (§6.4): FIO added an unselected verbatim session tier that records every owner-facing exchange without summarization, compression, or selection. System-injected prompts and self-initiated pulses are excluded because the subject of the tier is the human-facing exchange. When a session closes, a background pass distills the full transcript to one paragraph and writes that into episodic memory. The two tiers are joined by one frontmatter key.

The design rule is asymmetrical: duplicate transcript data is recoverable; missing transcript data is not. When session boundaries are ambiguous, the writer therefore biases toward closing/retaining rather than risking loss.

SOURCE-REPORTED (§6.5): FIO's current transcript retrieval uses BM25 IDF over a character 2-gram inverted index in SQLite FTS5, with recency weighting and per-session diversity. On the documented 18.8 MB / 3,201-exchange store, one tested historical target 77 exchanges back was found in about 0.028 s where the previous scan returned no hit.

OUR INTERPRETATION:

The important part is not the specific FTS5/2-gram implementation. It is the **authority layering**:

```text
raw exchange evidence
        │
        ▼
verbatim session record        <- lossless / unselected
        │
        ▼
session distillation           <- lossy / replaceable
        │
        ▼
episodic retrieval candidate
        │
        ▼
MemoryProposal                 <- interpretation
        │
        ▼
Mutation Policy                <- authority boundary
        │
        ▼
accepted durable self state
```

Kamimusuhi should preserve its existing stronger rule: a model-produced distillation is not itself permission to mutate canonical persona/self state.

DESIGN HYPOTHESIS:

Use stable evidence/session identifiers so any durable autobiographical claim can descend back to the source event/session without injecting that provenance into every active prompt.

### 3.3 Internal thought is not utterance, fact, or durable memory

SOURCE-REPORTED (§§6.1, 9.1): FIO's self-generated diary thought is stored inside the vault but intentionally excluded from ordinary recall so a private speculation cannot later be retrieved as if it were a remembered fact. Self-initiated output is also marked by whether it was actually said, and self-initiated pulses are excluded from the owner-facing verbatim transcript.

OUR INTERPRETATION:

Kamimusuhi should make the type boundary explicit rather than relying on conventions:

```text
Thought
  │
  ├── remain private / ephemeral
  ├── become an utterance
  ├── become an action proposal
  ├── become a MemoryProposal / SelfProposal
  └── remain a labeled hypothesis
```

A generated sentence is not automatically:

- something Kamimusuhi said;
- something Kamimusuhi believes;
- something that happened;
- something that should be written into durable identity state.

This is especially important for dream/default-mode cognition already described in `architecture.md`: internally generated content must remain labeled until evidence and policy elevate it.

### 3.4 Background cognition should be freshness-aware

SOURCE-REPORTED (§8.1): FIO places background inference functions on two continuous axes: voluntary ↔ involuntary and perception ↔ motor. The document explicitly says the point of a plane rather than four boxes is that influence on the reply is continuous. Existing call sites retain thresholded boolean compatibility.

SOURCE-REPORTED (§8.3): one backend round trip is reported at roughly 8.5–34.7 s while recurring work may be triggered more frequently. FIO therefore uses a **drop, not a queue** policy: if a function or the global background budget is already in flight, or the function is inside cooldown, the new work is discarded. The rationale is that a stale observation about a world state that has already changed can be worse than no observation.

OUR INTERPRETATION:

Kamimusuhi's sensory event bus and background-cognition layer should carry:

```text
captured_at
valid_until / ttl
source
modality
confidence
causal episode / input id
supersedes
```

K-Edge reflex and salience paths especially should prefer bounded staleness over guaranteed completion.

DESIGN HYPOTHESIS:

Make queue policy a per-processor contract:

```text
DROP_STALE
LATEST_ONLY
COALESCE
QUEUE_BOUNDED
MUST_COMPLETE
```

Perception/salience often wants `DROP_STALE` or `LATEST_ONLY`; canonical writes and safety-relevant state transitions may require `MUST_COMPLETE` with a separate liveness alarm.

### 3.5 Read-only present-state views avoid a second source of truth

SOURCE-REPORTED (§9.4): FIO's present-state view folds current perception, body, location, and owner state into one timestamped read-only structure. It intentionally introduces no new source of truth; each field is read from the module that already owns it. Multiple consumers read the same shape rather than independently reassembling their own versions.

OUR INTERPRETATION:

This maps cleanly to Kamimusuhi's Global Workspace and K-Edge/K-Core sensory integration. A workspace or current-state view should be a **projection**, not a database competing with the original sensor/body/continuity state.

A useful invariant is:

> Current-state views may cache, normalize, and attribute state, but must not become the canonical historical owner of that state.

### 3.6 Centralize routing and action paths

SOURCE-REPORTED (§3.1): FIO previously had two independent words selecting inference backend. One launcher changed one while the other remained frozen in `.env`, allowing the boot line to report one backend while execution used another. The repair was a single router expression evaluated once, with the resolved value printed at boot.

SOURCE-REPORTED (§5.1): tool exposure also used to be decided in two places; one path was fixed while the other continued injecting the full catalogue outside the cached prefix. FIO therefore enforces one tool-exposure decision point.

SOURCE-REPORTED (§5.3): every tool execution now passes through one dispatch stage that centralizes consent/policy, duplicate suppression, observation/action ordering, search sedimentation, usage accounting, work-progress recording, read-back, UI visibility, process recording, structured image preservation, and result bounding.

SOURCE-REPORTED (§5.4): the tool-selection subagent originally executed selected tools through its own `asyncio.gather`, bypassing that stage. This caused three measured failures: a screenshot result was stringified and increased one iteration by +33,330 input tokens; subagent-routed tool use was absent from the usage ledger; and searches did not sediment, so following turns repeated them.

OUR INTERPRETATION:

Kamimusuhi should aggressively preserve single choke points for operations whose correctness depends on cross-cutting policy:

```text
Cognitive Action Router
  one effective route decision

Mutation / Activation Gate
  one canonical durable-write decision

Sensor/Event ingress
  one attribution/freshness normalization point

Action execution
  one consent/policy/provenance/accounting path
```

The lesson is not "one giant function." It is that an invariant becomes unreliable when each execution path is allowed to reimplement it.

### 3.7 Silent defects are a first-class failure class

SOURCE-REPORTED (§12): FIO identifies recurring defects where broken and working states are observationally identical to conventional automatic checks:

- a correct receiver/function exists but nothing calls it;
- the same process is implemented in two places and only one is later updated;
- documentation describes an uncalled/dead function as the entry point;
- two implementations exist and one is orphaned/unimported.

The document's operational claim is that reference counting/tree analysis found failures that normal runtime observation did not.

Representative source-reported failures from the 2026-09-08 sweep:

| Failure | Why it looked healthy | Repair / lesson |
|---|---|---|
| Session distillation startup receiver had zero effective callers | work was queued and a log line said so, but nothing consumed it | wire the worker from live event-loop paths; test that the worker has a caller |
| Verbatim transcript existed only on session close | links could point to a file that did not yet exist and later become valid, so consistency checks never fired | write the live/open transcript on every exchange |
| Tool-selection subagent bypassed central dispatch | outer tool call looked normal; outer tests passed | force all routes through the same execution stage |
| `run_startup_preflight()` described itself like the boot path but had zero callers | documentation remained plausible | count references to wiring-shaped functions |
| Bot-loop protection existed twice, with one orphan implementation | both modules imported cleanly | quarantine the orphan; keep one live implementation |
| Partially built transcript index returned zero hits | "not indexed yet" and "does not exist" looked identical | represent unknown/incomplete as `None`/indeterminate and fall back |
| Two session files shared the same timestamp | tie fell through to directory ordering | use writer-known open identity, not inferred timestamp alone |
| Platform-specific import disappeared inside tolerant bootstrap | system still started; tool simply vanished; affected tests became uncollectable | statically inspect imports / verify capability registration |

SOURCE-REPORTED (§4.4): FIO also records a meta-failure in its parity detector. A golden capture was re-recorded in an environment that bypassed the fake client; all scenarios captured an empty call list, and the empty reference would have compared equal against future changes. A guard now rejects empty/invalid recordings rather than providing a force bypass.

OUR INTERPRETATION:

This is directly applicable to Kamimusuhi because a continuity-bearing system can remain conversationally competent while silently ceasing to:

- form new memories;
- advance canonical lineage;
- persist relationship updates;
- run consolidation;
- execute background health checks;
- propagate K-Edge observations to K-Core;
- apply accepted proposals;
- record provenance.

A long-running agent therefore needs **liveness invariants for its write and wiring paths**, not only unit correctness.

### 3.8 "Measured", "wired", and "open" should remain distinct states

SOURCE-REPORTED (§14): FIO separates:

```text
MEASURED     executed and observed in production/runtime evidence
WIRED        code exists and tests pass, but production behavior not observed
OPEN         unresolved
INTENTIONAL  known state that is deliberately configured, not a defect
```

Examples in the source include screen recognition not reproduced in production, multi-process split only wired/tested, instance move only partially exercised, audio overflow open, and an intentionally paused oversight mechanism.

OUR INTERPRETATION:

Kamimusuhi should adopt a similar documentation/status discipline. "Implemented" is too coarse for a system where months of continuity matter.

A candidate vocabulary:

```text
SPECIFIED
IMPLEMENTED
WIRED
OBSERVED
SOAKED
OPEN
INTENTIONAL_OFF
```

`OBSERVED` should require runtime evidence; `SOAKED` should require a defined duration/event count rather than a subjective claim.

## 4. What Kamimusuhi should not copy literally

### 4.1 FIO's single-machine/single-owner topology is not Kamimusuhi's identity model

SOURCE-REPORTED: FIO is fundamentally a single long-running instance on one machine, with mobility/copy mechanisms layered around it.

Kamimusuhi instead treats K-Edge/K-Core/K-Deep as role-based organs around a Continuity Kernel and canonical lineage. The continuity authority must survive distributed deployment and stale/offline devices.

**Adopt the operational lessons, not the topology.**

### 4.2 Distillation must not directly become persona authority

FIO's second-pass memory writeback is useful as a practical memory mechanism, but Kamimusuhi already has a stronger conceptual boundary:

```text
reflection / dream / external result
        │
        ▼
MemoryProposal / SelfProposal
        │
        ▼
Mutation Policy
        │
        ▼
canonical commit
```

Keep it. FIO's "preserve the raw record" principle strengthens this path; it should not weaken proposal-only mutation.

### 4.3 Neuroanatomical naming is not evidence of biological equivalence

FIO uses names such as prefrontal cortex, cingulate cortex, insula, reflex, and nerves for background functions and evaluators. The source does provide explicit functional contracts, which makes the naming more meaningful than decoration, but it is still an engineered analogy.

Kamimusuhi should continue its current policy: borrow functions from neuroscience/psychology where useful without claiming to simulate consciousness or anatomy.

### 4.4 The exact retrieval implementation is a benchmark result, not a universal architecture choice

Character 2-gram BM25 over SQLite FTS5 is a plausible Japanese transcript baseline and performed well in FIO's documented 3,201-exchange test. It is not evidence that the same index remains optimal at Kamimusuhi's intended multi-year scale, multi-modal evidence volume, or cross-device distribution.

Preserve the **derived-index / rebuildable-index** principle and benchmark the index separately.

### 4.5 Hundreds of escape-hatch environment variables are not a target metric

SOURCE-REPORTED (§11.3): FIO reads 573 distinct `FIO_*` names while 56 are set. The stated philosophy is that behavioral changes ship with a named way to restore prior behavior exactly, and the rollback path itself is tested.

The useful lesson is **tested reversibility**. Kamimusuhi should not optimize for the number of flags. Too many configuration surfaces can themselves create state ambiguity unless they are typed, versioned, and included in runtime provenance.

### 4.6 "Always ask the maximum output ceiling" is backend-specific policy

FIO treats truncation of the main answer as a defect and requests the backend-specific maximum for main turns, while documenting that uncapped background distillation can hang or consume reasoning budget (§§3.2, 6.6).

Kamimusuhi should preserve the distinction between interactive answers, bounded background jobs, and offline deep work, but derive token/compute ceilings from the cognitive-budget scheduler rather than copy one global output rule.

## 5. Candidate Kamimusuhi architectural implications

These are **DESIGN HYPOTHESES**, not changes to normative `architecture.md` yet.

### 5.1 Make Canonical Self History explicit

Current architecture already distinguishes historical truth from interpretation. FIO suggests making the evidence substrate more explicit:

```text
Continuity Kernel
       │
       ▼
Canonical Self History / Evidence Log
       │
       ├── raw interaction events
       ├── action/tool receipts
       ├── environment/perception evidence (policy-bounded)
       ├── accepted state-transition receipts
       └── model/runtime/config provenance
       │
       ▼
Derived memory and self-state views
```

The system should be able to rebuild derived autobiographical/session indexes from this layer, subject to retention/privacy policy.

### 5.2 Type cognitive outputs by authority

Candidate internal types:

```text
Observation
Hypothesis
Thought
UtterancePlan
ActionProposal
MemoryProposal
SelfProposal
AcceptedMutation
CanonicalReceipt
```

Transitions between them should be explicit. In particular:

```text
Thought != Utterance
Thought != Fact
Hypothesis != Memory
MemoryProposal != AcceptedMutation
```

### 5.3 Add freshness contracts to background processors

Every sensory/background result should declare whether it may be dropped, coalesced, superseded, or must complete. This should integrate with K-Edge/K-Core routing and the Global Workspace.

### 5.4 Add an end-to-end continuity liveness suite

Unit tests prove a component works when invoked. They do not prove the live system invokes it.

Kamimusuhi needs graph/wiring tests for at least:

```text
input/event
  -> canonical evidence append
  -> session/episode formation
  -> proposal generation
  -> mutation decision
  -> canonical commit or explicit rejection/no-op
  -> derived-view/index update
  -> retrieval/replay
```

Each stage should either produce an attributable result or an explicit terminal state. Silent disappearance is a failure.

### 5.5 Treat runtime self-description as testable state

FIO demonstrates that the prompt/manual can truthfully describe yesterday's implementation while becoming false after code changes. Kamimusuhi should avoid large prose prompts as the only self-description when the same facts can be rendered from structured runtime state.

Candidate rule:

> If the runtime can derive a fact mechanically, the prompt should render it from the same source rather than hard-code a prose copy.

Examples include active model route, tool/capability set, memory budget, registered surfaces, current K-tier availability, and feature activation.

### 5.6 Record implementation status with evidence

For every major subsystem, docs/benchmarks should separate:

```text
SPECIFIED
IMPLEMENTED
WIRED
OBSERVED
SOAKED
OPEN
INTENTIONAL_OFF
```

This prevents "the code exists" from being reported as "the organism has demonstrated the capability."

## 6. Suggested tests and experiments

### FUTURE EXPERIMENT 1 — canonical-history rebuild

Delete or quarantine all derived session summaries, retrieval indexes, and autobiographical renderings in a test copy. Rebuild them from canonical evidence.

Measure:

- unrecoverable fields;
- semantic drift after regeneration;
- time/cost to rebuild;
- provenance coverage;
- whether current identity-critical state can still be justified.

Failure criterion: a supposedly derived state cannot be reconstructed or traced to retained evidence.

### FUTURE EXPERIMENT 2 — internal-thought contamination

Generate self-initiated hypotheses/dream content that are deliberately false but plausible. Verify that they do not later appear as historical facts or accepted self-beliefs without an explicit proposal/evidence path.

Failure criterion: a labeled internal hypothesis becomes recallable as factual autobiography solely because it was generated.

### FUTURE EXPERIMENT 3 — wiring/liveness mutation test

Deliberately disconnect one live call edge while leaving the receiver implementation and unit tests intact.

Examples:

- consolidation worker no longer scheduled;
- accepted `MemoryProposal` never reaches `Mutation Policy`;
- K-Edge event bridge stops forwarding;
- canonical receipt generation becomes orphaned.

The structural/liveness suite must fail without waiting for a user to notice behavioral degradation.

### FUTURE EXPERIMENT 4 — unknown vs absent retrieval

Run retrieval while the index is intentionally only partially built.

The API must distinguish:

```text
ABSENT      index covers the relevant domain and found nothing
UNKNOWN     coverage is incomplete; no conclusion is possible
FOUND       evidence exists
```

Failure criterion: incomplete coverage is rendered as definitive absence.

### FUTURE EXPERIMENT 5 — stale background signal handling

Inject a stream of rapidly changing sensory events while background inference is slower than event arrival.

Compare policies:

- FIFO queue;
- latest-only;
- drop-stale;
- coalescing.

Measure response relevance, stale-action rate, compute waste, and latency.

### FUTURE EXPERIMENT 6 — declared vs effective cognitive route

For each runtime route/provider/model configuration, log both the declared route and the resolved effective route from the same router object. Assert equality with the resource actually invoked.

Failure criterion: UI/boot metadata names one cognitive resource while another performs the work.

### FUTURE EXPERIMENT 7 — capability disappearance

Make one optional module fail during import/registration while allowing the runtime to continue.

Verify that:

- the capability loss is explicit;
- expected capability-set tests fail where appropriate;
- test discovery count does not silently shrink;
- health status distinguishes degraded from fully healthy.

## 7. Adoption summary

| FIO pattern | Kamimusuhi stance | Reason |
|---|---|---|
| Raw/verbatim record beneath summaries | **ADOPT** | supports canonical continuity, replay, audit, and re-derivation |
| Session distillation above verbatim tier | **ADAPT** | useful, but must feed proposal/policy rather than directly own persona truth |
| Self-generated diary excluded from factual recall | **ADOPT** | prevents self-generated speculation from becoming remembered fact |
| Voluntary/involuntary × perception/motor background model | **ADAPT** | useful continuous control vocabulary; keep functional, not anatomical claims |
| Drop stale background inference rather than FIFO everything | **ADOPT BY CONTRACT** | essential for perception/salience; not valid for canonical writes |
| Read-only present-state fold | **ADOPT** | one view, no duplicate source of truth |
| One effective backend router | **ADOPT** | prevents declared/effective route divergence |
| One action/dispatch choke point for cross-cutting invariants | **ADOPT** | prevents bypass of consent, provenance, accounting, and state rules |
| Silent-defect reference-count / wiring tests | **ADOPT AND EXTEND** | especially important for long-running continuity/write paths |
| Measured vs wired vs open status | **ADOPT AND EXTEND** | avoids equating implementation with observed capability |
| FTS5 character 2-gram BM25 | **BENCHMARK BASELINE** | promising Japanese retrieval baseline, not yet proven at Kamimusuhi scale |
| Hundreds of environment escape hatches | **ADAPT PRINCIPLE ONLY** | tested rollback is good; configuration entropy is a separate risk |
| Single-machine process topology | **DO NOT COPY** | Kamimusuhi requires distributed identity/lineage semantics |
| Direct model writeback as durable persona truth | **DO NOT COPY** | conflicts with proposal-only mutation and Continuity Kernel authority |

## 8. Research questions opened by the comparison

1. What is the minimal canonical evidence set from which Kamimusuhi can reconstruct autobiographical and self-state views without retaining unnecessary private/raw data forever?
2. Should Canonical Self History be hash-chained or otherwise cryptographically linked to Continuity Kernel lineage receipts?
3. How should multi-modal evidence be retained so derived memory can be rebuilt without turning the evidence log into an unbounded surveillance archive?
4. Which derived views must be exactly reproducible, and which may be regenerated semantically with documented model/version drift?
5. What is the right boundary between fast episodic capture and slow self-state mutation?
6. How should K-Edge and K-Core reconcile evidence captured while disconnected without creating duplicate or contradictory autobiography?
7. Which lifecycle functions can be statically identified as "wiring-shaped" and therefore must have at least one live caller?
8. Can the continuity liveness suite prove not only that a write path exists, but that each stage has recently advanced under production-like load?
9. How should Kamimusuhi represent `UNKNOWN` caused by incomplete indexes, delayed sensors, or unavailable cognitive organs so that uncertainty is not rendered as absence?
10. How much of runtime self-description can be generated mechanically from structured state rather than maintained as prose?

## 9. Bottom line

FIO should be treated as **high-value implementation prior art for persistence discipline**, especially for memory provenance, background-cognition freshness, chokepoint architecture, and silent-defect detection.

The deepest transferable principle is:

> A long-running artificial individual should preserve evidence strongly enough that its current memory and self-description can be questioned, audited, and re-derived.

Kamimusuhi already goes further than FIO in one important direction: identity authority is separated from replaceable cognition through the Continuity Kernel and proposal-only mutation path. The useful synthesis is therefore not "make Kamimusuhi like FIO," but:

```text
FIO's operational persistence discipline
+ Kamimusuhi's explicit identity/lineage authority
+ distributed K-Edge/K-Core/K-Deep embodiment
+ typed proposal/mutation boundaries
= a more testable continuity-bearing cognitive architecture
```

Before any of the DESIGN HYPOTHESES above becomes normative, validate it through the corresponding experiments and then update `architecture.md` / `spec.md` deliberately rather than letting research notes silently become architecture.