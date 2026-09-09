-- Schema v4: resource-call correlation and retry accounting.
--
-- W4 could only tie a resource call to a turn through the operational trace,
-- which is observability and may be rotated away. A call is attribution and
-- has to survive on its own, so the correlation now lives in the row.
--
-- `attempts` records how many physical tries produced this one logical call.
-- Retrying is the adapter's business: a retried call stays one row, because
-- otherwise one question would look like several independent answers.
--
-- `latency_ms` is measured monotonically, not as completed_at - started_at:
-- wall time can step backwards mid-call and a duration must not.
--
-- Still no column here can hold a credential, a request body or a response
-- body. That remains a schema property, not a convention.
--
-- Never edit this file after it has shipped; add a new migration instead.

ALTER TABLE resource_calls ADD COLUMN turn_id TEXT REFERENCES turns(turn_id);

ALTER TABLE resource_calls ADD COLUMN attempts INTEGER NOT NULL DEFAULT 1;

ALTER TABLE resource_calls ADD COLUMN latency_ms INTEGER NOT NULL DEFAULT 0;

CREATE INDEX resource_calls_by_turn ON resource_calls(turn_id, started_at);
