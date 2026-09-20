# C0 — Chat & Self-Improvement Foundation

Status: **implemented and verified locally** (`scripts/ci-local.sh` green).
This report covers the C0 exit criteria; the architecture document is
`docs/architecture/conversational-organism.md`.

Claimed at C0:

> Kamimusuhi は会話可能で、その会話を自分の経験として保持し、自分の振る舞いを
> 評価して、監査・検証・rollback可能な形で次の振る舞いを変更できる。

Not claimed: autonomous source-code rewriting or unbounded self-improvement.

## Architecture

Two lanes over one store:

- **Canonical lane** (existing): `evidence_records` + `evidence_links`
  (append-only), `state_records` via `mutation_proposals` → `MutationPolicyV0`
  → kernel activation, `audit_events`. Conversation turns append
  `user_utterance` / `agent_utterance` / `system_event` evidence; the
  continuity head only moves when a canonical draft activates.
- **Derived lane** (new, migration `0005`): `c0_proposals`,
  `c0_activations` (full `OperativeView` snapshots), `c0_head` (movable),
  `c0_evaluations`. Rollback moves this head only.

Per turn: workspace assembly (history + retrieved memory + self model +
operative params + recalled evidence) → persona turn → drafts submitted to the
kernel (lazy writer claim) → response emitted → deterministic turn metrics
recorded. Reflection (`/reflect`, `reflect` command, or
`c0.reflection_interval_turns` cadence) → cited/checked drafts → pending
proposals → write-free replay → gate → activation.

## Storage schema (migration 0005)

```text
c0_proposals(proposal_id PK, individual_id FK, proposal_type,
             target, target_key, old_value_json, proposed_value_json,
             evidence_refs_json, expected_effect, risk, confidence,
             status CHECK pending|accepted|rejected|superseded,
             rejection_reason, reflection_id, created_at, decided_at,
             activation_seq)
c0_activations(activation_seq, activation_id, individual_id FK,
               proposal_id, predecessor_seq, params_json, self_json,
               rolled_back_at, created_at)  PK (individual_id, activation_seq)
c0_head(individual_id PK, activation_seq, updated_at)   -- absent row = 0
c0_evaluations(evaluation_id PK, individual_id FK, scope, subject_key,
               turn_id, proposal_id, metrics_json, evaluator, created_at)
```

## Provider / model used

| Component | In CI/tests | Real endpoint |
|-----------|-------------|---------------|
| Persona   | `FakePersonaCore` fixture | `--persona openai-compatible --persona-url <url> --persona-model <m>` |
| Reflector | `FakeReflector` fixture (distinct component → `evaluator_distinct_from_actor: true`) | `c0.reflector: openai-compatible` or `mirror` |

The OpenAI-compatible path is implemented (`kamimusuhi-persona-http`,
including a `Reflector` impl with one schema-validation retry) and exercised
against `FixtureServer` sockets in `persona_boundary.rs`/`text_dialogue.rs`;
no live model endpoint was used for this report, so nothing here claims model
competence — only that the mechanism is correct and provider-swappable
(`swapping_the_persona_backend_preserves_identity_and_history`).

## Memory retrieval example

Session A (process 1): `私はほうじ茶が好き。覚えておいて` → persona drafts
`relationship.fact` → kernel activates → `state_records` row
`{"preference": "ほうじ茶"}` citing the input evidence id.

Session B (new process): `私の好きなお茶は何だっけ？` → workspace envelope
shows `RELATIONSHIP_MEMORY=1` and `/context` contains the record under
`relationship_memories`. Test: `c01_a_new_process_recalls_what_another_session_learned`.

Retrieval is character-bigram lexical scoring over a bounded pool
(`retrieval.evidence_pool`, `episodic_top_k`, `min_score`, …), fully
deterministic for a given store state (`retrieval_is_deterministic_for_the_same_state`).

## Self-model example

After one reflection cycle (fixture reflector, real run):

```json
"self_model": {"fields": {"capabilities": [{
  "key": "relationship_recall",
  "value": "retains and recalls subject-scoped facts across sessions",
  "confidence": 0.6,
  "evidence_refs": ["a2940d6f0d7348c8bafbb456db003027"],
  "proposal_id": "538fdd3f828d46d8a08d5f8342d9281a"
}]}}
```

The entry cites only the individual's *own* utterance evidence — a
self-model claim grounded in first-party experience, written by an accepted
proposal, never by the model directly.

## Reflection example (real run)

`chat` input `… /reflect …` produced:

```json
{
  "reflector_backend": "fixture:fake-reflector",
  "evaluator_distinct_from_actor": true,
  "episodes_presented": 6,
  "corrections_linked": 0,
  "proposals_created": [
    {"kind": "retrieval_update", "target": "retrieval.evidence_top_k",
     "proposed_value": 6, "status": "pending"},
    {"kind": "self_update", "target": "capabilities",
     "target_key": "relationship_recall", "status": "pending"}
  ],
  "gate_results": [
    {"gate": "reject", "proposal_id": "df259f3c…",
     "reason": "candidate produced no measurable change; nothing to activate"},
    {"gate": "accept", "proposal_id": "538fdd3f…", "activation_seq": 1}
  ],
  "activations": [1]
}
```

The cycle is narrated as a canonical `reflection` evidence record with
`derived_from` links to the episodes it cited.

## Improvement proposal → evaluation → replay

Per-turn metrics recorded (scope `turn`):

```json
{"memories_surfaced": 0, "retrieval_missed": true, "response_chars": 98,
 "repetition_ratio": 0.0, "unsupported_marker_count": 0}
```

The replay gate ran baseline vs candidate over the last user inputs
(scope `replay`, recorded in `c0_evaluations`):

- `retrieval_update` proposal: all turns completed, every axis delta 0 →
  **rejected** ("no measurable change" — a no-op must not move the head).
- `self_update` proposal: `repetition_ratio` delta −0.5 (the candidate
  workspace carried the self-state section, changing the generated response) →
  **accepted**, `activation_seq = 1`.

Test `a_replay_writes_nothing` asserts a replay adds zero evidence rows and
zero sessions.

## Before / after

After activation, `turn` assembles under the new view: the next `/context`
shows `self_state` containing `relationship_recall`. Test
`c03_an_accepted_proposal_changes_the_next_workspace` asserts both the store
state and the workspace-visible effect.

## Rollback test

`/rollback` on the demo runtime:

```json
{"rolled_back_seq": 1, "restored_seq": 0,
 "proposal_id": "538fdd3f828d46d8a08d5f8342d9281a"}
```

`c0_activations` keeps the row with `rolled_back_at` set; `c0_head` returns to
0; canonical evidence count strictly *increased* (the rollback narration).
Test: `c04_rollback_moves_the_derived_head_and_keeps_canonical_history`.

## Required invariants — test coverage

| Spec requirement | Test |
|---|---|
| canonical events immutable | `c04_…` (evidence count grows, never shrinks); no mutation API exists |
| event IDs unique, timestamps ordered | `canonical_evidence_ids_are_unique_and_monotonic` |
| session restart persistence | `c01_…` |
| subject isolation | `subjects_do_not_share_retained_memory` |
| memory provenance | `c02_…` (evidence_refs + Corrects links) |
| correction handling | `c02_correction_supersedes_the_belief_not_the_event` |
| proposal cannot mutate state directly | `a_proposal_row_is_not_state_and_a_rejection_is_not_an_effect` |
| rejected proposal → zero effect | same test + gate rejection path |
| accepted proposal affects next workspace | `c03_…` |
| rollback restores previous active state | `c04_…` |
| provider swap preserves identity/history | `swapping_the_persona_backend_preserves_identity_and_history` |
| malformed model JSON cannot corrupt state | `malformed_improvement_drafts_are_rejected_at_the_boundary`; reflector schema-validation path; `persona_boundary.rs` malformed-response tests |
| API secrets never persisted | `persona_boundary.rs` (`…secret_bearing_provider_replies…`, endpoint-inheritance test) — `auth_env` stores only the variable *name* |
| deterministic retrieval | `retrieval_is_deterministic_for_the_same_state` |

## Known limitations

- **Fixtures prove mechanism, not competence.** Real-model behaviour under a
  live endpoint is unwired in CI; the HTTP reflector/persona paths are
  socket-tested against `FixtureServer` only.
- **Lexical retrieval is shallow.** Bigram overlap is deterministic and
  language-agnostic but has no semantics; embedding retrieval is a later
  phase. `retrieval.*` params are the first honest knobs.
- **Evaluation metrics are heuristic.** Deterministic signals (length,
  boundedness, surfaced-memory, retrieval misses, repetition) — no
  semantic judge. `evaluator` identity is recorded per row so a future model
  evaluator stays attributable.
- **Replay cost.** Each gated proposal replays `c0.replay_turns` inputs ×2
  through the persona; with an HTTP backend that is real latency. Bounded,
  but not free.
- **No streaming yet** — provider abstraction returns a single emission;
  incremental output is an adapter extension.
- **Rollback granularity is one activation per call**; multi-step rollback is
  repeated `/rollback`, not a depth parameter.
- **Reflection cadence** is turn-count based; no salience-triggered or
  idle-time reflection (C2 territory).

## Exact commands

```bash
# build + full local gate (fmt, clippy -D warnings, all tests, dep boundary)
./scripts/ci-local.sh

# C0 lane tests only
cargo test -p kamimusuhi-runtime --test c0_lane

# interactive demo against the deterministic fixture
cargo run -p kamimusuhi-runtime -- init --dir /tmp/c0demo --seed 1
cargo run -p kamimusuhi-runtime -- chat --dir /tmp/c0demo \
    --subject user-fixture --persona fake
#   inside:  /status /memory /reflect /rollback /context /quit

# standalone lane commands
kamimusuhi-runtime reflect  --dir /tmp/c0demo --subject user-fixture
kamimusuhi-runtime activate --dir /tmp/c0demo --proposal-id <hex>
kamimusuhi-runtime rollback --dir /tmp/c0demo
kamimusuhi-runtime inspect  --dir /tmp/c0demo        # includes "c0" section
kamimusuhi-runtime talk     --dir /tmp/c0demo --message "…" --debug-context
```

## Commit

This changeset lands on `master` base `a6a599d`, branch
`devin/c0-conversational-organism`. The report is part of the commit it
describes.
