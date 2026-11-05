-- Schema v6: recall v0 usage bookkeeping.
--
-- `last_used_at`/`use_count` are mutable bookkeeping, not canonical record
-- content, so they live in this side table and never touch `state_records`
-- rows. Times are unix milliseconds, like every other timestamp column.
--
-- Never edit this file after it has shipped; add a new migration instead.

CREATE TABLE IF NOT EXISTS state_record_usage (
    state_record_id TEXT PRIMARY KEY REFERENCES state_records(state_record_id),
    last_used_at    INTEGER NOT NULL,
    use_count       INTEGER NOT NULL DEFAULT 0
) STRICT;
