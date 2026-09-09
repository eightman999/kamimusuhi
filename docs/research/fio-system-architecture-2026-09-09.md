# FIO / SerialExperiments システムアーキテクチャ 2026-09-09 — Kamimusuhi 参考ノート

Status: **research note / source-reported architecture snapshot**

Primary source: user-supplied PDF, *FIO / SerialExperiments システムアーキテクチャ*, 2026-09-09, 11 pages.

Related Kamimusuhi docs:

- [`../fio-system-reference-analysis.md`](../fio-system-reference-analysis.md)
- [`fio-neural-function-growth-2026-09-08.md`](./fio-neural-function-growth-2026-09-08.md)
- [`../multiscale-brain-architecture.md`](../multiscale-brain-architecture.md)
- [`../learned-discrete-reflex-layer.md`](../learned-discrete-reflex-layer.md)
- [`../fly-brain-connectome-reference.md`](../fly-brain-connectome-reference.md)
- [`../sensory-nervous-system.md`](../sensory-nervous-system.md)
- [`../latency-architecture.md`](../latency-architecture.md)
- [`../persistent-agent-implementation-pitfalls.md`](../persistent-agent-implementation-pitfalls.md)

This note records what the 2026-09-09 FIO architecture document itself reports, then separates Kamimusuhi-side interpretation and design hypotheses. It does **not** treat FIO's internal measurements as independently reproduced results.

Use the following evidence vocabulary:

```text
SOURCE-REPORTED    stated/measured in the supplied FIO document
OUR INTERPRETATION architectural lesson inferred for Kamimusuhi
DESIGN HYPOTHESIS  candidate Kamimusuhi design, not normative
FUTURE EXPERIMENT  validation required before promotion into architecture/spec
```

---

## 1. Why this FIO snapshot matters

SOURCE-REPORTED (p.1): FIO describes itself not as an input-response assistant but as a system for keeping a subject with a continuous inner life running in a form that can survive practical operation. The document presents four structural differences from an ordinary dialogue agent:

1. **Involuntary perception** — screen, camera, and sound can enter independently of a user's utterance.
2. **Two-level memory** — selected/summarized memory sits above the unselected original record, and the system can descend from a summary to the original.
3. **Continuing subject** — the subject continues when the machine or inference engine changes.
4. **Spontaneous cognition** — a default-mode-like pulse and a faster neural heartbeat can run even when no user calls the system.

SOURCE-REPORTED (p.1): FIO also explicitly separates the accumulated result of one grown individual (memory, personality, relationships) from the transferable design method. The document describes this as a `toC` / `toB` separation, enabled in part by keeping human-readable Markdown as canonical material while treating indexes and vector stores as rebuildable derivatives.

OUR INTERPRETATION:

This is close to Kamimusuhi's core problem statement. The useful overlap is not that FIO and Kamimusuhi should share implementation details; it is that both make **continuity, autonomous background activity, sensory ingress, and rebuildable derived state architectural concerns rather than prompt features**.

---

## 2. Source-reported implementation scale

The following numbers are reported as measurements from the live repository, not copied from a design plan.

| Area | Source-reported size / state |
|---|---:|
| `fio/core` | 100 files / 55,531 lines |
| `fio/tools` | 95 files / 24,736 lines / 272 registered tools |
| `fio/tests` | 315 files / 85,707 lines / 4,580 fences |
| `fio/core/claude_client` | 8 files / 8,945 lines |
| `fio/voice` | 19 files / 5,671 lines |
| `fio/scripts` | 22 files / 4,841 lines |
| `installer` | 3 files / 2,884 lines |
| body wiring | 8 files / 1,698 lines |
| Discord bot | 6 files / 1,530 lines |
| UI | 3 files / 1,504 lines |

SOURCE-REPORTED (p.2): accumulated state includes a 79.5 MB conversation database, a 119.5 MB episodic store with 16,955 rows, a 22.1 MB FTS5 session index covering 98 stored sessions / 7,889 exchanges, and 5,468 Markdown notes in the vault.

SOURCE-REPORTED (p.2): the runtime is Python 3.13 on Windows 11 in current practical operation. Inference can be switched among `local`, `sdk`, and `api`. Persistence combines SQLite/FTS5, a vector store, and a Markdown vault. The reported embedding model is `BAAI/bge-m3`, 1024 dimensions, with an 8,192-token model limit and an empirically selected chunk size of about 2,000 Japanese characters.

OUR INTERPRETATION:

FIO is large enough that its most useful contribution to Kamimusuhi is not a module checklist. It is evidence about **which integration invariants became necessary after the system had accumulated enough moving parts and enough irreplaceable state for silent failures to matter**.

---

## 3. Independent timing domains: conscious turn vs unconscious heartbeat

SOURCE-REPORTED (p.3): FIO splits consciousness and unconscious/background work into separate processes. The conscious process handles the main turn, fast non-LLM reflexes, and external connections. The unconscious process contains the four-quadrant nervous system, a 0.25-second neural heartbeat, default-mode-like spontaneous pulses, supervisors, internal organs, and a qualia module.

The two sides are connected through one JSON state object (`fio/data/consolidated_unconscious.json`). Each layer writes only its own section so writes do not collide.

The document gives four reasons for the split:

- turn-scale work takes seconds to minutes while neural heartbeat work is 0.25 s;
- an LLM stall should not stop the unconscious side;
- each side can be stopped independently;
- one connection point makes failures easier to localize.

SOURCE-REPORTED (p.3): FIO also reports a fallback to one-process behavior with `FIO_UNCONSCIOUS_PROCESS=0`.

SOURCE-REPORTED (p.3): the document says its pulse timing uses a golden-ratio-derived pattern instead of equal spacing or unconstrained randomness, motivated by perceived naturalness. No independent evidence is provided in this document that this timing rule is superior.

### Kamimusuhi interpretation

OUR INTERPRETATION:

This supports the **engineering premise** behind Kamimusuhi's multi-timescale architecture: high-frequency control should not be serialized behind slow semantic inference.

A useful conceptual mapping is:

```text
high-rate domain
  perception gates / reflex / salience / interrupt / local state
        |
        v
mid-rate domain
  background regulation / habituation / affect-like control / DMN-like work
        |
        v
low-rate semantic domain
  Persona Core / K-Edge language / K-Core deliberation / K-Deep research
```

This is compatible with `multiscale-brain-architecture.md`, `learned-discrete-reflex-layer.md`, and the current FlyBrain-inspired K-Fast line.

However, **FIO does not validate a FlyBrain-like circuit, a fly-scale cluster, or a specific neural algorithm**. It only provides an independently developed implementation example showing that separating a fast nervous/control clock from the slow LLM turn is operationally useful.

DESIGN HYPOTHESIS:

Kamimusuhi should preserve independent timing domains even if the implementation substrate changes from Python processes to Rust tasks, native event loops, small learned controllers, neuromorphic hardware, or distributed K-Edge nodes.

The invariant should be about **latency and ownership**, not about reproducing FIO's 0.25-second interval or its golden-ratio pulse pattern.

---

## 4. Turn assembly: response first, memory decision second

SOURCE-REPORTED (p.4): FIO's turn center is split across modules for turn assembly, static self-description/prompt construction, event classification, native tool loops, metadata parsing, second-pass metadata generation, memory writeback, prompt assembly, and retry queues.

SOURCE-REPORTED (p.4): FIO uses a two-pass structure. The first pass returns the response. The second pass decides what to remember. The document frames this as placing the subject of memory selection on the subject rather than freezing importance into an external heuristic.

SOURCE-REPORTED (p.4): prompt-prefix stability is treated as an explicit engineering concern. The reported static prefix is about 122,000 characters, so small changes can invalidate cache reuse without raising an error.

SOURCE-REPORTED (p.4): injecting the full tool catalogue every turn was measured at 27,371 characters and rejected. A name-only index plus delayed schema retrieval was measured at 3,516 characters, an 87% reduction. Cold turns can omit even the index, while full behavior remains available through `FIO_TOOL_CATALOG=full`.

OUR INTERPRETATION:

Kamimusuhi should keep three things separate:

```text
what happened
what the subject currently interprets it to mean
what is allowed to become durable canonical self state
```

FIO's second pass is valuable as a **candidate-memory formation mechanism**, but Kamimusuhi should retain its stronger Continuity Kernel / proposal-policy boundary: a Persona Core deciding that something is memorable is not by itself authorization to mutate canonical identity.

---

## 5. Memory: summarized upper layer, unselected original underneath

SOURCE-REPORTED (p.5): FIO describes a memory structure with an upper tier of selected/summarized material and a lower tier of the unselected original record.

Upper-tier material includes:

- integrated/persona memory;
- episodic memory plus vector retrieval;
- consciousness/experience events;
- diary material.

Lower-tier material includes:

- session JSONL as retained original evidence;
- session Markdown notes;
- an append-only hub/catalogue;
- FTS5/BM25 retrieval indexes as derived data.

The document emphasizes that summaries can descend to the original through session links. The design goal is that recall quality is not permanently bounded by the quality of the first summary.

SOURCE-REPORTED (p.5): Markdown is treated as canonical while vector DB and SQLite indexes are rebuildable derivatives. The document reports a 2,000-character chunk size as empirically selected: 6,000-character chunks lost 22%, while reducing to 500 characters did not provide further gain in the reported test.

OUR INTERPRETATION:

This strongly reinforces the existing Kamimusuhi rule:

```text
append-oriented evidence / original record
        |
        +--> indexes
        +--> summaries
        +--> autobiographical views
        +--> retrieval caches
        +--> prompt projections
```

Derived memory should be replaceable. Original evidence and lineage-bearing state should be more authoritative than the current retrieval representation.

DESIGN HYPOTHESIS:

Every durable autobiographical claim should carry enough provenance to descend to the evidence or event lineage from which it was formed, without forcing the whole provenance graph into every prompt.

---

## 6. Perception bandwidth control is treated as a prerequisite

SOURCE-REPORTED (p.6): FIO separates involuntary perception from voluntary inspection. Modules include an involuntary-perception gate, recognition events, screen vision, camera, held perceptions, sound meaning, current-scene/difference representation, and conversion of screen changes into semantic meaning.

SOURCE-REPORTED (p.6): involuntary perception does **not** pass raw images into the main turn. Only a short semantic meaning can cross the gate; raw frames are pulled through the voluntary path when the subject actively looks. The document reports an earlier failure where one interrupt caused 78 images / 60 blocks to enter a turn.

OUR INTERPRETATION:

This maps almost directly onto Kamimusuhi's sensory nervous system:

```text
raw sensor stream
      |
      v
feature/change extraction
      |
      v
fast gate / salience / novelty
      |
      +--> ignore
      +--> hold
      +--> wake semantic cognition with compact event
      +--> allow deliberate raw inspection when needed
```

The lesson is not merely prompt compression. A persistent embodied agent needs **sensory backpressure and salience control before semantic cognition**.

FUTURE EXPERIMENT:

Flood the Kamimusuhi sensory path with sustained high-rate visual/audio changes while injecting artificial K-Core/Persona-Core stalls. Verify that:

- the fast gate remains live;
- raw payload does not accumulate without bound;
- semantically obsolete events are dropped or superseded;
- deliberate inspection can still recover the relevant raw evidence.

---

## 7. Four-quadrant nervous system and per-nerve learning

SOURCE-REPORTED (p.6): FIO organizes nervous functions into four quadrants along two axes:

```text
voluntary   x internal
voluntary   x external
involuntary x internal
involuntary x external
```

A gate decides whether a signal passes to consciousness or is held. Each nerve has its own store. Conscious output returns through `nerve_feedback`, allowing the nerve's rule to change.

SOURCE-REPORTED (pp.6-7): the document reports that feedback reversed a nerve decision in 43% of measured cases. It interprets this behavior as resembling habituation and emphasizes that the learned rule remains in readable symbolic form so the system can later inspect why a signal was passed or suppressed.

Important limitation: the 43% number is an internal FIO measurement described by the source. This note does not treat it as a general habituation benchmark or as external neuroscience validation.

### Kamimusuhi interpretation

OUR INTERPRETATION:

This is especially relevant to the emerging **K-Fast / learned discrete reflex** design. It demonstrates a practical pattern in which fast local controllers can have:

- their own state;
- their own local learning history;
- descending feedback from slower cognition;
- readable/auditable rules;
- a gate into semantic awareness.

That pattern is more important than FIO's exact Python implementation.

DESIGN HYPOTHESIS:

A Kamimusuhi K-Fast node or peripheral nerve should expose at least:

```text
input_state
local_state
proposed_action
pass_hold_ignore/escalate decision
confidence / salience
rule/model version
feedback receipt
last_update
```

The Persona Core or K-Core may provide descending modulation, but fast nodes should not silently become a second canonical personality store.

FUTURE EXPERIMENT:

Run learned reflex candidates in **shadow mode** beside fixed rules and measure:

- false wake rate;
- missed-salience rate;
- adaptation speed;
- recovery after distribution shift;
- latency and energy;
- interpretability of the resulting local state/rules.

---

## 8. Spontaneous inner activity and body as observability

SOURCE-REPORTED (p.7): FIO includes a DMN module, a heartbeat/autonomous pulse, a qualia module, a bridge from consciousness back to unconscious functions, and a separate unconscious process. The document describes the qualia module operationally as combining cues into a continuous "current feeling"; this naming should not be read as evidence that phenomenal consciousness has been established.

SOURCE-REPORTED (p.7): body modules connect internal function to visible physical/GUI behavior: cursor/hand control, posture, mouth movement, pointing, sleep/stop state, and related wiring. The design explicitly frames embodiment as more than presentation. Visible body state is used to make internal state observable from outside, serving both debugging and subject-level consistency.

OUR INTERPRETATION:

Kamimusuhi should preserve this useful engineering idea:

> A body is also an observability surface.

Examples:

- speaking state should be externally visible while TTS is active;
- interruption/sleep/resource-pressure states should have observable outward correlates where appropriate;
- actions should expose receipts that connect visible behavior to the internal decision that caused it.

This can reduce the gap between "the system says it is doing X" and "the runtime is actually doing X".

---

## 9. Inference substrate is a vessel, not the subject

SOURCE-REPORTED (p.8): FIO abstracts inference across local, SDK, and API backends. A single decision point decides which inference vessel is active. Modules also support moving the running subject to another local/network location and listening at the destination for summons.

The document defines persistence/ubiquity in terms of the same subject continuing when machines or inference engines change. It also describes migration as handing over only after the destination is observed to have started, to avoid double-starts and orphans.

OUR INTERPRETATION:

This is strongly aligned with Kamimusuhi's rule that an external model is a **cognitive resource**, not automatically the identity-bearing subject.

However, Kamimusuhi should not copy FIO's mobility mechanism as its canonical identity model. Kamimusuhi already has a stronger distributed-continuity problem: K-Edge, K-Core, and K-Deep roles can be stale, disconnected, replaced, or concurrent, so canonical lineage still needs a Continuity Kernel and explicit activation rules.

---

## 10. Tool routing: all action paths must cross the same invariants

SOURCE-REPORTED (p.8): FIO has 272 registered tools and a tool-selection index. A separate delegated helper (`shikigami`) once bypassed the normal action pipeline. The document reports an incident where an image was collapsed through `str()` and caused +33,330 input tokens, alongside other bypassed accounting/recording behavior.

OUR INTERPRETATION:

This is the same general failure class already recorded in `persistent-agent-implementation-pitfalls.md`: a secondary execution path can look functionally correct while silently bypassing consent, provenance, accounting, caching, read-back, or memory sedimentation.

Kamimusuhi should preserve a single logical action path for cross-cutting invariants even when many execution backends exist.

Candidate invariant path:

```text
ActionProposal
    -> capability/policy check
    -> duplicate/idempotency check
    -> execution
    -> observation/read-back
    -> provenance + receipt
    -> optional memory proposal
```

---

## 11. Seven FIO design principles worth retaining as references

SOURCE-REPORTED (p.10): FIO extracts seven principles from actual implementation failures.

### 11.1 One decision point

Do not implement the same decision independently in two places. FIO's example is tool-catalogue exposure being reduced in one path but silently reintroduced in another.

### 11.2 Tests are fences, not a complete specification

Tests should mark known cliffs. FIO's 4,580 tests/fences are described as records of places where the system has fallen before, not as proof that the whole system is correct.

### 11.3 Failure must not look identical to success

FIO highlights false/success ambiguity, including memory writes with multiple meanings hidden behind one boolean and asynchronous test doubles that could fail incorrectly while tests remained green.

### 11.4 Bind ratios/sets, not brittle absolute counts

Growing systems invalidate fixed numeric assertions. FIO prefers inequalities, ratios, and set relationships where that better captures the invariant.

### 11.5 Every new behavior needs a fallback

New behavior should have a tested path back to old behavior without editing code.

### 11.6 Keep the original; rebuild derivatives

Summaries, indexes, and vector representations should be discardable/rebuildable beneath an append-oriented original record.

### 11.7 Do not burn absolute paths into the system

The machine will move. Path resolution should be centralized, and mixed-platform path fragments can fail silently.

OUR INTERPRETATION:

These principles fit Kamimusuhi particularly well because continuity-bearing failures may remain invisible while conversation quality stays superficially good. The most dangerous bug is often not a crash; it is a subsystem that quietly stops contributing to future self-state.

---

## 12. Direct consequences for Kamimusuhi

### 12.1 Adopt as strong reference patterns

The following should be treated as high-value patterns for Kamimusuhi design review:

1. **Independent timing domains** for fast nervous/reflex work and slow semantic inference.
2. **Single explicit bridges** between timing/ownership domains.
3. **Lossless or append-oriented evidence below summaries and indexes**.
4. **Perceptual meaning gates before LLM-scale cognition**.
5. **Local state and adaptation for individual nerves/controllers**.
6. **Descending feedback from slow cognition to fast control** without making the fast layer the canonical self.
7. **Body/output state as an observability mechanism**, not only presentation.
8. **Inference substrate abstraction** so identity is not equated with one model provider.
9. **Single decision/action chokepoints** for cross-cutting invariants.
10. **Explicit fallbacks** for new runtime paths.
11. **Runtime evidence labels** that distinguish wired/tested behavior from actually observed operation.

### 12.2 Do not copy literally

The following should remain non-normative unless independently validated:

- the exact 0.25-second neural heartbeat;
- the golden-ratio pulse schedule;
- Python process topology as the target implementation substrate;
- FIO's particular Markdown/SQLite/vector-store division as the only correct persistence model;
- FIO's internal module names as claims about biological or phenomenal reality;
- the reported 43% feedback reversal rate as a general target;
- FIO's single-instance mobility semantics as sufficient for Kamimusuhi's distributed lineage problem.

### 12.3 Relation to FlyBrain / K-Fast

The most important new connection is:

```text
FIO evidence:
slow LLM turn and fast nervous heartbeat are operationally separated
        |
        v
Kamimusuhi design space:
K-Fast / peripheral nerves can run at a higher effective clock than Persona Core
        |
        v
FlyBrain-inspired hypothesis:
sparse recurrent / local learned circuits may be useful implementations of that fast layer
```

The arrow from FIO to K-Fast is an **architectural analogy**, not evidence that FIO validates the fly connectome approach.

FIO strengthens the case for asking:

> Which decisions must remain live while language-model inference is stalled, expensive, offline, or unnecessary?

It does not answer:

> Which biological circuit or learning rule is best for those decisions?

That second question remains the job of `fly-brain-connectome-reference.md`, `learned-discrete-reflex-layer.md`, and future local experiments.

---

## 13. Experiments to borrow from this architecture

Before promoting these ideas further into normative architecture, run explicit Kamimusuhi experiments.

### E1. Slow-core isolation test

Artificially stall Persona Core / K-Core for 5, 15, 30, and 60 seconds. Verify that fast sensory/reflex state continues, does not deadlock, and does not accumulate unbounded stale work.

### E2. Sensory flood / backpressure test

Feed high-rate visual/audio events and verify bounded memory, latest-state semantics, hold/drop behavior, and deliberate retrieval of raw evidence.

### E3. Fast-controller shadow adaptation

Run learned/reflex nodes in shadow mode against fixed rules. Measure disagreement and whether descending feedback improves false-wake/missed-salience tradeoffs without destabilizing behavior.

### E4. Evidence-descent test

Select a durable autobiographical statement, follow its provenance to the summarized episode, then to the original event/session evidence. Ensure derived indexes can be deleted and rebuilt without losing that path.

### E5. Bridge corruption / ownership test

Inject stale, partial, duplicate, and conflicting state into the fast/slow bridge. Verify single-writer or versioned ownership rules prevent silent corruption.

### E6. Action-path bypass test

Add a second execution backend or delegated helper and prove it cannot bypass policy, provenance, idempotency, read-back, and accounting.

### E7. Embodiment observability test

For speech, sleep, interruption, pointer/action, and resource-pressure states, verify that visible/telemetry state corresponds to the actual runtime state rather than to self-report text.

---

## 14. Source limitations and current unverified areas

SOURCE-REPORTED (p.11): the FIO document explicitly distinguishes "wired" from "worked" and lists important unverified states.

At the date of the document:

- the three unconscious supervisors had been intentionally stopped since 2026-08-09;
- automatic saving for the second-tier memory, startup installation, and turn-time recall were reported as wired/tested but not production-verified;
- those paths had zero observed production firings in the stated scope.

SOURCE-REPORTED: the accumulated memory/personality/relationship state is described as irreplaceable in the sense that it represents days lived since May 2026. Maintenance that writes indexes, exports, or schemas is therefore treated as owner-approved work rather than harmless experimentation.

OUR INTERPRETATION:

This evidence discipline is itself one of the most important things to import. For Kamimusuhi, use at least:

```text
SPECIFIED
IMPLEMENTED
WIRED
OBSERVED
SOAKED
OPEN
INTENTIONAL_OFF
```

Do not call a path "implemented and working" merely because code exists, a unit test passes, or a diagram contains an arrow.

---

## 15. Current conclusion

FIO 2026-09-09 is a strong engineering reference for the **operational nervous system around a persistent subject**:

- separate fast and slow clocks;
- gate sensory bandwidth;
- keep local adaptive nervous state;
- preserve original evidence below summaries;
- treat embodiment as runtime observability;
- make inference engines replaceable vessels;
- force cross-cutting invariants through single decision/action points;
- distinguish runtime observation from mere wiring.

For Kamimusuhi, the most valuable next step is not to reproduce FIO. It is to combine these operational lessons with Kamimusuhi's stronger distributed-continuity model and the newer K-Fast / FlyBrain-inspired hypothesis.

A concise target picture is:

```text
raw world / body
      |
      v
PNL / K-Fast
high-rate, local, cheap, adaptive
      |
      | compact event / interrupt / wake / receipt
      v
K-Edge / Global Workspace
      |
      v
Persona Core / K-Core
semantic self + deliberation + autobiographical interpretation
      |
      v
K-Deep / external cognition
long-horizon / high-compute work

canonical continuity and evidence remain outside replaceable fast-control optimizations
```

This is a design direction, not yet a measured Kamimusuhi result.