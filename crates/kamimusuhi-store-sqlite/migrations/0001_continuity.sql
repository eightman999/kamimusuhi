-- Schema v1: canonical continuity domain.
--
-- Identifiers are 32-char lowercase hex runtime IDs (TEXT). Timestamps are UTC
-- unix milliseconds (INTEGER). Typed payloads are JSON text; SQL columns hold
-- only what needs indexing or constraint enforcement.
--
-- Never edit this file after it has shipped; add a new migration instead.

CREATE TABLE schema_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;

CREATE TABLE individuals (
    individual_id        TEXT PRIMARY KEY,
    root_commit_id       TEXT NOT NULL UNIQUE
                         REFERENCES canonical_commits(commit_id) DEFERRABLE INITIALLY DEFERRED,
    current_writer_epoch INTEGER NOT NULL CHECK (current_writer_epoch >= 1),
    created_at           INTEGER NOT NULL
) STRICT;

-- Single-writer fencing: the newest epoch is the only one allowed to activate.
CREATE TABLE writer_epochs (
    individual_id TEXT NOT NULL REFERENCES individuals(individual_id),
    writer_epoch  INTEGER NOT NULL CHECK (writer_epoch >= 1),
    node_id       TEXT NOT NULL,
    boot_id       TEXT NOT NULL,
    claimed_at    INTEGER NOT NULL,
    PRIMARY KEY (individual_id, writer_epoch)
) STRICT;

CREATE TABLE mutation_proposals (
    proposal_id         TEXT PRIMARY KEY,
    individual_id       TEXT NOT NULL REFERENCES individuals(individual_id),
    domain              TEXT NOT NULL,
    operation           TEXT NOT NULL,
    subject_key         TEXT,
    candidate_json      TEXT NOT NULL,
    expected_commit_id  TEXT NOT NULL,
    expected_generation INTEGER NOT NULL,
    evidence_refs_json  TEXT NOT NULL,
    origin_class        TEXT NOT NULL,
    requested_by_json   TEXT NOT NULL,
    writer_epoch        INTEGER NOT NULL,
    policy_version      INTEGER NOT NULL,
    idempotency_key     TEXT NOT NULL,
    payload_fingerprint TEXT NOT NULL,
    created_at          INTEGER NOT NULL,
    recorded_at         INTEGER NOT NULL
) STRICT;

CREATE INDEX mutation_proposals_by_idempotency
    ON mutation_proposals(individual_id, idempotency_key);

CREATE TABLE canonical_commits (
    commit_id             TEXT PRIMARY KEY,
    individual_id         TEXT NOT NULL REFERENCES individuals(individual_id),
    generation            INTEGER NOT NULL CHECK (generation >= 0),
    predecessor_commit_id TEXT REFERENCES canonical_commits(commit_id),
    proposal_id           TEXT UNIQUE REFERENCES mutation_proposals(proposal_id),
    created_at            INTEGER NOT NULL,
    UNIQUE (individual_id, generation),
    -- Only the root (generation 0) has no predecessor and no proposal.
    CHECK ((generation = 0) = (predecessor_commit_id IS NULL)),
    CHECK ((generation = 0) = (proposal_id IS NULL))
) STRICT;

CREATE TABLE continuity_heads (
    individual_id TEXT PRIMARY KEY REFERENCES individuals(individual_id),
    commit_id     TEXT NOT NULL UNIQUE REFERENCES canonical_commits(commit_id),
    generation    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
) STRICT;

CREATE TABLE mutation_decisions (
    proposal_id    TEXT PRIMARY KEY REFERENCES mutation_proposals(proposal_id),
    disposition    TEXT NOT NULL CHECK (disposition IN ('accept', 'reject', 'quarantine')),
    reason_code    TEXT NOT NULL,
    detail         TEXT,
    policy_version INTEGER NOT NULL,
    decided_at     INTEGER NOT NULL
) STRICT;

CREATE TABLE activation_receipts (
    receipt_id            TEXT PRIMARY KEY,
    individual_id         TEXT NOT NULL REFERENCES individuals(individual_id),
    proposal_id           TEXT NOT NULL UNIQUE REFERENCES mutation_proposals(proposal_id),
    commit_id             TEXT NOT NULL UNIQUE REFERENCES canonical_commits(commit_id),
    predecessor_commit_id TEXT NOT NULL REFERENCES canonical_commits(commit_id),
    generation            INTEGER NOT NULL,
    idempotency_key       TEXT NOT NULL,
    created_at            INTEGER NOT NULL,
    -- One activation per logical mutation.
    UNIQUE (individual_id, idempotency_key)
) STRICT;

-- Durable audit: written inside the same transaction as the transition.
CREATE TABLE audit_events (
    audit_seq      INTEGER PRIMARY KEY AUTOINCREMENT,
    audit_event_id TEXT NOT NULL UNIQUE,
    individual_id  TEXT NOT NULL REFERENCES individuals(individual_id),
    kind           TEXT NOT NULL,
    commit_id      TEXT,
    proposal_id    TEXT,
    payload_json   TEXT NOT NULL,
    created_at     INTEGER NOT NULL
) STRICT;

CREATE INDEX audit_events_by_individual ON audit_events(individual_id, audit_seq);
