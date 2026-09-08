//! `EvidenceStore` on SQLite: sessions, turns, append-oriented records and
//! lineage links (plan §6.2).
//!
//! Appending never touches `continuity_heads`. Recording what was said is a
//! different act from changing what the individual holds true, and the schema
//! keeps them in different tables so the two can never be confused.

use std::collections::HashMap;
use std::str::FromStr;

use kamimusuhi_core::evidence::{
    EvidenceError, EvidenceFacts, EvidenceKind, EvidenceLink, EvidenceLookup, EvidenceRecord,
    EvidenceRelation, EvidenceSnapshot, EvidenceSource, EvidenceStore, NewEvidence,
    NewEvidenceLink, NewSession, NewTurn, RetentionClass, Session, Turn, resolve_roots,
};
use kamimusuhi_core::ids::{EvidenceId, IndividualId, SessionId};
use kamimusuhi_core::mutation::OriginClass;
use kamimusuhi_core::time::UtcTimestamp;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::store::SqliteStore;

fn corrupt(detail: impl Into<String>) -> EvidenceError {
    EvidenceError::Corrupt {
        detail: detail.into(),
    }
}

fn invalid(reason: impl Into<String>) -> EvidenceError {
    EvidenceError::Invalid {
        reason: reason.into(),
    }
}

fn map_sqlite(error: rusqlite::Error) -> EvidenceError {
    match &error {
        rusqlite::Error::SqliteFailure(failure, _)
            if matches!(
                failure.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            ) =>
        {
            EvidenceError::Contended {
                detail: error.to_string(),
            }
        }
        _ => EvidenceError::Backend {
            message: error.to_string(),
        },
    }
}

fn parse<T: FromStr>(field: &str, text: &str) -> Result<T, EvidenceError>
where
    T::Err: std::fmt::Display,
{
    text.parse::<T>()
        .map_err(|e| corrupt(format!("column {field} holds {text:?}: {e}")))
}

fn parse_opt<T: FromStr>(field: &str, text: Option<String>) -> Result<Option<T>, EvidenceError>
where
    T::Err: std::fmt::Display,
{
    text.map(|t| parse(field, &t)).transpose()
}

fn to_i64(value: u64, field: &str) -> Result<i64, EvidenceError> {
    i64::try_from(value).map_err(|_| invalid(format!("{field} {value} does not fit an INTEGER")))
}

fn from_i64(value: i64, field: &str) -> Result<u64, EvidenceError> {
    u64::try_from(value)
        .map_err(|_| corrupt(format!("column {field} holds negative value {value}")))
}

fn individual_exists(
    conn: &Connection,
    individual_id: IndividualId,
) -> Result<bool, EvidenceError> {
    conn.query_row(
        "SELECT COUNT(*) > 0 FROM individuals WHERE individual_id = ?1",
        params![individual_id.to_string()],
        |r| r.get(0),
    )
    .map_err(map_sqlite)
}

const EVIDENCE_COLUMNS: &str = "evidence_id, individual_id, session_id, turn_id, kind, origin_class, \
     payload_json, source_id, source_sequence, content_digest, retention_class, created_at";

type EvidenceRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    String,
    String,
    String,
    Option<String>,
    Option<i64>,
    Option<String>,
    String,
    i64,
);

fn read_evidence_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EvidenceRow> {
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
    ))
}

fn build_evidence(row: EvidenceRow) -> Result<EvidenceRecord, EvidenceError> {
    let (
        evidence_id,
        individual_id,
        session_id,
        turn_id,
        kind,
        origin_class,
        payload,
        source_id,
        source_sequence,
        content_digest,
        retention_class,
        created_at,
    ) = row;
    Ok(EvidenceRecord {
        evidence_id: parse("evidence_records.evidence_id", &evidence_id)?,
        individual_id: parse("evidence_records.individual_id", &individual_id)?,
        session_id: parse_opt("evidence_records.session_id", session_id)?,
        turn_id: parse_opt("evidence_records.turn_id", turn_id)?,
        kind: parse::<EvidenceKind>("evidence_records.kind", &kind)?,
        origin_class: parse::<OriginClass>("evidence_records.origin_class", &origin_class)?,
        payload: serde_json::from_str(&payload)
            .map_err(|e| corrupt(format!("evidence payload is not JSON: {e}")))?,
        source: EvidenceSource {
            source_id,
            source_sequence: source_sequence
                .map(|s| from_i64(s, "evidence_records.source_sequence"))
                .transpose()?,
            content_digest,
        },
        retention_class: parse::<RetentionClass>(
            "evidence_records.retention_class",
            &retention_class,
        )?,
        created_at: UtcTimestamp::from_unix_millis(created_at),
    })
}

fn get_in(
    conn: &Connection,
    evidence_id: EvidenceId,
) -> Result<Option<EvidenceRecord>, EvidenceError> {
    conn.query_row(
        &format!("SELECT {EVIDENCE_COLUMNS} FROM evidence_records WHERE evidence_id = ?1"),
        params![evidence_id.to_string()],
        read_evidence_row,
    )
    .optional()
    .map_err(map_sqlite)?
    .map(build_evidence)
    .transpose()
}

fn links_in(
    conn: &Connection,
    column: &str,
    evidence_id: EvidenceId,
) -> Result<Vec<EvidenceLink>, EvidenceError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT from_evidence_id, to_evidence_id, relation, created_at
             FROM evidence_links WHERE {column} = ?1 ORDER BY created_at, relation"
        ))
        .map_err(map_sqlite)?;
    let rows = stmt
        .query_map(params![evidence_id.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })
        .map_err(map_sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_sqlite)?;
    rows.into_iter()
        .map(|(from, to, relation, created_at)| {
            Ok(EvidenceLink {
                from_evidence_id: parse("evidence_links.from_evidence_id", &from)?,
                to_evidence_id: parse("evidence_links.to_evidence_id", &to)?,
                relation: parse::<EvidenceRelation>("evidence_links.relation", &relation)?,
                created_at: UtcTimestamp::from_unix_millis(created_at),
            })
        })
        .collect()
}

/// Every link declared by any record, for lineage resolution.
///
/// W2 keeps the whole edge set in memory: the graph is small (one edge per
/// derivation) and a bounded recursive query buys nothing yet. If the edge set
/// ever stops fitting, this becomes a recursive CTE without changing the
/// contract.
pub(crate) fn link_graph(
    conn: &Connection,
) -> Result<HashMap<EvidenceId, Vec<EvidenceLink>>, EvidenceError> {
    let mut stmt = conn
        .prepare(
            "SELECT from_evidence_id, to_evidence_id, relation, created_at FROM evidence_links",
        )
        .map_err(map_sqlite)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })
        .map_err(map_sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(map_sqlite)?;
    let mut map: HashMap<EvidenceId, Vec<EvidenceLink>> = HashMap::new();
    for (from, to, relation, created_at) in rows {
        let link = EvidenceLink {
            from_evidence_id: parse("evidence_links.from_evidence_id", &from)?,
            to_evidence_id: parse("evidence_links.to_evidence_id", &to)?,
            relation: parse::<EvidenceRelation>("evidence_links.relation", &relation)?,
            created_at: UtcTimestamp::from_unix_millis(created_at),
        };
        map.entry(link.from_evidence_id).or_default().push(link);
    }
    Ok(map)
}

/// Resolve the policy-facing facts for `evidence_ids` against an open
/// connection. Shared by the trait method and the activation transaction, so
/// what the policy saw and what activation re-checks cannot drift apart.
pub(crate) fn facts_in(
    conn: &Connection,
    evidence_ids: &[EvidenceId],
) -> Result<EvidenceSnapshot, EvidenceError> {
    if evidence_ids.is_empty() {
        return Ok(EvidenceSnapshot::default());
    }
    let links = link_graph(conn)?;
    let corrected_by: HashMap<EvidenceId, Vec<EvidenceId>> =
        links.values().flatten().fold(HashMap::new(), |mut acc, l| {
            if l.relation == EvidenceRelation::Corrects {
                acc.entry(l.to_evidence_id)
                    .or_default()
                    .push(l.from_evidence_id);
            }
            acc
        });

    let mut facts = Vec::new();
    for evidence_id in evidence_ids {
        let Some(record) = get_in(conn, *evidence_id)? else {
            continue;
        };
        let corrects = links
            .get(evidence_id)
            .map(|ls| {
                ls.iter()
                    .filter(|l| l.relation == EvidenceRelation::Corrects)
                    .map(|l| l.to_evidence_id)
                    .collect()
            })
            .unwrap_or_default();
        // A record whose *source* was corrected is no longer current either:
        // correcting the original does not leave a summary of it standing.
        let root_evidence = resolve_roots(*evidence_id, &links);
        let is_corrected = corrected_by.contains_key(evidence_id)
            || root_evidence.iter().any(|r| corrected_by.contains_key(r));
        facts.push(EvidenceFacts {
            evidence_id: *evidence_id,
            individual_id: record.individual_id,
            kind: record.kind,
            origin_class: record.origin_class,
            root_evidence,
            corrects,
            is_corrected,
        });
    }
    Ok(EvidenceSnapshot::new(facts))
}

/// Evidence corrected by any of `evidence_ids`.
pub(crate) fn corrected_by(
    conn: &Connection,
    evidence_ids: &[EvidenceId],
) -> Result<Vec<EvidenceId>, EvidenceError> {
    let snapshot = facts_in(conn, evidence_ids)?;
    let mut corrected: Vec<EvidenceId> = snapshot
        .facts()
        .iter()
        .flat_map(|f| f.corrects.iter().copied())
        .collect();
    corrected.sort_unstable();
    corrected.dedup();
    Ok(corrected)
}

impl EvidenceLookup for SqliteStore {
    fn facts(&self, evidence_ids: &[EvidenceId]) -> Result<EvidenceSnapshot, EvidenceError> {
        let conn = self.conn().map_err(|e| EvidenceError::Backend {
            message: e.to_string(),
        })?;
        facts_in(&conn, evidence_ids)
    }
}

impl EvidenceStore for SqliteStore {
    fn open_session(&self, session: NewSession) -> Result<Session, EvidenceError> {
        if session.session_id.is_nil() || session.individual_id.is_nil() {
            return Err(invalid("session has a nil id"));
        }
        let now = self.clock().now_utc();
        let mut conn = self.conn().map_err(|e| EvidenceError::Backend {
            message: e.to_string(),
        })?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite)?;
        if !individual_exists(&tx, session.individual_id)? {
            return Err(EvidenceError::IndividualNotFound(session.individual_id));
        }
        let existing: Option<(String, i64, Option<i64>)> = tx
            .query_row(
                "SELECT individual_id, started_at, ended_at FROM sessions WHERE session_id = ?1",
                params![session.session_id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(map_sqlite)?;
        if let Some((owner, started_at, ended_at)) = existing {
            let owner: IndividualId = parse("sessions.individual_id", &owner)?;
            if owner != session.individual_id {
                return Err(invalid(format!(
                    "session {} already belongs to {owner}",
                    session.session_id
                )));
            }
            return Ok(Session {
                session_id: session.session_id,
                individual_id: owner,
                started_at: UtcTimestamp::from_unix_millis(started_at),
                ended_at: ended_at.map(UtcTimestamp::from_unix_millis),
            });
        }
        tx.execute(
            "INSERT INTO sessions(session_id, individual_id, started_at, ended_at)
             VALUES (?1, ?2, ?3, NULL)",
            params![
                session.session_id.to_string(),
                session.individual_id.to_string(),
                now.unix_millis()
            ],
        )
        .map_err(map_sqlite)?;
        tx.commit().map_err(map_sqlite)?;
        Ok(Session {
            session_id: session.session_id,
            individual_id: session.individual_id,
            started_at: now,
            ended_at: None,
        })
    }

    fn record_turn(&self, turn: NewTurn) -> Result<Turn, EvidenceError> {
        if turn.turn_id.is_nil() || turn.session_id.is_nil() || turn.individual_id.is_nil() {
            return Err(invalid("turn has a nil id"));
        }
        let now = self.clock().now_utc();
        let sequence = to_i64(turn.sequence, "turns.sequence")?;
        let mut conn = self.conn().map_err(|e| EvidenceError::Backend {
            message: e.to_string(),
        })?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite)?;

        let session_owner: Option<String> = tx
            .query_row(
                "SELECT individual_id FROM sessions WHERE session_id = ?1",
                params![turn.session_id.to_string()],
                |r| r.get(0),
            )
            .optional()
            .map_err(map_sqlite)?;
        let Some(session_owner) = session_owner else {
            return Err(invalid(format!(
                "session {} does not exist",
                turn.session_id
            )));
        };
        if parse::<IndividualId>("sessions.individual_id", &session_owner)? != turn.individual_id {
            return Err(invalid(format!(
                "session {} belongs to another individual",
                turn.session_id
            )));
        }

        let existing: Option<(String, i64, i64)> = tx
            .query_row(
                "SELECT session_id, sequence, started_at FROM turns WHERE turn_id = ?1",
                params![turn.turn_id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(map_sqlite)?;
        if let Some((session_id, stored_sequence, started_at)) = existing {
            let session_id: SessionId = parse("turns.session_id", &session_id)?;
            if session_id != turn.session_id || stored_sequence != sequence {
                return Err(invalid(format!(
                    "turn {} already exists with different placement",
                    turn.turn_id
                )));
            }
            return Ok(Turn {
                turn_id: turn.turn_id,
                session_id,
                individual_id: turn.individual_id,
                sequence: turn.sequence,
                started_at: UtcTimestamp::from_unix_millis(started_at),
            });
        }

        tx.execute(
            "INSERT INTO turns(turn_id, session_id, individual_id, sequence, started_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                turn.turn_id.to_string(),
                turn.session_id.to_string(),
                turn.individual_id.to_string(),
                sequence,
                now.unix_millis()
            ],
        )
        .map_err(map_sqlite)?;
        tx.commit().map_err(map_sqlite)?;
        Ok(Turn {
            turn_id: turn.turn_id,
            session_id: turn.session_id,
            individual_id: turn.individual_id,
            sequence: turn.sequence,
            started_at: now,
        })
    }

    fn append(&self, record: NewEvidence) -> Result<EvidenceRecord, EvidenceError> {
        if record.evidence_id.is_nil() || record.individual_id.is_nil() {
            return Err(invalid("evidence has a nil id"));
        }
        let now = self.clock().now_utc();
        let mut conn = self.conn().map_err(|e| EvidenceError::Backend {
            message: e.to_string(),
        })?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite)?;
        if !individual_exists(&tx, record.individual_id)? {
            return Err(EvidenceError::IndividualNotFound(record.individual_id));
        }

        let stored = EvidenceRecord {
            evidence_id: record.evidence_id,
            individual_id: record.individual_id,
            session_id: record.session_id,
            turn_id: record.turn_id,
            kind: record.kind,
            origin_class: record.origin_class,
            payload: record.payload,
            source: record.source,
            retention_class: record.retention_class,
            created_at: now,
        };

        // Append-oriented: an identical re-append is a no-op, a differing one
        // is an error. Nothing overwrites an existing record.
        if let Some(existing) = get_in(&tx, record.evidence_id)? {
            let same = existing.individual_id == stored.individual_id
                && existing.session_id == stored.session_id
                && existing.turn_id == stored.turn_id
                && existing.kind == stored.kind
                && existing.origin_class == stored.origin_class
                && existing.payload == stored.payload
                && existing.source == stored.source
                && existing.retention_class == stored.retention_class;
            return if same {
                Ok(existing)
            } else {
                Err(EvidenceError::AlreadyExists(record.evidence_id))
            };
        }

        tx.execute(
            &format!(
                "INSERT INTO evidence_records({EVIDENCE_COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
            ),
            params![
                stored.evidence_id.to_string(),
                stored.individual_id.to_string(),
                stored.session_id.map(|s| s.to_string()),
                stored.turn_id.map(|t| t.to_string()),
                stored.kind.as_str(),
                stored.origin_class.as_str(),
                stored.payload.to_string(),
                stored.source.source_id,
                stored
                    .source
                    .source_sequence
                    .map(|s| to_i64(s, "source_sequence"))
                    .transpose()?,
                stored.source.content_digest,
                stored.retention_class.as_str(),
                stored.created_at.unix_millis(),
            ],
        )
        .map_err(map_sqlite)?;
        tx.commit().map_err(map_sqlite)?;
        get_in(&conn, record.evidence_id)?
            .ok_or_else(|| corrupt("appended evidence is missing right after commit"))
    }

    fn get(&self, evidence_id: EvidenceId) -> Result<Option<EvidenceRecord>, EvidenceError> {
        let conn = self.conn().map_err(|e| EvidenceError::Backend {
            message: e.to_string(),
        })?;
        get_in(&conn, evidence_id)
    }

    fn link(&self, link: NewEvidenceLink) -> Result<EvidenceLink, EvidenceError> {
        if link.from_evidence_id == link.to_evidence_id {
            return Err(invalid("an evidence record cannot link to itself"));
        }
        let now = self.clock().now_utc();
        let mut conn = self.conn().map_err(|e| EvidenceError::Backend {
            message: e.to_string(),
        })?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite)?;
        let from = get_in(&tx, link.from_evidence_id)?
            .ok_or(EvidenceError::NotFound(link.from_evidence_id))?;
        let to = get_in(&tx, link.to_evidence_id)?
            .ok_or(EvidenceError::NotFound(link.to_evidence_id))?;
        if from.individual_id != to.individual_id {
            return Err(invalid(
                "evidence lineage cannot cross individuals".to_owned(),
            ));
        }
        tx.execute(
            "INSERT INTO evidence_links(from_evidence_id, to_evidence_id, relation, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(from_evidence_id, to_evidence_id, relation) DO NOTHING",
            params![
                link.from_evidence_id.to_string(),
                link.to_evidence_id.to_string(),
                link.relation.as_str(),
                now.unix_millis()
            ],
        )
        .map_err(map_sqlite)?;
        let created_at: i64 = tx
            .query_row(
                "SELECT created_at FROM evidence_links
                 WHERE from_evidence_id = ?1 AND to_evidence_id = ?2 AND relation = ?3",
                params![
                    link.from_evidence_id.to_string(),
                    link.to_evidence_id.to_string(),
                    link.relation.as_str()
                ],
                |r| r.get(0),
            )
            .map_err(map_sqlite)?;
        tx.commit().map_err(map_sqlite)?;
        Ok(EvidenceLink {
            from_evidence_id: link.from_evidence_id,
            to_evidence_id: link.to_evidence_id,
            relation: link.relation,
            created_at: UtcTimestamp::from_unix_millis(created_at),
        })
    }

    fn links_from(&self, evidence_id: EvidenceId) -> Result<Vec<EvidenceLink>, EvidenceError> {
        let conn = self.conn().map_err(|e| EvidenceError::Backend {
            message: e.to_string(),
        })?;
        links_in(&conn, "from_evidence_id", evidence_id)
    }

    fn links_to(&self, evidence_id: EvidenceId) -> Result<Vec<EvidenceLink>, EvidenceError> {
        let conn = self.conn().map_err(|e| EvidenceError::Backend {
            message: e.to_string(),
        })?;
        links_in(&conn, "to_evidence_id", evidence_id)
    }
}
