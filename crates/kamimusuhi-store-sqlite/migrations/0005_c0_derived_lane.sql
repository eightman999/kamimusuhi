-- Schema v5: the C0 derived operative lane.
--
-- Canonical state (evidence, commits, state_records) stays strictly
-- forward-only. This lane holds the *operative view* a conversation runs
-- with — conversation policy, retrieval parameters and the self model — plus
-- the improvement proposals that may change it. Its semantics differ from the
-- canonical lane on purpose:
--
--   * proposals pass through a staged lifecycle (pending -> accepted /
--     rejected / quarantined / superseded), because a model's suggestion is
--     never itself a state change;
--   * an activation stores a full snapshot of the operative view it produced;
--   * a movable head selects which activation is in force, so a rollback is a
--     head move, never a rewrite. Rolled-back history is kept.
--
-- Every lifecycle transition is also narrated into the canonical evidence
-- log by the runtime, so the immutable record always says what happened even
-- though it does not own this lane.

CREATE TABLE c0_proposals (
    proposal_id         TEXT PRIMARY KEY,
    individual_id       TEXT NOT NULL REFERENCES individuals(individual_id),
    -- The improvement lane this proposal belongs to. Memory and relationship
    -- improvements are canonical mutations and live in mutation_proposals;
    -- this table holds only operative-lane proposals.
    proposal_type       TEXT NOT NULL
                        CHECK (proposal_type IN ('policy_update', 'self_update', 'retrieval_update')),
    -- Fixed-vocabulary target: a parameter key or a self-model field.
    target              TEXT NOT NULL,
    -- Entries inside a list-like self field.
    target_key          TEXT,
    -- The value in force when the proposal was drafted; NULL = unset.
    old_value_json      TEXT,
    proposed_value_json TEXT NOT NULL,
    evidence_refs_json  TEXT NOT NULL,
    expected_effect     TEXT,
    risk                TEXT,
    -- Schema-checked at intake to be within [0,1]; no row outside it exists.
    confidence          REAL,
    status              TEXT NOT NULL
                        CHECK (status IN ('pending', 'accepted', 'rejected', 'quarantined', 'superseded')),
    rejection_reason    TEXT,
    -- Evidence id of the reflection record that produced this proposal.
    reflection_id       TEXT REFERENCES evidence_records(evidence_id),
    created_at          INTEGER NOT NULL,
    decided_at          INTEGER,
    -- The activation that applied it; set on acceptance, never unset.
    activation_seq      INTEGER
) STRICT;

CREATE INDEX c0_proposals_by_status ON c0_proposals(individual_id, status);
CREATE INDEX c0_proposals_by_target ON c0_proposals(individual_id, target);

-- One row per applied proposal: an immutable point in the derived lane's
-- history carrying the full operative view it produced.
CREATE TABLE c0_activations (
    activation_seq   INTEGER NOT NULL,
    activation_id    TEXT NOT NULL,
    individual_id    TEXT NOT NULL REFERENCES individuals(individual_id),
    proposal_id      TEXT NOT NULL REFERENCES c0_proposals(proposal_id),
    -- The head this activation built on. After a rollback a later activation
    -- branches off the restored head; dormant branches stay inspectable.
    predecessor_seq  INTEGER NOT NULL,
    -- Full operative-view snapshots after applying the proposal.
    params_json      TEXT NOT NULL,
    self_json        TEXT NOT NULL,
    -- Set when a rollback moved the head back across this activation.
    rolled_back_at   INTEGER,
    created_at       INTEGER NOT NULL,
    PRIMARY KEY (individual_id, activation_seq),
    UNIQUE (individual_id, proposal_id)
) STRICT;

-- The movable head: which activation's snapshot is operative. Seq 0 means
-- defaults — no activation has ever been applied.
CREATE TABLE c0_head (
    individual_id  TEXT PRIMARY KEY REFERENCES individuals(individual_id),
    activation_seq INTEGER NOT NULL CHECK (activation_seq >= 0),
    updated_at     INTEGER NOT NULL
) STRICT;

-- Deterministic evaluation output. `evaluator` names what produced the
-- metrics: the deterministic evaluator's version string, or a backend
-- descriptor when a model is used — the manifest never hides which.
CREATE TABLE c0_evaluations (
    evaluation_id TEXT PRIMARY KEY,
    individual_id TEXT NOT NULL REFERENCES individuals(individual_id),
    scope         TEXT NOT NULL CHECK (scope IN ('turn', 'replay', 'reflection')),
    subject_key   TEXT,
    turn_id       TEXT REFERENCES turns(turn_id),
    proposal_id   TEXT REFERENCES c0_proposals(proposal_id),
    metrics_json  TEXT NOT NULL,
    evaluator     TEXT NOT NULL,
    created_at    INTEGER NOT NULL
) STRICT;

CREATE INDEX c0_evaluations_by_individual ON c0_evaluations(individual_id, created_at);
CREATE INDEX c0_evaluations_by_proposal ON c0_evaluations(proposal_id);
