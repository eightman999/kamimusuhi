//! `ResourceCallLog` on SQLite (plan §6.5).
//!
//! Records who was asked, when, and what came back — by digest. A call is
//! attribution, not a canonical transition: it writes no commit, moves no
//! head, and appears in `resource_calls`, never in `audit_events`.
//!
//! No column here can hold a credential, a request body or a result body.
//! That is a schema property, not a convention.

use std::str::FromStr;

use kamimusuhi_core::ids::{IndividualId, ResourceCallId, TurnId};
use kamimusuhi_core::resources::{
    NewResourceCall, ResourceCall, ResourceCallLog, ResourceCallLogError, ResourceOutcome,
    ResourceSlot,
};
use kamimusuhi_core::time::UtcTimestamp;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::store::SqliteStore;

fn corrupt(detail: impl Into<String>) -> ResourceCallLogError {
    ResourceCallLogError::Corrupt {
        detail: detail.into(),
    }
}

fn invalid(reason: impl Into<String>) -> ResourceCallLogError {
    ResourceCallLogError::Invalid {
        reason: reason.into(),
    }
}

fn map_sqlite(error: rusqlite::Error) -> ResourceCallLogError {
    match &error {
        rusqlite::Error::SqliteFailure(failure, _)
            if matches!(
                failure.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            ) =>
        {
            ResourceCallLogError::Contended {
                detail: error.to_string(),
            }
        }
        _ => ResourceCallLogError::Backend {
            message: error.to_string(),
        },
    }
}

fn parse<T: FromStr>(field: &str, text: &str) -> Result<T, ResourceCallLogError>
where
    T::Err: std::fmt::Display,
{
    text.parse::<T>()
        .map_err(|e| corrupt(format!("column {field} holds {text:?}: {e}")))
}

const CALL_COLUMNS: &str = "resource_call_id, resource_id, slot, individual_id, turn_id, adapter, \
     purpose, request_digest, outcome, result_digest, error_code, attempts, latency_ms, \
     started_at, completed_at";

type CallRow = (
    String,
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
    i64,
    i64,
    i64,
);

fn read_call_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<CallRow> {
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
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
        row.get(14)?,
    ))
}

fn build_call(row: CallRow) -> Result<ResourceCall, ResourceCallLogError> {
    let (
        resource_call_id,
        resource_id,
        slot,
        individual_id,
        turn_id,
        adapter,
        purpose,
        request_digest,
        outcome,
        result_digest,
        error_code,
        attempts,
        latency_ms,
        started_at,
        completed_at,
    ) = row;
    Ok(ResourceCall {
        resource_call_id: parse("resource_calls.resource_call_id", &resource_call_id)?,
        resource_id: parse("resource_calls.resource_id", &resource_id)?,
        slot: ResourceSlot::new(slot),
        individual_id: parse("resource_calls.individual_id", &individual_id)?,
        turn_id: turn_id
            .map(|id| parse::<TurnId>("resource_calls.turn_id", &id))
            .transpose()?,
        adapter,
        purpose,
        request_digest,
        outcome: parse::<ResourceOutcome>("resource_calls.outcome", &outcome)?,
        result_digest,
        error_code,
        attempts: u32::try_from(attempts)
            .map_err(|_| corrupt(format!("resource_calls.attempts holds {attempts}")))?,
        latency_ms: u64::try_from(latency_ms)
            .map_err(|_| corrupt(format!("resource_calls.latency_ms holds {latency_ms}")))?,
        started_at: UtcTimestamp::from_unix_millis(started_at),
        completed_at: UtcTimestamp::from_unix_millis(completed_at),
    })
}

fn get_call_in(
    conn: &Connection,
    resource_call_id: ResourceCallId,
) -> Result<Option<ResourceCall>, ResourceCallLogError> {
    conn.query_row(
        &format!("SELECT {CALL_COLUMNS} FROM resource_calls WHERE resource_call_id = ?1"),
        params![resource_call_id.to_string()],
        read_call_row,
    )
    .optional()
    .map_err(map_sqlite)?
    .map(build_call)
    .transpose()
}

fn individual_exists(
    conn: &Connection,
    individual_id: IndividualId,
) -> Result<bool, ResourceCallLogError> {
    conn.query_row(
        "SELECT COUNT(*) > 0 FROM individuals WHERE individual_id = ?1",
        params![individual_id.to_string()],
        |r| r.get(0),
    )
    .map_err(map_sqlite)
}

impl ResourceCallLog for SqliteStore {
    fn record(&self, call: NewResourceCall) -> Result<ResourceCall, ResourceCallLogError> {
        if call.resource_call_id.is_nil() || call.resource_id.is_nil() {
            return Err(invalid("resource call has a nil id"));
        }
        if call.attempts == 0 {
            return Err(invalid("a recorded call must have at least one attempt"));
        }
        match (call.outcome, &call.result_digest, &call.error_code) {
            (ResourceOutcome::Ok, Some(_), None) | (ResourceOutcome::Error, None, Some(_)) => {}
            _ => {
                return Err(invalid(
                    "outcome must carry exactly one of result_digest or error_code",
                ));
            }
        }

        let mut conn = self.conn().map_err(|e| ResourceCallLogError::Backend {
            message: e.to_string(),
        })?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite)?;
        if !individual_exists(&tx, call.individual_id)? {
            return Err(ResourceCallLogError::IndividualNotFound(call.individual_id));
        }
        if get_call_in(&tx, call.resource_call_id)?.is_some() {
            return Err(invalid(format!(
                "resource call {} is already recorded",
                call.resource_call_id
            )));
        }

        tx.execute(
            &format!(
                "INSERT INTO resource_calls({CALL_COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"
            ),
            params![
                call.resource_call_id.to_string(),
                call.resource_id.to_string(),
                call.slot.as_str(),
                call.individual_id.to_string(),
                call.turn_id.map(|id| id.to_string()),
                call.adapter,
                call.purpose,
                call.request_digest,
                call.outcome.as_str(),
                call.result_digest,
                call.error_code,
                i64::from(call.attempts),
                i64::try_from(call.latency_ms).unwrap_or(i64::MAX),
                call.started_at.unix_millis(),
                call.completed_at.unix_millis(),
            ],
        )
        .map_err(map_sqlite)?;
        tx.commit().map_err(map_sqlite)?;

        get_call_in(&conn, call.resource_call_id)?
            .ok_or_else(|| corrupt("resource call is missing right after commit"))
    }

    fn get_call(
        &self,
        resource_call_id: ResourceCallId,
    ) -> Result<Option<ResourceCall>, ResourceCallLogError> {
        let conn = self.conn().map_err(|e| ResourceCallLogError::Backend {
            message: e.to_string(),
        })?;
        get_call_in(&conn, resource_call_id)
    }

    fn calls(
        &self,
        individual_id: IndividualId,
    ) -> Result<Vec<ResourceCall>, ResourceCallLogError> {
        let conn = self.conn().map_err(|e| ResourceCallLogError::Backend {
            message: e.to_string(),
        })?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {CALL_COLUMNS} FROM resource_calls
                 WHERE individual_id = ?1 ORDER BY started_at, resource_call_id"
            ))
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![individual_id.to_string()], read_call_row)
            .map_err(map_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;
        rows.into_iter().map(build_call).collect()
    }
}
