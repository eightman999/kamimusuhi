-- Schema v2: canonical interaction evidence and durable derived state.
--
-- Two separate record families, on purpose:
--   * evidence_records  — what reached the runtime, append-oriented.
--   * state_records     — what the individual holds, produced only by an
--                         activated canonical commit.
-- A raw utterance and the relationship fact derived from it are therefore
-- always different rows in different tables (plan §6.2, §6.3).
--
-- Never edit this file after it has shipped; add a new migration instead.

CREATE TABLE sessions (
    session_id    TEXT PRIMARY KEY,
    individual_id TEXT NOT NULL REFERENCES individuals(individual_id),
    started_at    INTEGER NOT NULL,
    ended_at      INTEGER
) STRICT;

CREATE INDEX sessions_by_individual ON sessions(individual_id, started_at);

CREATE TABLE turns (
    turn_id       TEXT PRIMARY KEY,
    session_id    TEXT NOT NULL REFERENCES sessions(session_id),
    individual_id TEXT NOT NULL REFERENCES individuals(individual_id),
    -- The sender's ordering inside the session, not the canonical commit
    -- order: a late-arriving turn keeps its original position.
    sequence      INTEGER NOT NULL CHECK (sequence >= 0),
    started_at    INTEGER NOT NULL,
    UNIQUE (session_id, sequence)
) STRICT;

CREATE TABLE evidence_records (
    evidence_id     TEXT PRIMARY KEY,
    individual_id   TEXT NOT NULL REFERENCES individuals(individual_id),
    session_id      TEXT REFERENCES sessions(session_id),
    turn_id         TEXT REFERENCES turns(turn_id),
    kind            TEXT NOT NULL,
    origin_class    TEXT NOT NULL,
    payload_json    TEXT NOT NULL,
    source_id       TEXT,
    source_sequence INTEGER,
    -- Integrity/dedup aid. Not anonymisation, not a deletion substitute.
    content_digest  TEXT,
    retention_class TEXT NOT NULL,
    created_at      INTEGER NOT NULL
) STRICT;

CREATE INDEX evidence_by_individual ON evidence_records(individual_id, created_at);
CREATE INDEX evidence_by_turn ON evidence_records(turn_id);

-- Lineage. Declared by the newer record about the record it depends on or
-- corrects. Append-only: a correction never rewrites what it corrects.
CREATE TABLE evidence_links (
    from_evidence_id TEXT NOT NULL REFERENCES evidence_records(evidence_id),
    to_evidence_id   TEXT NOT NULL REFERENCES evidence_records(evidence_id),
    relation         TEXT NOT NULL
                     CHECK (relation IN ('derived_from', 'corrects', 'duplicates')),
    created_at       INTEGER NOT NULL,
    PRIMARY KEY (from_evidence_id, to_evidence_id, relation),
    CHECK (from_evidence_id <> to_evidence_id)
) STRICT;

CREATE INDEX evidence_links_by_target ON evidence_links(to_evidence_id, relation);

CREATE TABLE state_records (
    state_record_id             TEXT PRIMARY KEY,
    individual_id               TEXT NOT NULL REFERENCES individuals(individual_id),
    domain                      TEXT NOT NULL CHECK (domain IN ('episodic', 'relationship', 'self')),
    subject_key                 TEXT,
    kind                        TEXT NOT NULL,
    payload_json                TEXT NOT NULL,
    lifecycle_state             TEXT NOT NULL
                                CHECK (lifecycle_state IN ('active', 'superseded', 'invalidated')),
    -- The commit is written in the same transaction, hence DEFERRED.
    created_commit_id           TEXT NOT NULL
                                REFERENCES canonical_commits(commit_id) DEFERRABLE INITIALLY DEFERRED,
    supersedes_state_record_id  TEXT REFERENCES state_records(state_record_id),
    superseded_by_state_record_id TEXT REFERENCES state_records(state_record_id),
    created_at                  INTEGER NOT NULL,
    -- A relationship record is always about someone.
    CHECK (domain <> 'relationship' OR subject_key IS NOT NULL)
) STRICT;

CREATE INDEX state_records_current
    ON state_records(individual_id, domain, subject_key, lifecycle_state);
CREATE INDEX state_records_by_commit ON state_records(created_commit_id);

CREATE TABLE state_record_evidence (
    state_record_id TEXT NOT NULL REFERENCES state_records(state_record_id),
    evidence_id     TEXT NOT NULL REFERENCES evidence_records(evidence_id),
    PRIMARY KEY (state_record_id, evidence_id)
) STRICT;

CREATE INDEX state_record_evidence_by_evidence ON state_record_evidence(evidence_id);

-- A correction names the record it replaces on the proposal itself, so the
-- supersession is auditable from the proposal, not only from the result.
ALTER TABLE mutation_proposals ADD COLUMN supersedes_state_record_id TEXT;
