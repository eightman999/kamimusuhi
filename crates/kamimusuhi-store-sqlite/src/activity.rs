//! Read-only memory-growth activity: how much conversation has accumulated
//! since canonical memory (a `state_records` row) last grew.
//!
//! Every query here is a `SELECT` over existing tables. Nothing is written,
//! no schema is added, and the answer is derived on demand, so the signal
//! built from it can never disagree with canonical state.
//!
//! "Since" is measured on `created_at` (unix milliseconds, the store clock).
//! The boundary is strict: a record created in the same millisecond as the
//! newest state record counts as *before* it. Within a dialogue turn the
//! user utterance is appended before any draft it causes is activated, so
//! the utterance that produced a memory is never counted against it.

use std::collections::BTreeMap;

use kamimusuhi_core::continuity::ContinuityError;
use kamimusuhi_core::ids::IndividualId;
use kamimusuhi_core::time::UtcTimestamp;
use rusqlite::{OptionalExtension, params};

use crate::error::map_sqlite;
use crate::store::SqliteStore;

/// Raw counts behind the memory-stall signal. No content, only counts and
/// timestamps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryActivity {
    pub individual_created_at: UtcTimestamp,
    /// All state records ever created (any lifecycle state).
    pub state_records_total: u64,
    /// Newest `state_records.created_at`; `None` when memory never grew.
    pub last_state_record_at: Option<UtcTimestamp>,
    /// The point counts are measured from: the newest state record, or the
    /// individual's creation when there is none.
    pub baseline_at: UtcTimestamp,
    /// `user_utterance` evidence with `retention_class = 'standard'` created
    /// after the baseline, all sources. Only these count toward a stall.
    pub user_utterances_since: u64,
    /// Newest of those standard user utterances.
    pub last_user_utterance_at: Option<UtcTimestamp>,
    /// `user_utterance` evidence with `retention_class = 'ephemeral'` (e.g.
    /// non-operator speech in a public channel) after the baseline. Such
    /// speech is not meant to become memory, so it is reported for
    /// reference and never counted toward a stall.
    pub ephemeral_user_utterances_since: u64,
    /// `reflection` evidence (one per completed reflection cycle) after the
    /// baseline.
    pub reflections_since: u64,
    pub last_reflection_at: Option<UtcTimestamp>,
    /// Drafts the reflection cycles themselves refused before intake
    /// (`drafts_rejected` entries in their narration).
    pub reflection_drafts_rejected_since: u64,
    /// Canonical mutation proposals recorded after the baseline.
    pub memory_proposals_since: u64,
    /// Decisions on those proposals: `accept` / `reject` / `quarantine`.
    pub memory_decisions_since: BTreeMap<String, u64>,
    /// Operative-lane (C0) proposals created after the baseline, by status.
    pub c0_proposals_since: BTreeMap<String, u64>,
}

fn count(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

impl SqliteStore {
    /// Collect [`MemoryActivity`] for one individual. `Ok(None)` when the
    /// individual does not exist. One read transaction, `SELECT`s only.
    pub fn memory_activity(
        &self,
        individual_id: IndividualId,
    ) -> Result<Option<MemoryActivity>, ContinuityError> {
        let conn = self.conn()?;
        // A deferred transaction gives every count the same snapshot; it is
        // dropped (rolled back) without ever writing.
        let tx = conn.unchecked_transaction().map_err(map_sqlite)?;
        let id = individual_id.to_string();
        let Some(created) = tx
            .query_row(
                "SELECT created_at FROM individuals WHERE individual_id = ?1",
                params![id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(map_sqlite)?
        else {
            return Ok(None);
        };
        let (state_total, last_state): (i64, Option<i64>) = tx
            .query_row(
                "SELECT COUNT(*), MAX(created_at) FROM state_records WHERE individual_id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(map_sqlite)?;
        let baseline = last_state.unwrap_or(created);

        let evidence_since = |kind: &str| -> Result<(i64, Option<i64>), ContinuityError> {
            tx.query_row(
                "SELECT COUNT(*), MAX(created_at) FROM evidence_records \
                 WHERE individual_id = ?1 AND kind = ?2 AND created_at > ?3",
                params![id, kind, baseline],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(map_sqlite)
        };
        let utterances_since = |retention: &str| -> Result<(i64, Option<i64>), ContinuityError> {
            tx.query_row(
                "SELECT COUNT(*), MAX(created_at) FROM evidence_records \
                 WHERE individual_id = ?1 AND kind = 'user_utterance' \
                   AND retention_class = ?2 AND created_at > ?3",
                params![id, retention, baseline],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(map_sqlite)
        };
        let (utterances, last_utterance) = utterances_since("standard")?;
        let (ephemeral, _) = utterances_since("ephemeral")?;
        let (reflections, last_reflection) = evidence_since("reflection")?;
        let drafts_rejected: i64 = tx
            .query_row(
                "SELECT COALESCE(SUM(CASE WHEN json_valid(payload_json) \
                     THEN json_array_length(payload_json, '$.drafts_rejected') END), 0) \
                 FROM evidence_records \
                 WHERE individual_id = ?1 AND kind = 'reflection' AND created_at > ?2",
                params![id, baseline],
                |row| row.get(0),
            )
            .map_err(map_sqlite)?;
        let proposals: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM mutation_proposals \
                 WHERE individual_id = ?1 AND recorded_at > ?2",
                params![id, baseline],
                |row| row.get(0),
            )
            .map_err(map_sqlite)?;

        let grouped = |sql: &str| -> Result<BTreeMap<String, u64>, ContinuityError> {
            let mut stmt = tx.prepare(sql).map_err(map_sqlite)?;
            let rows = stmt
                .query_map(params![id, baseline], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(map_sqlite)?;
            let mut out = BTreeMap::new();
            for row in rows {
                let (key, n) = row.map_err(map_sqlite)?;
                out.insert(key, count(n));
            }
            Ok(out)
        };
        let decisions = grouped(
            "SELECT d.disposition, COUNT(*) FROM mutation_decisions d \
             JOIN mutation_proposals p ON p.proposal_id = d.proposal_id \
             WHERE p.individual_id = ?1 AND p.recorded_at > ?2 \
             GROUP BY d.disposition",
        )?;
        let c0 = grouped(
            "SELECT status, COUNT(*) FROM c0_proposals \
             WHERE individual_id = ?1 AND created_at > ?2 GROUP BY status",
        )?;
        drop(tx);

        Ok(Some(MemoryActivity {
            individual_created_at: UtcTimestamp::from_unix_millis(created),
            state_records_total: count(state_total),
            last_state_record_at: last_state.map(UtcTimestamp::from_unix_millis),
            baseline_at: UtcTimestamp::from_unix_millis(baseline),
            user_utterances_since: count(utterances),
            last_user_utterance_at: last_utterance.map(UtcTimestamp::from_unix_millis),
            ephemeral_user_utterances_since: count(ephemeral),
            reflections_since: count(reflections),
            last_reflection_at: last_reflection.map(UtcTimestamp::from_unix_millis),
            reflection_drafts_rejected_since: count(drafts_rejected),
            memory_proposals_since: count(proposals),
            memory_decisions_since: decisions,
            c0_proposals_since: c0,
        }))
    }
}
