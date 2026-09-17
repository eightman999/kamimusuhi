//! The C0 derived lane: proposals, activations, the movable head and
//! evaluation rows.
//!
//! This lane is *not* canonical state. Canonical history (evidence, commits,
//! state_records) is strictly forward-only; this lane exists precisely
//! because improvement needs the opposite — a staged proposal lifecycle and
//! a head that can move back. Every write here is still atomic and durable,
//! and every lifecycle transition is also narrated into canonical evidence
//! by the runtime, so the immutable record always says what the lane did.

use kamimusuhi_core::c0::{
    Activation, C0Error, ImprovementProposal, OperativeView, ProposalStatus, apply_proposal,
};
use kamimusuhi_core::evidence::EvidenceRecord;
use kamimusuhi_core::ids::{
    C0ActivationId, C0EvaluationId, C0ProposalId, EvidenceId, IndividualId, TurnId,
};
use kamimusuhi_core::time::UtcTimestamp;
use rusqlite::{OptionalExtension, params};

use crate::evidence::EVIDENCE_COLUMNS;
use crate::store::SqliteStore;

/// A recorded evaluation row.
#[derive(Debug, Clone, PartialEq)]
pub struct C0Evaluation {
    pub evaluation_id: C0EvaluationId,
    pub individual_id: IndividualId,
    pub scope: String,
    pub subject_key: Option<String>,
    pub turn_id: Option<TurnId>,
    pub proposal_id: Option<C0ProposalId>,
    pub metrics: serde_json::Value,
    pub evaluator: String,
    pub created_at: UtcTimestamp,
}

/// The outcome of a rollback: which activation was crossed and where the
/// head now rests. `None` when the head was already at defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RollbackOutcome {
    pub rolled_back_seq: u64,
    pub restored_seq: u64,
    pub proposal_id: C0ProposalId,
}

fn backend(err: rusqlite::Error) -> C0Error {
    C0Error::Backend {
        message: err.to_string(),
    }
}

fn proposal_from_row(row: &rusqlite::Row<'_>) -> Result<ImprovementProposal, rusqlite::Error> {
    let proposal_id: String = row.get(0)?;
    let individual_id: String = row.get(1)?;
    let kind: String = row.get(2)?;
    let status: String = row.get(9)?;
    Ok(ImprovementProposal {
        proposal_id: proposal_id
            .parse()
            .map_err(|_| rusqlite::Error::InvalidParameterName("bad c0 proposal id".to_owned()))?,
        individual_id: individual_id
            .parse()
            .map_err(|_| rusqlite::Error::InvalidParameterName("bad individual id".to_owned()))?,
        kind: kind
            .parse()
            .map_err(|e: kamimusuhi_core::c0::UnknownImprovementKind| {
                rusqlite::Error::InvalidParameterName(e.to_string())
            })?,
        target: row.get(3)?,
        target_key: row.get(4)?,
        old_value: row
            .get::<_, Option<String>>(5)?
            .map(|s| serde_json::from_str(&s).unwrap_or(serde_json::Value::Null)),
        proposed_value: serde_json::from_str(&row.get::<_, String>(6)?)
            .unwrap_or(serde_json::Value::Null),
        evidence_refs: serde_json::from_str(&row.get::<_, String>(7)?).unwrap_or_default(),
        expected_effect: row.get(8)?,
        status: status
            .parse()
            .map_err(|e: kamimusuhi_core::c0::UnknownProposalStatus| {
                rusqlite::Error::InvalidParameterName(e.to_string())
            })?,
        rejection_reason: row.get(10)?,
        reflection_id: row
            .get::<_, Option<String>>(11)?
            .map(|s| s.parse().unwrap_or(EvidenceId::from_u128(0))),
        created_at: UtcTimestamp::from_unix_millis(row.get(12)?),
        decided_at: row
            .get::<_, Option<i64>>(13)?
            .map(UtcTimestamp::from_unix_millis),
        activation_seq: row.get::<_, Option<i64>>(14)?.map(|v| v as u64),
        risk: row.get(15)?,
        confidence: row.get(16)?,
    })
}

const PROPOSAL_COLUMNS: &str = "proposal_id, individual_id, proposal_type, target, target_key,
     old_value_json, proposed_value_json, evidence_refs_json, expected_effect, status,
     rejection_reason, reflection_id, created_at, decided_at, activation_seq, risk, confidence";

fn activation_from_row(row: &rusqlite::Row<'_>) -> Result<Activation, rusqlite::Error> {
    let activation_id: String = row.get(1)?;
    let individual_id: String = row.get(2)?;
    let proposal_id: String = row.get(3)?;
    Ok(Activation {
        activation_seq: row.get::<_, i64>(0)? as u64,
        activation_id: activation_id
            .parse()
            .map_err(|_| rusqlite::Error::InvalidParameterName("bad activation id".to_owned()))?,
        individual_id: individual_id
            .parse()
            .map_err(|_| rusqlite::Error::InvalidParameterName("bad individual id".to_owned()))?,
        proposal_id: proposal_id
            .parse()
            .map_err(|_| rusqlite::Error::InvalidParameterName("bad c0 proposal id".to_owned()))?,
        predecessor_seq: row.get::<_, i64>(4)? as u64,
        view: OperativeView {
            params: serde_json::from_str(&row.get::<_, String>(5)?)
                .map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?,
            self_model: serde_json::from_str(&row.get::<_, String>(6)?)
                .map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))?,
        },
        rolled_back_at: row
            .get::<_, Option<i64>>(7)?
            .map(UtcTimestamp::from_unix_millis),
        created_at: UtcTimestamp::from_unix_millis(row.get(8)?),
    })
}

const ACTIVATION_COLUMNS: &str = "activation_seq, activation_id, individual_id, proposal_id,
     predecessor_seq, params_json, self_json, rolled_back_at, created_at";

impl SqliteStore {
    /// Insert a pending proposal. Returns `false` when the id already exists —
    /// intake is idempotent, and a duplicate submission is not a new proposal.
    pub fn c0_insert_proposal(&self, proposal: &ImprovementProposal) -> Result<bool, C0Error> {
        let conn = self.conn().map_err(|e| C0Error::Backend {
            message: e.to_string(),
        })?;
        let changed = conn
            .execute(
                &format!(
                    "INSERT OR IGNORE INTO c0_proposals ({PROPOSAL_COLUMNS}) \
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)"
                ),
                params![
                    proposal.proposal_id.to_string(),
                    proposal.individual_id.to_string(),
                    proposal.kind.as_str(),
                    proposal.target,
                    proposal.target_key,
                    proposal.old_value.as_ref().map(|v| v.to_string()),
                    proposal.proposed_value.to_string(),
                    serde_json::to_string(&proposal.evidence_refs)
                        .unwrap_or_else(|_| "[]".to_owned()),
                    proposal.expected_effect,
                    proposal.status.as_str(),
                    proposal.rejection_reason,
                    proposal.reflection_id.map(|id| id.to_string()),
                    proposal.created_at.unix_millis(),
                    proposal.decided_at.map(|t| t.unix_millis()),
                    proposal.activation_seq.map(|s| s as i64),
                    proposal.risk,
                    proposal.confidence,
                ],
            )
            .map_err(backend)?;
        Ok(changed > 0)
    }

    pub fn c0_proposal(
        &self,
        proposal_id: C0ProposalId,
    ) -> Result<Option<ImprovementProposal>, C0Error> {
        let conn = self.conn().map_err(|e| C0Error::Backend {
            message: e.to_string(),
        })?;
        conn.query_row(
            &format!("SELECT {PROPOSAL_COLUMNS} FROM c0_proposals WHERE proposal_id = ?1"),
            params![proposal_id.to_string()],
            proposal_from_row,
        )
        .optional()
        .map_err(backend)
    }

    /// Proposals for an individual, optionally filtered to one status,
    /// ordered by creation for deterministic iteration.
    pub fn c0_proposals(
        &self,
        individual_id: IndividualId,
        status: Option<ProposalStatus>,
    ) -> Result<Vec<ImprovementProposal>, C0Error> {
        let conn = self.conn().map_err(|e| C0Error::Backend {
            message: e.to_string(),
        })?;
        let sql = format!(
            "SELECT {PROPOSAL_COLUMNS} FROM c0_proposals WHERE individual_id = ?1 {} \
             ORDER BY created_at, proposal_id",
            if status.is_some() {
                "AND status = ?2"
            } else {
                ""
            }
        );
        let mut stmt = conn.prepare(&sql).map_err(backend)?;
        let rows = match status {
            Some(status) => stmt
                .query_map(
                    params![individual_id.to_string(), status.as_str()],
                    proposal_from_row,
                )
                .map_err(backend)?,
            None => stmt
                .query_map(params![individual_id.to_string()], proposal_from_row)
                .map_err(backend)?,
        };
        rows.collect::<Result<Vec<_>, _>>().map_err(backend)
    }

    /// Record a decision on a pending proposal. Anything else is a lifecycle
    /// violation: decided rows do not move backward.
    pub fn c0_decide(
        &self,
        proposal_id: C0ProposalId,
        status: ProposalStatus,
        reason: Option<&str>,
        decided_at: UtcTimestamp,
    ) -> Result<(), C0Error> {
        if status == ProposalStatus::Pending {
            return Err(C0Error::MalformedProposal {
                reason: "a decision cannot move a proposal back to pending".to_owned(),
            });
        }
        let conn = self.conn().map_err(|e| C0Error::Backend {
            message: e.to_string(),
        })?;
        let changed = conn
            .execute(
                "UPDATE c0_proposals SET status = ?2, rejection_reason = ?3, decided_at = ?4 \
                 WHERE proposal_id = ?1 AND status = 'pending'",
                params![
                    proposal_id.to_string(),
                    status.as_str(),
                    reason,
                    decided_at.unix_millis()
                ],
            )
            .map_err(backend)?;
        if changed == 0 {
            // Stay on the held connection: calling c0_proposal here would
            // re-lock the store mutex and deadlock.
            let exists: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM c0_proposals WHERE proposal_id = ?1",
                    params![proposal_id.to_string()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(backend)?;
            return match exists {
                Some(_) => Err(C0Error::ProposalNotPending(proposal_id)),
                None => Err(C0Error::ProposalNotFound(proposal_id)),
            };
        }
        Ok(())
    }

    /// The current derived-lane head position. 0 = defaults.
    pub fn c0_head(&self, individual_id: IndividualId) -> Result<u64, C0Error> {
        let conn = self.conn().map_err(|e| C0Error::Backend {
            message: e.to_string(),
        })?;
        conn.query_row(
            "SELECT activation_seq FROM c0_head WHERE individual_id = ?1",
            params![individual_id.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map(|v| v.unwrap_or(0) as u64)
        .map_err(backend)
    }

    pub fn c0_activation(
        &self,
        individual_id: IndividualId,
        seq: u64,
    ) -> Result<Option<Activation>, C0Error> {
        let conn = self.conn().map_err(|e| C0Error::Backend {
            message: e.to_string(),
        })?;
        conn.query_row(
            &format!(
                "SELECT {ACTIVATION_COLUMNS} FROM c0_activations \
                 WHERE individual_id = ?1 AND activation_seq = ?2"
            ),
            params![individual_id.to_string(), seq as i64],
            activation_from_row,
        )
        .optional()
        .map_err(backend)
    }

    /// Every activation in order, including dormant branches and rolled-back
    /// rows — the lane's history, not just its current view.
    pub fn c0_activations(&self, individual_id: IndividualId) -> Result<Vec<Activation>, C0Error> {
        let conn = self.conn().map_err(|e| C0Error::Backend {
            message: e.to_string(),
        })?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {ACTIVATION_COLUMNS} FROM c0_activations \
                 WHERE individual_id = ?1 ORDER BY activation_seq"
            ))
            .map_err(backend)?;
        stmt.query_map(params![individual_id.to_string()], activation_from_row)
            .map_err(backend)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(backend)
    }

    /// The operative view in force: the snapshot at the current head, or
    /// defaults when nothing has ever been activated.
    pub fn c0_view(&self, individual_id: IndividualId) -> Result<OperativeView, C0Error> {
        let head = self.c0_head(individual_id)?;
        if head == 0 {
            return Ok(OperativeView::default());
        }
        self.c0_activation(individual_id, head)?
            .map(|a| a.view)
            .ok_or_else(|| C0Error::CorruptRow(format!("head points at missing activation {head}")))
    }

    /// Apply an accepted proposal as a new activation. One atomic write:
    /// activation row, the proposal's status/activation link, supersession of
    /// other pending proposals on the same target, and the head move.
    ///
    /// The gate decision must already have been made; this writes what an
    /// `Accept` verdict concluded.
    pub fn c0_apply_activation(
        &self,
        proposal_id: C0ProposalId,
        activation_id: C0ActivationId,
        now: UtcTimestamp,
    ) -> Result<Activation, C0Error> {
        let mut conn = self.conn().map_err(|e| C0Error::Backend {
            message: e.to_string(),
        })?;
        let tx = conn.transaction().map_err(backend)?;

        let proposal = tx
            .query_row(
                &format!("SELECT {PROPOSAL_COLUMNS} FROM c0_proposals WHERE proposal_id = ?1"),
                params![proposal_id.to_string()],
                proposal_from_row,
            )
            .optional()
            .map_err(backend)?
            .ok_or(C0Error::ProposalNotFound(proposal_id))?;
        if proposal.status != ProposalStatus::Pending {
            return Err(C0Error::ProposalNotPending(proposal_id));
        }

        let head: u64 = tx
            .query_row(
                "SELECT activation_seq FROM c0_head WHERE individual_id = ?1",
                params![proposal.individual_id.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(backend)?
            .unwrap_or(0) as u64;

        let view_at_head = if head == 0 {
            OperativeView::default()
        } else {
            tx.query_row(
                "SELECT params_json, self_json FROM c0_activations \
                 WHERE individual_id = ?1 AND activation_seq = ?2",
                params![proposal.individual_id.to_string(), head as i64],
                |row| {
                    Ok(OperativeView {
                        params: serde_json::from_str(&row.get::<_, String>(0)?).unwrap_or_default(),
                        self_model: serde_json::from_str(&row.get::<_, String>(1)?)
                            .unwrap_or_default(),
                    })
                },
            )
            .optional()
            .map_err(backend)?
            .ok_or_else(|| {
                C0Error::CorruptRow(format!("head points at missing activation {head}"))
            })?
        };

        let next_view = apply_proposal(&view_at_head, &proposal);
        // Sequence numbers keep counting even across a rollback: a branch
        // reuses no number, so every activation stays uniquely addressable.
        let next_seq: u64 = tx
            .query_row(
                "SELECT COALESCE(MAX(activation_seq), 0) + 1 FROM c0_activations \
                 WHERE individual_id = ?1",
                params![proposal.individual_id.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .map_err(backend)? as u64;

        let activation = Activation {
            activation_seq: next_seq,
            activation_id,
            individual_id: proposal.individual_id,
            proposal_id,
            predecessor_seq: head,
            view: next_view,
            rolled_back_at: None,
            created_at: now,
        };
        tx.execute(
            &format!(
                "INSERT INTO c0_activations ({ACTIVATION_COLUMNS}) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,NULL,?8)"
            ),
            params![
                next_seq as i64,
                activation_id.to_string(),
                proposal.individual_id.to_string(),
                proposal_id.to_string(),
                head as i64,
                serde_json::to_string(&activation.view.params).unwrap_or_else(|_| "{}".to_owned()),
                serde_json::to_string(&activation.view.self_model)
                    .unwrap_or_else(|_| "{}".to_owned()),
                now.unix_millis(),
            ],
        )
        .map_err(backend)?;

        tx.execute(
            "UPDATE c0_proposals SET status = 'accepted', decided_at = ?2, activation_seq = ?3 \
             WHERE proposal_id = ?1",
            params![proposal_id.to_string(), now.unix_millis(), next_seq as i64],
        )
        .map_err(backend)?;

        // A pending proposal on the same target is superseded: the question
        // it asked has been answered by a newer accepted one.
        tx.execute(
            "UPDATE c0_proposals SET status = 'superseded', decided_at = ?3 \
             WHERE individual_id = ?1 AND target = ?2 AND status = 'pending' \
             AND proposal_id != ?4 \
             AND (target_key IS ?5 OR target_key = ?5)",
            params![
                proposal.individual_id.to_string(),
                proposal.target,
                now.unix_millis(),
                proposal_id.to_string(),
                proposal.target_key,
            ],
        )
        .map_err(backend)?;

        tx.execute(
            "INSERT INTO c0_head (individual_id, activation_seq, updated_at) \
             VALUES (?1, ?2, ?3) \
             ON CONFLICT(individual_id) DO UPDATE SET activation_seq = ?2, updated_at = ?3",
            params![
                proposal.individual_id.to_string(),
                next_seq as i64,
                now.unix_millis()
            ],
        )
        .map_err(backend)?;

        tx.commit().map_err(backend)?;
        Ok(activation)
    }

    /// Move the head back over the most recent activation. The crossed
    /// activation is marked, not deleted — history is kept.
    pub fn c0_rollback(
        &self,
        individual_id: IndividualId,
        now: UtcTimestamp,
    ) -> Result<RollbackOutcome, C0Error> {
        let mut conn = self.conn().map_err(|e| C0Error::Backend {
            message: e.to_string(),
        })?;
        let tx = conn.transaction().map_err(backend)?;

        let head: u64 = tx
            .query_row(
                "SELECT activation_seq FROM c0_head WHERE individual_id = ?1",
                params![individual_id.to_string()],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(backend)?
            .unwrap_or(0) as u64;
        if head == 0 {
            return Err(C0Error::NothingToRollBack);
        }
        let crossed = tx
            .query_row(
                "SELECT proposal_id, predecessor_seq FROM c0_activations \
                 WHERE individual_id = ?1 AND activation_seq = ?2",
                params![individual_id.to_string(), head as i64],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64)),
            )
            .optional()
            .map_err(backend)?
            .ok_or_else(|| {
                C0Error::CorruptRow(format!("head points at missing activation {head}"))
            })?;
        let proposal_id: C0ProposalId = crossed
            .0
            .parse()
            .map_err(|_| C0Error::CorruptRow("bad proposal id in activation".to_owned()))?;

        tx.execute(
            "UPDATE c0_activations SET rolled_back_at = ?3 \
             WHERE individual_id = ?1 AND activation_seq = ?2",
            params![individual_id.to_string(), head as i64, now.unix_millis()],
        )
        .map_err(backend)?;
        tx.execute(
            "UPDATE c0_head SET activation_seq = ?2, updated_at = ?3 WHERE individual_id = ?1",
            params![
                individual_id.to_string(),
                crossed.1 as i64,
                now.unix_millis()
            ],
        )
        .map_err(backend)?;
        tx.commit().map_err(backend)?;

        Ok(RollbackOutcome {
            rolled_back_seq: head,
            restored_seq: crossed.1,
            proposal_id,
        })
    }

    pub fn c0_record_evaluation(&self, evaluation: &C0Evaluation) -> Result<(), C0Error> {
        let conn = self.conn().map_err(|e| C0Error::Backend {
            message: e.to_string(),
        })?;
        conn.execute(
            "INSERT OR IGNORE INTO c0_evaluations \
             (evaluation_id, individual_id, scope, subject_key, turn_id, proposal_id, \
              metrics_json, evaluator, created_at) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                evaluation.evaluation_id.to_string(),
                evaluation.individual_id.to_string(),
                evaluation.scope,
                evaluation.subject_key,
                evaluation.turn_id.map(|id| id.to_string()),
                evaluation.proposal_id.map(|id| id.to_string()),
                evaluation.metrics.to_string(),
                evaluation.evaluator,
                evaluation.created_at.unix_millis(),
            ],
        )
        .map_err(backend)?;
        Ok(())
    }

    pub fn c0_evaluations(
        &self,
        individual_id: IndividualId,
        scope: Option<&str>,
        limit: usize,
    ) -> Result<Vec<C0Evaluation>, C0Error> {
        let conn = self.conn().map_err(|e| C0Error::Backend {
            message: e.to_string(),
        })?;
        let sql = "SELECT evaluation_id, individual_id, scope, subject_key, turn_id, \
             proposal_id, metrics_json, evaluator, created_at FROM c0_evaluations \
             WHERE individual_id = ?1 AND (?2 IS NULL OR scope = ?2) \
             ORDER BY created_at DESC, evaluation_id LIMIT ?3";
        let mut stmt = conn.prepare(sql).map_err(backend)?;
        let map = |row: &rusqlite::Row<'_>| -> Result<C0Evaluation, rusqlite::Error> {
            Ok(C0Evaluation {
                evaluation_id: row
                    .get::<_, String>(0)?
                    .parse()
                    .unwrap_or(C0EvaluationId::from_u128(0)),
                individual_id: row
                    .get::<_, String>(1)?
                    .parse()
                    .unwrap_or(IndividualId::from_u128(0)),
                scope: row.get(2)?,
                subject_key: row.get(3)?,
                turn_id: row
                    .get::<_, Option<String>>(4)?
                    .map(|s| s.parse().unwrap_or(TurnId::from_u128(0))),
                proposal_id: row
                    .get::<_, Option<String>>(5)?
                    .map(|s| s.parse().unwrap_or(C0ProposalId::from_u128(0))),
                metrics: serde_json::from_str(&row.get::<_, String>(6)?)
                    .unwrap_or(serde_json::Value::Null),
                evaluator: row.get(7)?,
                created_at: UtcTimestamp::from_unix_millis(row.get(8)?),
            })
        };
        let rows = stmt
            .query_map(params![individual_id.to_string(), scope, limit as i64], map)
            .map_err(backend)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(backend)
    }

    /// Canonical evidence for one individual, newest first — the candidate
    /// pool for lexical recall and reflection. Scoring happens in the
    /// runtime; this query only bounds the work.
    pub fn c0_evidence_pool(
        &self,
        individual_id: IndividualId,
        limit: usize,
    ) -> Result<Vec<EvidenceRecord>, C0Error> {
        let conn = self.conn().map_err(|e| C0Error::Backend {
            message: e.to_string(),
        })?;
        let mut stmt = conn
            .prepare(&format!(
                "SELECT {EVIDENCE_COLUMNS} FROM evidence_records \
                 WHERE individual_id = ?1 \
                 ORDER BY rowid DESC LIMIT ?2"
            ))
            .map_err(backend)?;
        stmt.query_map(
            params![individual_id.to_string(), limit as i64],
            crate::evidence::read_evidence_row,
        )
        .map_err(backend)?
        .map(|row| {
            row.and_then(|r| {
                crate::evidence::build_evidence(r)
                    .map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(backend)
    }
}
