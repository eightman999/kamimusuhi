//! Durable derived state on SQLite (plan §6.3).
//!
//! Reads are plain indexed lookups: no model call is involved, so a restarted
//! runtime answers from disk alone. Writes are not here — a state record only
//! ever comes into being inside the activation transaction, which is what
//! keeps durable memory behind the Continuity Kernel.

use std::str::FromStr;

use kamimusuhi_core::continuity::ContinuityError;
use kamimusuhi_core::evidence::EvidenceSnapshot;
use kamimusuhi_core::ids::{CommitId, EvidenceId, IndividualId, MemoryId};
use kamimusuhi_core::memory::{
    AttributedMemory, LifecycleState, MemoryError, MemoryQuery, MemoryRepository, StateLookup,
    StateRecord, StateRecordFacts,
};
use kamimusuhi_core::mutation::{MutationDomain, MutationOperation};
use kamimusuhi_core::time::UtcTimestamp;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::evidence::facts_in;
use crate::store::SqliteStore;

fn corrupt(detail: impl Into<String>) -> MemoryError {
    MemoryError::Corrupt {
        detail: detail.into(),
    }
}

fn map_sqlite(error: rusqlite::Error) -> MemoryError {
    match &error {
        rusqlite::Error::SqliteFailure(failure, _)
            if matches!(
                failure.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            ) =>
        {
            MemoryError::Contended {
                detail: error.to_string(),
            }
        }
        _ => MemoryError::Backend {
            message: error.to_string(),
        },
    }
}

fn parse<T: FromStr>(field: &str, text: &str) -> Result<T, MemoryError>
where
    T::Err: std::fmt::Display,
{
    text.parse::<T>()
        .map_err(|e| corrupt(format!("column {field} holds {text:?}: {e}")))
}

fn parse_opt<T: FromStr>(field: &str, text: Option<String>) -> Result<Option<T>, MemoryError>
where
    T::Err: std::fmt::Display,
{
    text.map(|t| parse(field, &t)).transpose()
}

const STATE_COLUMNS: &str = "state_record_id, individual_id, domain, subject_key, kind, \
     payload_json, lifecycle_state, created_commit_id, supersedes_state_record_id, \
     superseded_by_state_record_id, created_at";

type StateRow = (
    String,
    String,
    String,
    Option<String>,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    i64,
);

fn read_state_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StateRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
    ))
}

fn evidence_refs_of(
    conn: &Connection,
    state_record_id: MemoryId,
) -> Result<Vec<EvidenceId>, MemoryError> {
    let mut stmt = conn
        .prepare(
            "SELECT evidence_id FROM state_record_evidence
             WHERE state_record_id = ?1 ORDER BY evidence_id",
        )
        .map_err(map_sqlite)?;
    let rows = stmt
        .query_map(params![state_record_id.to_string()], |r| {
            r.get::<_, String>(0)
        })
        .map_err(map_sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_sqlite)?;
    rows.iter()
        .map(|id| parse("state_record_evidence.evidence_id", id))
        .collect()
}

fn build_state(conn: &Connection, row: StateRow) -> Result<StateRecord, MemoryError> {
    let (
        state_record_id,
        individual_id,
        domain,
        subject_key,
        kind,
        payload,
        lifecycle_state,
        created_commit_id,
        supersedes,
        superseded_by,
        created_at,
    ) = row;
    let state_record_id: MemoryId = parse("state_records.state_record_id", &state_record_id)?;
    Ok(StateRecord {
        state_record_id,
        individual_id: parse("state_records.individual_id", &individual_id)?,
        domain: parse::<MutationDomain>("state_records.domain", &domain)?,
        subject_key,
        kind,
        payload: serde_json::from_str(&payload)
            .map_err(|e| corrupt(format!("state payload is not JSON: {e}")))?,
        lifecycle_state: parse::<LifecycleState>(
            "state_records.lifecycle_state",
            &lifecycle_state,
        )?,
        created_commit_id: parse("state_records.created_commit_id", &created_commit_id)?,
        supersedes_state_record_id: parse_opt(
            "state_records.supersedes_state_record_id",
            supersedes,
        )?,
        superseded_by_state_record_id: parse_opt(
            "state_records.superseded_by_state_record_id",
            superseded_by,
        )?,
        evidence_refs: evidence_refs_of(conn, state_record_id)?,
        created_at: UtcTimestamp::from_unix_millis(created_at),
    })
}

fn get_state_in(
    conn: &Connection,
    state_record_id: MemoryId,
) -> Result<Option<StateRecord>, MemoryError> {
    let row = conn
        .query_row(
            &format!("SELECT {STATE_COLUMNS} FROM state_records WHERE state_record_id = ?1"),
            params![state_record_id.to_string()],
            read_state_row,
        )
        .optional()
        .map_err(map_sqlite)?;
    row.map(|row| build_state(conn, row)).transpose()
}

impl StateLookup for SqliteStore {
    fn state_facts(
        &self,
        state_record_id: MemoryId,
    ) -> Result<Option<StateRecordFacts>, MemoryError> {
        let conn = self.conn().map_err(|e| MemoryError::Backend {
            message: e.to_string(),
        })?;
        state_facts_in(&conn, state_record_id)
    }
}

pub(crate) fn state_facts_in(
    conn: &Connection,
    state_record_id: MemoryId,
) -> Result<Option<StateRecordFacts>, MemoryError> {
    let row: Option<(String, String, Option<String>, String)> = conn
        .query_row(
            "SELECT individual_id, domain, subject_key, lifecycle_state
             FROM state_records WHERE state_record_id = ?1",
            params![state_record_id.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .map_err(map_sqlite)?;
    row.map(|(individual_id, domain, subject_key, lifecycle_state)| {
        Ok(StateRecordFacts {
            state_record_id,
            individual_id: parse("state_records.individual_id", &individual_id)?,
            domain: parse::<MutationDomain>("state_records.domain", &domain)?,
            subject_key,
            lifecycle_state: parse::<LifecycleState>(
                "state_records.lifecycle_state",
                &lifecycle_state,
            )?,
        })
    })
    .transpose()
}

impl MemoryRepository for SqliteStore {
    fn retrieve(&self, query: &MemoryQuery) -> Result<Vec<AttributedMemory>, MemoryError> {
        if query.individual_id.is_nil() {
            return Err(MemoryError::Invalid {
                reason: "query has a nil individual_id".to_owned(),
            });
        }
        let conn = self.conn().map_err(|e| MemoryError::Backend {
            message: e.to_string(),
        })?;

        let mut sql = format!("SELECT {STATE_COLUMNS} FROM state_records WHERE individual_id = ?1");
        let mut args: Vec<Box<dyn rusqlite::ToSql>> =
            vec![Box::new(query.individual_id.to_string())];
        if !query.include_non_current {
            sql.push_str(" AND lifecycle_state = 'active'");
        }
        if let Some(domain) = query.domain {
            args.push(Box::new(domain.as_str().to_owned()));
            sql.push_str(&format!(" AND domain = ?{}", args.len()));
        }
        if let Some(subject_key) = &query.subject_key {
            args.push(Box::new(subject_key.clone()));
            sql.push_str(&format!(" AND subject_key = ?{}", args.len()));
        }
        sql.push_str(" ORDER BY created_at DESC, state_record_id DESC");
        if let Some(limit) = query.limit {
            sql.push_str(&format!(" LIMIT {limit}"));
        }

        let mut stmt = conn.prepare(&sql).map_err(map_sqlite)?;
        let params: Vec<&dyn rusqlite::ToSql> = args.iter().map(AsRef::as_ref).collect();
        let rows = stmt
            .query_map(params.as_slice(), read_state_row)
            .map_err(map_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;

        rows.into_iter()
            .map(|row| {
                let record = build_state(&conn, row)?;
                let snapshot =
                    facts_in(&conn, &record.evidence_refs).map_err(|e| MemoryError::Backend {
                        message: e.to_string(),
                    })?;
                Ok(attribute(record, &snapshot))
            })
            .collect()
    }

    fn get_state_record(
        &self,
        state_record_id: MemoryId,
    ) -> Result<Option<StateRecord>, MemoryError> {
        let conn = self.conn().map_err(|e| MemoryError::Backend {
            message: e.to_string(),
        })?;
        get_state_in(&conn, state_record_id)
    }
}

/// Attach independent-support attribution to a record.
///
/// The count is distinct *root* evidence, so three summaries of one
/// conversation contribute one, not three (test T07).
fn attribute(record: StateRecord, snapshot: &EvidenceSnapshot) -> AttributedMemory {
    let root_evidence: Vec<EvidenceId> = snapshot.independent_support().into_iter().collect();
    AttributedMemory {
        independent_evidence_count: root_evidence.len(),
        root_evidence,
        record,
    }
}

/// Minimal W2 `state_records.kind` vocabulary, derived from the operation.
pub(crate) const fn state_kind(operation: MutationOperation) -> &'static str {
    match operation {
        MutationOperation::Capture => "episode",
        MutationOperation::Fact | MutationOperation::Correction => "fact",
    }
}

/// Insert the state record activated by a proposal, inside the activation
/// transaction. Called only from [`crate::continuity`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn insert_state_record(
    tx: &Transaction<'_>,
    state_record_id: MemoryId,
    individual_id: IndividualId,
    domain: MutationDomain,
    subject_key: Option<&str>,
    kind: &str,
    payload: &serde_json::Value,
    created_commit_id: CommitId,
    supersedes: Option<MemoryId>,
    evidence_refs: &[EvidenceId],
    now: UtcTimestamp,
) -> Result<(), ContinuityError> {
    let backend = |e: rusqlite::Error| ContinuityError::Backend {
        message: e.to_string(),
    };
    tx.execute(
        &format!(
            "INSERT INTO state_records({STATE_COLUMNS})
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', ?7, ?8, NULL, ?9)"
        ),
        params![
            state_record_id.to_string(),
            individual_id.to_string(),
            domain.as_str(),
            subject_key,
            kind,
            payload.to_string(),
            created_commit_id.to_string(),
            supersedes.map(|s| s.to_string()),
            now.unix_millis(),
        ],
    )
    .map_err(backend)?;
    for evidence_id in evidence_refs {
        tx.execute(
            "INSERT INTO state_record_evidence(state_record_id, evidence_id) VALUES (?1, ?2)
             ON CONFLICT(state_record_id, evidence_id) DO NOTHING",
            params![state_record_id.to_string(), evidence_id.to_string()],
        )
        .map_err(backend)?;
    }
    Ok(())
}

/// Mark `target` superseded by `replacement`. History is kept: the old row
/// stays, only its lifecycle and back-reference change.
pub(crate) fn mark_superseded(
    tx: &Transaction<'_>,
    target: MemoryId,
    replacement: MemoryId,
) -> Result<(), ContinuityError> {
    let changed = tx
        .execute(
            "UPDATE state_records
             SET lifecycle_state = 'superseded', superseded_by_state_record_id = ?2
             WHERE state_record_id = ?1 AND lifecycle_state = 'active'",
            params![target.to_string(), replacement.to_string()],
        )
        .map_err(|e| ContinuityError::Backend {
            message: e.to_string(),
        })?;
    if changed != 1 {
        return Err(ContinuityError::Corrupt {
            detail: format!(
                "supersession target {target} was not active inside the activation transaction"
            ),
        });
    }
    Ok(())
}

/// Move every active record that rests on `evidence_ids` to `invalidated`.
///
/// This is the re-evaluation edge of audit A03: correcting a source does not
/// declare the derived interpretation false, it takes it out of the current
/// view until something re-establishes it. Returns the affected records.
pub(crate) fn invalidate_records_supported_by(
    tx: &Transaction<'_>,
    individual_id: IndividualId,
    evidence_ids: &[EvidenceId],
    except: Option<MemoryId>,
) -> Result<Vec<MemoryId>, ContinuityError> {
    let backend = |e: rusqlite::Error| ContinuityError::Backend {
        message: e.to_string(),
    };
    let mut affected = Vec::new();
    for evidence_id in evidence_ids {
        let mut stmt = tx
            .prepare(
                "SELECT s.state_record_id FROM state_records s
                 JOIN state_record_evidence e ON e.state_record_id = s.state_record_id
                 WHERE s.individual_id = ?1 AND e.evidence_id = ?2 AND s.lifecycle_state = 'active'",
            )
            .map_err(backend)?;
        let ids = stmt
            .query_map(
                params![individual_id.to_string(), evidence_id.to_string()],
                |r| r.get::<_, String>(0),
            )
            .map_err(backend)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend)?;
        drop(stmt);
        for id in ids {
            let record_id: MemoryId = id.parse().map_err(|e| ContinuityError::Corrupt {
                detail: format!("state_records.state_record_id holds {id:?}: {e}"),
            })?;
            if except == Some(record_id) || affected.contains(&record_id) {
                continue;
            }
            tx.execute(
                "UPDATE state_records SET lifecycle_state = 'invalidated'
                 WHERE state_record_id = ?1 AND lifecycle_state = 'active'",
                params![id],
            )
            .map_err(backend)?;
            affected.push(record_id);
        }
    }
    Ok(affected)
}
