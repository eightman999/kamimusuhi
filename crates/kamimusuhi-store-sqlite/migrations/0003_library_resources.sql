-- Schema v3: external Library material and cognitive-resource attribution.
--
-- Neither family is canonical self state:
--   * library_artifacts / library_chunks hold documents Kamimusuhi can read.
--     They carry no individual_id and no foreign key into self state, so
--     imported text cannot become a belief by association (plan §6.4).
--   * resource_calls record where external cognitive material came from. A
--     call is attribution, not a lineage event, and never moves the head.
--
-- Never edit this file after it has shipped; add a new migration instead.

CREATE TABLE library_artifacts (
    artifact_id     TEXT PRIMARY KEY,
    source_uri      TEXT,
    title           TEXT,
    media_type      TEXT NOT NULL CHECK (media_type IN ('text/plain', 'text/markdown')),
    -- Integrity/dedup aid over the imported text. Never the identity: the
    -- same document imported from two sources is two artifacts.
    content_digest  TEXT NOT NULL,
    chunker_version INTEGER NOT NULL CHECK (chunker_version >= 1),
    chunk_count     INTEGER NOT NULL CHECK (chunk_count >= 0),
    imported_at     INTEGER NOT NULL
) STRICT;

CREATE INDEX library_artifacts_by_digest ON library_artifacts(content_digest);

CREATE TABLE library_chunks (
    chunk_id    TEXT PRIMARY KEY,
    artifact_id TEXT NOT NULL REFERENCES library_artifacts(artifact_id),
    -- (artifact_id, ordinal) is the citable location of a span.
    ordinal     INTEGER NOT NULL CHECK (ordinal >= 0),
    heading     TEXT,
    char_offset INTEGER NOT NULL CHECK (char_offset >= 0),
    text        TEXT NOT NULL,
    UNIQUE (artifact_id, ordinal)
) STRICT;

CREATE TABLE resource_calls (
    resource_call_id TEXT PRIMARY KEY,
    -- The resource that actually answered, not the slot that was asked.
    resource_id      TEXT NOT NULL,
    slot             TEXT NOT NULL,
    individual_id    TEXT NOT NULL REFERENCES individuals(individual_id),
    adapter          TEXT NOT NULL,
    purpose          TEXT NOT NULL,
    -- Digests only. No request body, no result body, no credential: there is
    -- deliberately no column here that could hold one.
    request_digest   TEXT NOT NULL,
    outcome          TEXT NOT NULL CHECK (outcome IN ('ok', 'error')),
    result_digest    TEXT,
    error_code       TEXT,
    started_at       INTEGER NOT NULL,
    completed_at     INTEGER NOT NULL,
    CHECK ((outcome = 'ok') = (result_digest IS NOT NULL)),
    CHECK ((outcome = 'error') = (error_code IS NOT NULL))
) STRICT;

CREATE INDEX resource_calls_by_individual ON resource_calls(individual_id, started_at);
CREATE INDEX resource_calls_by_resource ON resource_calls(resource_id, started_at);
