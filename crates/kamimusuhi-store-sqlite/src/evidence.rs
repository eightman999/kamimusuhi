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
use kamimusuhi_core::ids::{EvidenceId, IndividualId, SessionId, TurnId};
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

pub(crate) const EVIDENCE_COLUMNS: &str = "evidence_id, individual_id, session_id, turn_id, kind, origin_class, \
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

pub(crate) fn read_evidence_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EvidenceRow> {
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

pub(crate) fn build_evidence(row: EvidenceRow) -> Result<EvidenceRecord, EvidenceError> {
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

fn append_in(
    conn: &Connection,
    record: NewEvidence,
    now: UtcTimestamp,
) -> Result<EvidenceRecord, EvidenceError> {
    if record.evidence_id.is_nil() || record.individual_id.is_nil() {
        return Err(invalid("evidence has a nil id"));
    }
    if !individual_exists(conn, record.individual_id)? {
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
    if let Some(existing) = get_in(conn, record.evidence_id)? {
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

    conn.execute(
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
    let landed = get_in(conn, record.evidence_id)?
        .ok_or_else(|| corrupt("appended evidence is missing before commit"))?;
    if landed != stored {
        return Err(corrupt("appended evidence readback mismatch"));
    }
    Ok(landed)
}

fn link_in(
    conn: &Connection,
    link: &NewEvidenceLink,
    now: UtcTimestamp,
) -> Result<EvidenceLink, EvidenceError> {
    if link.from_evidence_id == link.to_evidence_id {
        return Err(invalid("an evidence record cannot link to itself"));
    }
    let from = get_in(conn, link.from_evidence_id)?
        .ok_or(EvidenceError::NotFound(link.from_evidence_id))?;
    let to =
        get_in(conn, link.to_evidence_id)?.ok_or(EvidenceError::NotFound(link.to_evidence_id))?;
    if from.individual_id != to.individual_id {
        return Err(invalid(
            "evidence lineage cannot cross individuals".to_owned(),
        ));
    }
    conn.execute(
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
    let created_at: i64 = conn
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
    Ok(EvidenceLink {
        from_evidence_id: link.from_evidence_id,
        to_evidence_id: link.to_evidence_id,
        relation: link.relation,
        created_at: UtcTimestamp::from_unix_millis(created_at),
    })
}

impl SqliteStore {
    /// Append evidence and its lineage as one atomic operation. A failed link
    /// cannot leave a visible, unsupported utterance in subsequent context.
    /// Repeating an identical append/link set is idempotent, as for append/link.
    pub fn append_with_links(
        &self,
        record: NewEvidence,
        links: &[NewEvidenceLink],
    ) -> Result<EvidenceRecord, EvidenceError> {
        if links
            .iter()
            .any(|link| link.from_evidence_id != record.evidence_id)
        {
            return Err(invalid(
                "atomic lineage must originate at the appended evidence",
            ));
        }
        let now = self.clock().now_utc();
        let mut conn = self.conn().map_err(|e| EvidenceError::Backend {
            message: e.to_string(),
        })?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite)?;
        let stored = append_in(&tx, record, now)?;
        for link in links {
            link_in(&tx, link, now)?;
        }
        let landed_links = links_in(&tx, "from_evidence_id", stored.evidence_id)?;
        if links.iter().any(|link| {
            !landed_links.iter().any(|landed| {
                landed.from_evidence_id == link.from_evidence_id
                    && landed.to_evidence_id == link.to_evidence_id
                    && landed.relation == link.relation
            })
        }) {
            return Err(corrupt("atomic lineage readback mismatch"));
        }
        // A link trigger can also affect the evidence; verify the final landing.
        if get_in(&tx, stored.evidence_id)?.as_ref() != Some(&stored) {
            return Err(corrupt("atomic evidence readback mismatch"));
        }
        tx.commit().map_err(map_sqlite)?;
        Ok(stored)
    }
}

impl SqliteStore {
    /// Bounded, insertion-ordered evidence for an operator-reviewed turn.
    /// Validate ownership rather than silently filtering out corrupt rows.
    pub fn evidence_for_turn(
        &self,
        individual_id: IndividualId,
        turn_id: TurnId,
    ) -> Result<Vec<EvidenceRecord>, EvidenceError> {
        let conn = self.conn().map_err(|e| EvidenceError::Backend {
            message: e.to_string(),
        })?;
        let owner = conn
            .query_row(
                "SELECT individual_id FROM turns WHERE turn_id = ?1",
                [turn_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(map_sqlite)?;
        if owner.as_deref() != Some(individual_id.to_string().as_str()) {
            return Err(invalid("turn is missing or belongs to another individual"));
        }
        let mut statement = conn
            .prepare(&format!(
                "SELECT {EVIDENCE_COLUMNS} FROM evidence_records
                 WHERE turn_id = ?1 ORDER BY rowid LIMIT 4097"
            ))
            .map_err(map_sqlite)?;
        let records = statement
            .query_map([turn_id.to_string()], read_evidence_row)
            .map_err(map_sqlite)?
            .map(|row| build_evidence(row.map_err(map_sqlite)?))
            .collect::<Result<Vec<_>, EvidenceError>>()?;
        if records.len() > 4096 {
            return Err(invalid("turn evidence exceeds the review limit"));
        }
        if records
            .iter()
            .any(|record| record.individual_id != individual_id)
        {
            return Err(corrupt("turn evidence ownership mismatch"));
        }
        Ok(records)
    }

    /// Recent raw dialogue from one operator-selected channel, oldest first.
    /// Generated-but-unemitted system events and other people's channels are
    /// excluded. SQLite insertion order also works with fixed test clocks.
    pub fn recent_conversation(
        &self,
        individual_id: IndividualId,
        source_id: &str,
        limit: usize,
    ) -> Result<Vec<EvidenceRecord>, EvidenceError> {
        if !(1..=128).contains(&limit) {
            return Err(invalid("conversation limit must be between 1 and 128"));
        }
        let conn = self.conn().map_err(|e| EvidenceError::Backend {
            message: e.to_string(),
        })?;
        let mut statement = conn
            .prepare(&format!(
                "SELECT {EVIDENCE_COLUMNS} FROM evidence_records
             WHERE individual_id = ?1 AND source_id = ?2
               AND kind IN ('user_utterance', 'agent_utterance')
             ORDER BY rowid DESC LIMIT ?3"
            ))
            .map_err(map_sqlite)?;
        let mut records = statement
            .query_map(
                params![individual_id.to_string(), source_id, limit as i64],
                read_evidence_row,
            )
            .map_err(map_sqlite)?
            .map(|row| build_evidence(row.map_err(map_sqlite)?))
            .collect::<Result<Vec<_>, EvidenceError>>()?;
        records.reverse();
        Ok(records)
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

    fn session(&self, session_id: SessionId) -> Result<Option<Session>, EvidenceError> {
        let conn = self.conn().map_err(|e| EvidenceError::Backend {
            message: e.to_string(),
        })?;
        let row: Option<(String, i64, Option<i64>)> = conn
            .query_row(
                "SELECT individual_id, started_at, ended_at FROM sessions WHERE session_id = ?1",
                params![session_id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(map_sqlite)?;
        row.map(|(individual_id, started_at, ended_at)| {
            Ok(Session {
                session_id,
                individual_id: parse("sessions.individual_id", &individual_id)?,
                started_at: UtcTimestamp::from_unix_millis(started_at),
                ended_at: ended_at.map(UtcTimestamp::from_unix_millis),
            })
        })
        .transpose()
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
        self.append_with_links(record, &[])
    }

    fn get(&self, evidence_id: EvidenceId) -> Result<Option<EvidenceRecord>, EvidenceError> {
        let conn = self.conn().map_err(|e| EvidenceError::Backend {
            message: e.to_string(),
        })?;
        get_in(&conn, evidence_id)
    }

    fn link(&self, link: NewEvidenceLink) -> Result<EvidenceLink, EvidenceError> {
        let now = self.clock().now_utc();
        let mut conn = self.conn().map_err(|e| EvidenceError::Backend {
            message: e.to_string(),
        })?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite)?;
        let stored = link_in(&tx, &link, now)?;
        tx.commit().map_err(map_sqlite)?;
        Ok(stored)
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

fn redact_record_in(
    tx: &Connection,
    record: &EvidenceRecord,
    discord_message_id: Option<u64>,
    reason: &str,
    now: UtcTimestamp,
) -> Result<bool, EvidenceError> {
    if record
        .payload
        .get("redacted")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        return Ok(false);
    }
    let digest = kamimusuhi_core::digest::content_digest(
        format!("social-redact:{}:{reason}", record.evidence_id).as_bytes(),
    );
    let hex = digest.strip_prefix("sha256:").unwrap_or(&digest);
    let mut correction_id: EvidenceId = hex[..32]
        .parse()
        .unwrap_or_else(|_| EvidenceId::from_u128(1));
    if correction_id.is_nil() || correction_id == record.evidence_id {
        correction_id = EvidenceId::from_u128(
            record
                .evidence_id
                .runtime_id()
                .as_u128()
                .wrapping_add(1)
                .max(1),
        );
    }
    let resolved_msg_id = discord_message_id.or_else(|| {
        record
            .payload
            .get("discord")
            .and_then(|d| d.get("message_id"))
            .and_then(serde_json::Value::as_u64)
    });
    let correction_payload = serde_json::json!({
        "event": "social_utterance_redacted",
        "target_evidence_id": record.evidence_id,
        "discord_message_id": resolved_msg_id,
        "reason": reason,
    });
    append_in(
        tx,
        NewEvidence {
            evidence_id: correction_id,
            individual_id: record.individual_id,
            session_id: record.session_id,
            turn_id: record.turn_id,
            kind: EvidenceKind::Correction,
            origin_class: OriginClass::Observed,
            payload: correction_payload,
            source: record.source.clone(),
            retention_class: RetentionClass::Standard,
        },
        now,
    )?;
    link_in(
        tx,
        &NewEvidenceLink {
            from_evidence_id: correction_id,
            to_evidence_id: record.evidence_id,
            relation: EvidenceRelation::Corrects,
        },
        now,
    )?;
    let scrubbed_discord = record
        .payload
        .get("discord")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let scrubbed_payload = serde_json::json!({
        "redacted": true,
        "reason": reason,
        "discord": scrubbed_discord,
        "redacted_at": now.unix_millis(),
    });
    tx.execute(
        "UPDATE evidence_records
            SET payload_json = ?1, content_digest = NULL, retention_class = 'ephemeral'
          WHERE evidence_id = ?2 AND individual_id = ?3",
        params![
            scrubbed_payload.to_string(),
            record.evidence_id.to_string(),
            record.individual_id.to_string(),
        ],
    )
    .map_err(map_sqlite)?;
    tx.execute(
        "UPDATE state_records
            SET lifecycle_state = 'invalidated'
          WHERE individual_id = ?1
            AND lifecycle_state = 'active'
            AND state_record_id IN (
                SELECT state_record_id FROM state_record_evidence WHERE evidence_id = ?2
            )",
        params![
            record.individual_id.to_string(),
            record.evidence_id.to_string(),
        ],
    )
    .map_err(map_sqlite)?;
    Ok(true)
}

impl SqliteStore {
    /// Scrub the text of a Discord `UserUtterance` (on deletion or explicit user request),
    /// record a `Correction` evidence item with `Corrects` lineage, and invalidate any
    /// active `state_records` backed by the scrubbed record.
    pub fn redact_discord_utterance(
        &self,
        individual_id: IndividualId,
        discord_message_id: Option<u64>,
        target_evidence_id: Option<EvidenceId>,
        requester_user_id: Option<u64>,
        reason: &str,
    ) -> Result<Vec<EvidenceId>, EvidenceError> {
        let reason = reason.trim();
        if reason.is_empty() || reason.len() > 512 {
            return Err(invalid("redaction reason must be 1..=512 bytes"));
        }
        if discord_message_id.is_none() && target_evidence_id.is_none() {
            return Err(invalid(
                "discord_message_id or target_evidence_id is required",
            ));
        }
        let now = self.clock().now_utc();
        let mut conn = self.conn().map_err(|e| EvidenceError::Backend {
            message: e.to_string(),
        })?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite)?;
        if !individual_exists(&tx, individual_id)? {
            return Err(EvidenceError::IndividualNotFound(individual_id));
        }
        let records = {
            let mut stmt = tx
                .prepare(&format!(
                    "SELECT {EVIDENCE_COLUMNS} FROM evidence_records
                     WHERE individual_id = ?1 AND kind = 'user_utterance'
                     ORDER BY rowid DESC"
                ))
                .map_err(map_sqlite)?;
            stmt.query_map(params![individual_id.to_string()], read_evidence_row)
                .map_err(map_sqlite)?
                .map(|row| build_evidence(row.map_err(map_sqlite)?))
                .collect::<Result<Vec<_>, EvidenceError>>()?
        };
        let mut redacted = Vec::new();
        for record in records {
            let matches_id = target_evidence_id.is_some_and(|id| id == record.evidence_id);
            let matches_discord = discord_message_id.is_some_and(|mid| {
                record
                    .payload
                    .get("discord")
                    .and_then(|d| d.get("message_id"))
                    .and_then(serde_json::Value::as_u64)
                    == Some(mid)
            });
            if !matches_id && !matches_discord {
                continue;
            }
            if let Some(requester) = requester_user_id {
                let owner = record
                    .payload
                    .get("discord")
                    .and_then(|d| d.get("user_id"))
                    .and_then(serde_json::Value::as_u64);
                if owner != Some(requester) {
                    return Err(invalid("requester does not own the target utterance"));
                }
            }
            if redact_record_in(&tx, &record, discord_message_id, reason, now)? {
                redacted.push(record.evidence_id);
            }
        }
        tx.commit().map_err(map_sqlite)?;
        Ok(redacted)
    }

    /// Redact Discord non-operator / ephemeral `UserUtterance` records that exceed
    /// `max_age_millis` or `max_utterances_per_source`.
    pub fn enforce_social_retention(
        &self,
        individual_id: IndividualId,
        max_age_millis: u64,
        max_utterances_per_source: usize,
    ) -> Result<Vec<EvidenceId>, EvidenceError> {
        let now = self.clock().now_utc();
        let max_age_i64 = i64::try_from(max_age_millis).unwrap_or(i64::MAX);
        let mut conn = self.conn().map_err(|e| EvidenceError::Backend {
            message: e.to_string(),
        })?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite)?;
        if !individual_exists(&tx, individual_id)? {
            return Err(EvidenceError::IndividualNotFound(individual_id));
        }
        let records = {
            let mut stmt = tx
                .prepare(&format!(
                    "SELECT {EVIDENCE_COLUMNS} FROM evidence_records
                     WHERE individual_id = ?1 AND kind = 'user_utterance'
                     ORDER BY rowid DESC"
                ))
                .map_err(map_sqlite)?;
            stmt.query_map(params![individual_id.to_string()], read_evidence_row)
                .map_err(map_sqlite)?
                .map(|row| build_evidence(row.map_err(map_sqlite)?))
                .collect::<Result<Vec<_>, EvidenceError>>()?
        };
        let mut counts: HashMap<String, usize> = HashMap::new();
        let mut redacted = Vec::new();
        for record in records {
            if record
                .payload
                .get("redacted")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
                || record.payload.get("text").is_none()
            {
                continue;
            }
            let is_other_discord = record
                .payload
                .get("discord")
                .and_then(|d| d.get("is_operator"))
                .and_then(serde_json::Value::as_bool)
                == Some(false);
            if record.retention_class != RetentionClass::Ephemeral && !is_other_discord {
                continue;
            }
            let age = now
                .unix_millis()
                .saturating_sub(record.created_at.unix_millis());
            let expired = age > max_age_i64;
            let source_key = record.source.source_id.clone().unwrap_or_default();
            let over_limit = if !expired {
                let count = counts.entry(source_key).or_insert(0);
                *count += 1;
                *count > max_utterances_per_source
            } else {
                false
            };
            if (expired || over_limit)
                && redact_record_in(&tx, &record, None, "retention_limit_expired", now)?
            {
                redacted.push(record.evidence_id);
            }
        }
        tx.commit().map_err(map_sqlite)?;
        Ok(redacted)
    }
}

#[cfg(test)]
mod social_retention_tests {
    use super::*;
    use crate::store::StoreConfig;
    use kamimusuhi_core::continuity::{ContinuityKernel, ContinuityStore, NewIndividual};
    use kamimusuhi_core::ids::{BootId, CommitId, NodeId, ProposalId};
    use kamimusuhi_core::mutation::{
        MutationDomain, MutationOperation, MutationPolicyV0, MutationProposal,
    };
    use kamimusuhi_testkit::{FixedClock, FixedIdGenerator};
    use std::sync::Arc;
    use std::time::Duration;

    fn setup() -> (
        tempfile::TempDir,
        Arc<FixedClock>,
        SqliteStore,
        IndividualId,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kamimusuhi.sqlite");
        let clock = Arc::new(FixedClock::baseline());
        let store = SqliteStore::open(
            &path,
            &StoreConfig::default(),
            clock.clone(),
            Arc::new(FixedIdGenerator::new(1)),
        )
        .unwrap();
        let individual_id = IndividualId::from_u128(0xA1);
        store
            .create_individual(NewIndividual {
                individual_id,
                root_commit_id: CommitId::from_u128(0xC0),
                node_id: NodeId::from_u128(0x0E),
                boot_id: BootId::from_u128(0xB1),
            })
            .unwrap();
        (dir, clock, store, individual_id)
    }

    #[test]
    fn redacts_discord_utterance_marks_derived_and_invalidates_state_records() {
        let (_dir, clock, store, individual_id) = setup();
        let session_id = SessionId::from_u128(0x51);
        let turn_id = TurnId::from_u128(0x71);
        store
            .open_session(NewSession {
                session_id,
                individual_id,
            })
            .unwrap();
        store
            .record_turn(NewTurn {
                turn_id,
                session_id,
                individual_id,
                sequence: 0,
            })
            .unwrap();
        let user_ev = EvidenceId::from_u128(0xE1);
        store
            .append(NewEvidence {
                evidence_id: user_ev,
                individual_id,
                session_id: Some(session_id),
                turn_id: Some(turn_id),
                kind: EvidenceKind::UserUtterance,
                origin_class: OriginClass::Reported,
                payload: serde_json::json!({
                    "text": "秘密のメモを削除して",
                    "discord": {"message_id": 9001_u64, "user_id": 20_u64, "is_operator": false}
                }),
                source: EvidenceSource {
                    source_id: Some("text-chat:discord-user".to_owned()),
                    source_sequence: Some(0),
                    content_digest: Some("sha256:abc".to_owned()),
                },
                retention_class: RetentionClass::Ephemeral,
            })
            .unwrap();
        let agent_ev = EvidenceId::from_u128(0xE2);
        store
            .append_with_links(
                NewEvidence {
                    evidence_id: agent_ev,
                    individual_id,
                    session_id: Some(session_id),
                    turn_id: Some(turn_id),
                    kind: EvidenceKind::AgentUtterance,
                    origin_class: OriginClass::Observed,
                    payload: serde_json::json!({"text": "承知しました"}),
                    source: EvidenceSource {
                        source_id: Some("text-chat:discord-user".to_owned()),
                        source_sequence: Some(1),
                        content_digest: None,
                    },
                    retention_class: RetentionClass::Standard,
                },
                &[NewEvidenceLink {
                    from_evidence_id: agent_ev,
                    to_evidence_id: user_ev,
                    relation: EvidenceRelation::DerivedFrom,
                }],
            )
            .unwrap();

        // Activate a state record citing user_ev
        let writer = store
            .claim_writer_epoch(
                individual_id,
                NodeId::from_u128(0x0E),
                BootId::from_u128(0xB1),
            )
            .unwrap();
        let head = store.load_head(individual_id).unwrap();
        let kernel = ContinuityKernel::new(store, MutationPolicyV0, clock);
        kernel
            .submit(&MutationProposal {
                proposal_id: ProposalId::from_u128(0xF1),
                individual_id,
                domain: MutationDomain::Episodic,
                operation: MutationOperation::Capture,
                subject_key: Some("discord-user".to_owned()),
                candidate: serde_json::json!({"note": "user note"}),
                expected_head: head.expected(),
                evidence_refs: vec![user_ev],
                supersedes: None,
                origin_class: OriginClass::Reported,
                requested_by: writer,
                policy_version: MutationPolicyV0::VERSION,
                idempotency_key: "redact-test-1".to_owned(),
                created_at: UtcTimestamp::from_unix_millis(1000),
            })
            .unwrap();
        let store = kernel.store();

        // Wrong requester is refused without modifying evidence.
        assert!(
            store
                .redact_discord_utterance(individual_id, Some(9001), None, Some(99), "delete")
                .is_err()
        );
        assert!(
            store
                .get(user_ev)
                .unwrap()
                .unwrap()
                .payload
                .get("text")
                .is_some()
        );

        // Matching requester redacts text, marks derived AgentUtterance corrected, invalidates memory.
        let redacted = store
            .redact_discord_utterance(individual_id, Some(9001), None, Some(20), "user_request")
            .unwrap();
        assert_eq!(redacted, vec![user_ev]);
        let scrubbed = store.get(user_ev).unwrap().unwrap();
        assert!(scrubbed.payload.get("text").is_none());
        assert_eq!(scrubbed.payload["redacted"], serde_json::json!(true));
        let facts = store.facts(&[user_ev, agent_ev]).unwrap();
        assert!(facts.get(user_ev).unwrap().is_corrected);
        assert!(facts.get(agent_ev).unwrap().is_corrected);

        let conn = store.conn().unwrap();
        let lifecycle: String = conn
            .query_row(
                "SELECT lifecycle_state FROM state_records WHERE individual_id = ?1",
                params![individual_id.to_string()],
                |r| r.get(0),
            )
            .unwrap();
        drop(conn);
        assert_eq!(lifecycle, "invalidated");

        // Second identical call is idempotent and returns empty.
        assert!(
            store
                .redact_discord_utterance(individual_id, Some(9001), None, Some(20), "user_request")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn enforces_retention_ttl_and_per_source_cap_without_scrubbing_operator() {
        let (_dir, clock, store, individual_id) = setup();
        let op_ev = EvidenceId::from_u128(0x101);
        store
            .append(NewEvidence {
                evidence_id: op_ev,
                individual_id,
                session_id: None,
                turn_id: None,
                kind: EvidenceKind::UserUtterance,
                origin_class: OriginClass::Reported,
                payload: serde_json::json!({
                    "text": "operator message",
                    "discord": {"message_id": 1_u64, "user_id": 10_u64, "is_operator": true}
                }),
                source: EvidenceSource {
                    source_id: Some("text-chat:eightman".to_owned()),
                    source_sequence: None,
                    content_digest: None,
                },
                retention_class: RetentionClass::Standard,
            })
            .unwrap();

        let other_1 = EvidenceId::from_u128(0x201);
        let other_2 = EvidenceId::from_u128(0x202);
        let other_3 = EvidenceId::from_u128(0x203);
        for (i, id) in [other_1, other_2, other_3].into_iter().enumerate() {
            if i == 1 {
                clock.advance(Duration::from_secs(100));
            } else if i == 2 {
                clock.advance(Duration::from_secs(5));
            }
            store
                .append(NewEvidence {
                    evidence_id: id,
                    individual_id,
                    session_id: None,
                    turn_id: None,
                    kind: EvidenceKind::UserUtterance,
                    origin_class: OriginClass::Reported,
                    payload: serde_json::json!({
                        "text": format!("msg {i}"),
                        "discord": {"message_id": (100 + i) as u64, "user_id": 20_u64, "is_operator": false}
                    }),
                    source: EvidenceSource {
                        source_id: Some("text-chat:discord-other".to_owned()),
                        source_sequence: Some(i as u64),
                        content_digest: None,
                    },
                    retention_class: RetentionClass::Ephemeral,
                })
                .unwrap();
        }

        // TTL=50s (other_1 is 105s old -> expired), max_utterances_per_source=1 (other_2 exceeds cap, other_3 stays)
        let redacted = store
            .enforce_social_retention(individual_id, 50_000, 1)
            .unwrap();
        assert_eq!(redacted.len(), 2);
        assert!(redacted.contains(&other_1));
        assert!(redacted.contains(&other_2));
        assert!(
            store
                .get(other_1)
                .unwrap()
                .unwrap()
                .payload
                .get("text")
                .is_none()
        );
        assert!(
            store
                .get(other_2)
                .unwrap()
                .unwrap()
                .payload
                .get("text")
                .is_none()
        );
        assert!(
            store
                .get(other_3)
                .unwrap()
                .unwrap()
                .payload
                .get("text")
                .is_some()
        );
        assert!(
            store
                .get(op_ev)
                .unwrap()
                .unwrap()
                .payload
                .get("text")
                .is_some()
        );
    }
}
