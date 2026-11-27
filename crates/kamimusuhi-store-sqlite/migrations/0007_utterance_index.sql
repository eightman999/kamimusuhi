-- Schema v7: a derived full-text index over canonical utterances.
--
-- Nothing here is canonical. Every row is recomputed from `evidence_records`
-- and may be dropped and rebuilt at any time; the evidence stays the only
-- record of what was said. The index exists so an utterance older than the
-- bounded recent pool can still be offered to the relevance judge.
--
-- `utterance_index` is a contentless FTS5 table whose only column holds the
-- utterance text as space-separated character bigrams. It stores no utterance
-- text of its own.
--
-- `utterance_index_map` ties each FTS row to its evidence record. The FTS
-- rowid is an explicit INTEGER PRIMARY KEY here on purpose: `evidence_records`
-- has only an implicit rowid, which VACUUM is free to renumber, so the index
-- must never be keyed on it. A map row without an FTS row marks an utterance
-- that was examined and had nothing to index.
--
-- The partial index keeps the "what is not indexed yet" scan off the evidence
-- payloads.
--
-- Never edit this file after it has shipped; add a new migration instead.

CREATE VIRTUAL TABLE IF NOT EXISTS utterance_index USING fts5(
    grams,
    content = '',
    contentless_delete = 1,
    tokenize = 'unicode61 remove_diacritics 0'
);

CREATE VIRTUAL TABLE IF NOT EXISTS utterance_index_vocab
    USING fts5vocab('utterance_index', 'row');

CREATE TABLE IF NOT EXISTS utterance_index_map (
    fts_rowid   INTEGER PRIMARY KEY,
    evidence_id TEXT NOT NULL UNIQUE REFERENCES evidence_records(evidence_id)
) STRICT;

CREATE INDEX IF NOT EXISTS evidence_indexable_utterances
    ON evidence_records(created_at, evidence_id)
    WHERE kind IN ('user_utterance', 'agent_utterance')
      AND retention_class = 'standard';
