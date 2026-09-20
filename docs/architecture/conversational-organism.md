# Conversational Organism — C0 architecture

C0 turns Kamimusuhi's existing substrate (immutable evidence log, mutation
kernel, SQLite store, persona boundary) into a conversational organism: an
individual that holds conversations, keeps them as experience, reflects on its
own behaviour, and changes that behaviour through an auditable gate — never by
letting model output rewrite state directly.

This document describes the shape of that loop and where each piece lives.

## The loop

```text
user
  ↓
observation            user utterance → canonical evidence (immutable)
  ↓
runtime                workspace assembly: recent history + retrieved memory
  ↓                    + derived self model + operative params + current input
workspace
  ↓
persona backend        generates response text + proposal drafts (no authority)
  ↓
canonical episode      user/assistant utterances appended; turn metrics
  ↓                    recorded in the derived lane
reflection             Reflector reads episodes + memories + metrics
  ↓
improvement proposal   validated at intake; stored PENDING
  ↓
evaluation / replay    deterministic turn metrics; baseline vs candidate
  ↓                    replay over recent inputs
activation gate        accept → new activation; reject → decided row
  ↓
accepted derived state activation log + movable head
  ↓
next conversation      workspace assembles under the new operative view
```

## Two lanes, one spine

C0's central structural decision is the split between **canonical** and
**derived** state:

| Lane | Contents | Mutability | Head |
|------|----------|-----------|------|
| Canonical | individuals, commits, evidence records + links, sessions, turns, memory state records, mutation proposals/decisions, audit events | append-only | continuity head (forward-only) |
| Derived (C0) | improvement proposals, activations (full view snapshots), activation head, evaluations | lifecycle transitions only | `c0_head` — movable |

Canonical history is what happened. The derived lane is what the individual
currently *runs on*. Rollback moves the derived head backward over an
activation; the crossed activation row is marked `rolled_back_at`, not deleted,
and canonical evidence only ever grows — the rollback itself is narrated as a
canonical `SystemEvent`.

Why a separate lane: conversation needs an operative view that can be improved
and *un-improved*. Canonical continuity (the individual's history spine) must
never roll back — doing so would falsify the record. The derived lane carries
everything that may legitimately be revised: retrieval parameters,
conversation policy, the structured self model.

## What the model is never allowed to do

- **Mutate canonical state directly.** Persona/reflector output produces
  *drafts*. Canonical drafts (`episodic.capture`, `relationship.fact`,
  `relationship.correction`) are submitted to the continuity kernel, which
  runs `MutationPolicyV0`: structure, vocabulary, origin class, evidence
  presence, writer-epoch fencing and predecessor checks decide. A draft with
  `Inferred` origin cannot become an external fact.
- **Become its own memory.** Model-generated text is `OriginClass::Inferred`
  expression. It is recorded as such; it never upgrades to evidence or belief.
- **Invent proposal targets.** Improvement proposals address a closed
  vocabulary (`param_bounds` in `core/c0.rs`): retrieval top-k's, score
  thresholds, response-length bound, reflection interval, plus named self-model
  fields. An unknown target is a malformed proposal, not a clamped one.
- **Cite evidence it was not shown.** Intake validation
  (`validate_improvement_draft`) checks every `evidence_refs` entry against
  the episode manifest presented to the reflector. Uncited or foreign
  evidence → rejected at intake, with the rejection reported.
- **Rewrite or delete history.** No API path mutates `evidence_records`.

## The components

### Workspace (`core/workspace.rs`)

The explicit, inspectable context per turn. Domains carry typed attribution —
`CurrentInput`, `CurrentContinuityState`, `RelationshipMemory`,
`EpisodicMemory`, `LibraryEvidence`, `ExternalResourceResult`,
`RecalledEvidence`, `SelfMemory`, `ActivePolicy` — with deterministic ordering,
fixed budgets, and the rule that current continuity and current input are
never budgeted away. External material can appear in the workspace but its
authority class prevents it from becoming self state.

### Persona envelope (`core/persona.rs`)

What the backend receives: conversation history (raw interaction records),
persona seed (configured disposition — not retrieved memory), durable self,
memory sections, recalled evidence, active policy, observed runtime, and the
reserved body/interoception slots for later Body/G1 wiring. `with_seed`
preserves the distinction between *who the individual is configured to be* and
*what it has experienced*.

### Derived lane (`core/c0.rs`, `store-sqlite/c0.rs`, migration 0005)

Tables:

- `c0_proposals` — id, kind (`policy_update | self_update | retrieval_update |
  memory_note`), target + optional key, old/proposed values, evidence refs,
  confidence, status (`pending | accepted | rejected | superseded`),
  rejection reason, deciding reflection id, activation seq.
- `c0_activations` — seq, activation id, proposal id, predecessor seq, full
  `OperativeView` snapshot (params + self model JSON), `rolled_back_at`.
- `c0_head` — current activation seq per individual (0 = documented defaults).
- `c0_evaluations` — scope (`turn` or `replay`), subject, turn/proposal
  correlation, metrics JSON, evaluator identity.

Each activation stores a **complete snapshot** of the operative view, so
rollback is a head move with no recomputation and no dependence on what
changed since.

### Reflection (`runtime/c0/reflection.rs`, `core/c0.rs` Reflector trait)

A reflection cycle gathers recent episodes (utterances, corrections, system
events — reflections excluded so a cycle cannot cite itself), memory records,
recent turn metrics and pending proposals, then asks the configured
`Reflector`. Output is schema-validated, citation-checked against the episode
manifest, and split: canonical drafts go to the kernel; improvement drafts
become pending `c0_proposals`. The cycle is narrated as `EvidenceKind::
Reflection` with `DerivedFrom` links to the episodes it cited.

`ReflectorBackend`: `fake` (deterministic fixture), `openai-compatible` (HTTP
endpoint with its own provider entry), or `mirror` (reuse the persona
backend). The report records whether evaluator and actor were the same
component — "the model evaluated itself" is a materially different claim than
"a separate evaluator did", and the manifest says which happened.

### Evaluation (`runtime/c0/eval.rs`)

Deterministic per-turn metrics — no second model required: response length,
boundedness, whether memories were surfaced, whether retrieval missed, input
echo share, evidence-backed proposal ratio. Written to `c0_evaluations` with
scope `turn` every turn; the reflector reads them as measurements.

### Replay gate (`runtime/c0/replay.rs`, `core/c0.rs` gate_decision)

Before an improvement proposal can activate, the runtime replays recent user
inputs twice — once under the baseline view, once under the candidate view the
proposal would produce — through the real workspace assembly and the real
persona. Metrics are compared per turn. The gate accepts only when every
replayed turn completed *and* no axis regressed *and* at least one axis
changed measurably ("candidate produced no measurable change" is a reject, so
no-op activations never move the head). A replay writes nothing: no sessions,
no turns, no evidence — it is an evaluation, not a commitment.

### Writer authority is lazy

Ordinary conversation never claims a writer epoch and never moves the
continuity head — a turn is observation + response + drafts. `ensure_writer`
claims the epoch the first time a draft is actually submitted, inside the turn
that produced it. Read-only sessions stay read-only in canonical terms.

## CLI surface

```text
kamimusuhi-runtime init      --dir <path> [--seed <n>]
kamimusuhi-runtime chat      --dir <path> --subject <id> --persona fake|openai-compatible ...
                             [--debug-context]
kamimusuhi-runtime talk      --dir <path> --message <text> ...
kamimusuhi-runtime reflect   --dir <path> [--subject <id>]
kamimusuhi-runtime activate  --dir <path> --proposal-id <hex> [--subject <id>]
kamimusuhi-runtime rollback  --dir <path>
kamimusuhi-runtime inspect   --dir <path>
```

Inside `chat`: `/quit` `/status` `/memory` `/reflect` `/rollback` `/context`
`/help`. `/context` and `--debug-context` expose the assembled workspace — the
derived context exactly as the model received it.

## What C0 deliberately does not claim

- No autonomous source-code rewriting, no self-directed tool use, no
  capability expansion beyond the closed proposal vocabulary.
- The persona prior is thin and configured; the *self model* is derived state
  built only through gated proposals.
- Reflection quality depends on the reflector; the fixtures prove the
  mechanism, not model competence.
- Streaming exists as a provider-level fallback (single emission); true
  incremental output is an adapter extension, not yet a runtime feature.

Future phases (C1 autobiographical memory, C2 initiative/background cognition,
G1 graft state, V0 speech, E0 evaluated mutation) plug into the slots this
document reserves rather than replacing them.
