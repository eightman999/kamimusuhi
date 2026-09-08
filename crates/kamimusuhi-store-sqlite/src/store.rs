//! `SqliteContinuityStore`: the SQLite-backed implementation of
//! `kamimusuhi_core::continuity::ContinuityStore`.
//!
//! `activate()` (plan §7) is implemented here. `kamimusuhi-core` remains
//! ignorant of SQL: this module is the only place `rusqlite::Row` /
//! `rusqlite::Connection` are used, and `ContinuityError` (not
//! `rusqlite::Error`) is what crosses back out through the trait.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

use kamimusuhi_core::audit::{AuditEvent, AuditEventKind};
use kamimusuhi_core::continuity::{
    ActivationOutcome, ActivationReceipt, CanonicalCommit, ContinuityError, ContinuityHead,
    ContinuityStore, Individual,
};
use kamimusuhi_core::ids::{CommitId, IdGenerator, IndividualId, ProposalId, ReceiptId};
use kamimusuhi_core::mutation::{Disposition, MutationProposal, ReasonCode};
use kamimusuhi_core::time::{UtcTimestamp, WallClock};

use crate::error::StoreError;
use crate::failpoint::{self, Failpoint};
use crate::migrations;
use crate::transaction::{open_in_memory_with_required_pragmas, open_with_required_pragmas};

pub struct SqliteContinuityStore {
    conn: Mutex<Connection>,
    clock: Box<dyn WallClock>,
    id_gen: Box<dyn IdGenerator>,
}

impl SqliteContinuityStore {
    /// Opens (creating if absent) a file-backed database, applies/verifies
    /// the required PRAGMAs, and migrates it to
    /// [`migrations::SUPPORTED_SCHEMA_VERSION`].
    pub fn open(
        path: &Path,
        clock: Box<dyn WallClock>,
        id_gen: Box<dyn IdGenerator>,
    ) -> Result<Self, StoreError> {
        let conn = open_with_required_pragmas(path)?;
        migrations::migrate(&conn)?;
        Ok(SqliteContinuityStore {
            conn: Mutex::new(conn),
            clock,
            id_gen,
        })
    }

    /// In-memory database for unit/integration tests. Not used for restart
    /// tests, which need a real file so a second process/connection can
    /// reopen it.
    pub fn open_in_memory(
        clock: Box<dyn WallClock>,
        id_gen: Box<dyn IdGenerator>,
    ) -> Result<Self, StoreError> {
        let conn = open_in_memory_with_required_pragmas()?;
        migrations::migrate(&conn)?;
        Ok(SqliteContinuityStore {
            conn: Mutex::new(conn),
            clock,
            id_gen,
        })
    }

    /// Creates a brand-new individual with a generation-0 root commit, as
    /// one atomic transaction (plan §15.2 "root individual creation
    /// atomicity"). The root commit has no predecessor and no proposal.
    pub fn create_root_individual(&self) -> Result<Individual, ContinuityError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        let individual_id = IndividualId::new(self.id_gen.as_ref());
        let root_commit_id = CommitId::new(self.id_gen.as_ref());
        let now = self.clock.now_utc();

        let result: Result<Individual, StoreError> = (|| {
            conn.execute_batch("BEGIN IMMEDIATE;")?;
            let inner = (|| -> Result<Individual, StoreError> {
                conn.execute(
                    "INSERT INTO individuals (individual_id, created_at, root_commit_id)
                     VALUES (?1, ?2, ?3)",
                    params![
                        individual_id.to_string(),
                        now.to_rfc3339(),
                        root_commit_id.to_string()
                    ],
                )?;
                conn.execute(
                    "INSERT INTO canonical_commits
                        (commit_id, individual_id, generation, predecessor_commit_id, proposal_id, created_at)
                     VALUES (?1, ?2, 0, NULL, NULL, ?3)",
                    params![
                        root_commit_id.to_string(),
                        individual_id.to_string(),
                        now.to_rfc3339()
                    ],
                )?;
                conn.execute(
                    "INSERT INTO continuity_heads (individual_id, commit_id, generation, updated_at)
                     VALUES (?1, ?2, 0, ?3)",
                    params![
                        individual_id.to_string(),
                        root_commit_id.to_string(),
                        now.to_rfc3339()
                    ],
                )?;
                insert_audit_event(
                    &conn,
                    &AuditEvent {
                        individual_id: Some(individual_id),
                        kind: AuditEventKind::IndividualCreated,
                        proposal_id: None,
                        commit_id: Some(root_commit_id),
                        detail: serde_json::json!({}),
                        created_at: now,
                    },
                )?;
                Ok(Individual {
                    individual_id,
                    created_at: now,
                    root_commit_id,
                })
            })();
            match inner {
                Ok(v) => {
                    conn.execute_batch("COMMIT;")?;
                    Ok(v)
                }
                Err(e) => {
                    let _ = conn.execute_batch("ROLLBACK;");
                    Err(e)
                }
            }
        })();

        result.map_err(ContinuityError::from)
    }

    /// Read-only introspection used by `kamimusuhi-runtime inspect` and by
    /// restart tests. See [`crate::recovery`].
    pub fn list_individuals(&self) -> Result<Vec<Individual>, StoreError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        crate::recovery::list_individuals(&conn)
    }

    pub fn count_commits_for_individual(
        &self,
        individual_id: IndividualId,
    ) -> Result<u64, StoreError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        crate::recovery::count_commits_for_individual(&conn, individual_id)
    }

    /// Test/inspection helper: reads back a canonical commit row.
    pub fn load_commit(&self, commit_id: CommitId) -> Result<Option<CanonicalCommit>, StoreError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        load_commit(&conn, commit_id)
    }

    /// Test-only: arm a failpoint for the *next* matching crash window
    /// hit by `activate()` on this store's connection thread. See
    /// [`crate::failpoint`].
    pub fn arm_failpoint(&self, fp: Failpoint) {
        failpoint::arm(fp);
    }
}

impl ContinuityStore for SqliteContinuityStore {
    fn load_head(&self, individual: IndividualId) -> Result<ContinuityHead, ContinuityError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        load_head_row(&conn, individual).map_err(ContinuityError::from)
    }

    fn activate(&self, proposal: MutationProposal) -> Result<ActivationOutcome, ContinuityError> {
        let conn = self.conn.lock().expect("store mutex poisoned");
        activate_inner(&conn, self.id_gen.as_ref(), self.clock.as_ref(), proposal)
            .map_err(ContinuityError::from)
    }
}

fn load_head_row(
    conn: &Connection,
    individual: IndividualId,
) -> Result<ContinuityHead, StoreError> {
    conn.query_row(
        "SELECT individual_id, commit_id, generation, updated_at
         FROM continuity_heads WHERE individual_id = ?1",
        params![individual.to_string()],
        |row| {
            let individual_id: String = row.get(0)?;
            let commit_id: String = row.get(1)?;
            let generation: i64 = row.get(2)?;
            let updated_at: String = row.get(3)?;
            Ok((individual_id, commit_id, generation, updated_at))
        },
    )
    .optional()?
    .ok_or_else(|| StoreError::InvariantViolation(format!("no continuity head for {individual}")))
    .and_then(|(individual_id, commit_id, generation, updated_at)| {
        Ok(ContinuityHead {
            individual_id: individual_id.parse()?,
            commit_id: commit_id.parse()?,
            generation: generation as u64,
            updated_at: UtcTimestamp::parse_rfc3339(&updated_at)?,
        })
    })
}

fn insert_audit_event(conn: &Connection, event: &AuditEvent) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO audit_events (individual_id, kind, proposal_id, commit_id, detail_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            event.individual_id.map(|i| i.to_string()),
            serde_json::to_string(&event.kind)?,
            event.proposal_id.map(|p| p.to_string()),
            event.commit_id.map(|c| c.to_string()),
            serde_json::to_string(&event.detail)?,
            event.created_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

/// Looks up a prior proposal (by idempotency key) together with its
/// decision and, if accepted, its receipt. This is the idempotent-replay
/// path: retrying activation with the same idempotency key must return
/// the exact same outcome rather than doing any work again (plan §7.2
/// invariant 3).
fn find_prior_outcome(
    conn: &Connection,
    idempotency_key: &str,
) -> Result<Option<ActivationOutcome>, StoreError> {
    let prior_proposal_id: Option<String> = conn
        .query_row(
            "SELECT proposal_id FROM mutation_proposals WHERE idempotency_key = ?1",
            params![idempotency_key],
            |row| row.get(0),
        )
        .optional()?;

    let Some(prior_proposal_id) = prior_proposal_id else {
        return Ok(None);
    };
    let prior_proposal_id: ProposalId = prior_proposal_id.parse()?;

    let decision: Option<(String, String)> = conn
        .query_row(
            "SELECT disposition, reason_code FROM mutation_decisions WHERE proposal_id = ?1",
            params![prior_proposal_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;

    let Some((disposition_json, reason_code_json)) = decision else {
        // Proposal row exists but decision has not been recorded yet.
        // Under our single-writer, single-transaction activate(), this
        // should not happen; treat it as "no prior outcome" rather than
        // guessing (plan §20: never guess success/failure for UNKNOWN).
        return Ok(None);
    };

    let disposition: Disposition = serde_json::from_str(&disposition_json)?;
    let reason_code: ReasonCode = serde_json::from_str(&reason_code_json)?;

    match disposition {
        Disposition::Accept => {
            let receipt = load_receipt_by_proposal(conn, prior_proposal_id)?.ok_or_else(|| {
                StoreError::InvariantViolation(format!(
                    "accepted proposal {prior_proposal_id} has no receipt"
                ))
            })?;
            Ok(Some(ActivationOutcome::Activated(receipt)))
        }
        Disposition::Reject | Disposition::Quarantine => Ok(Some(ActivationOutcome::Rejected {
            proposal_id: prior_proposal_id,
            reason_code,
        })),
    }
}

fn load_receipt_by_proposal(
    conn: &Connection,
    proposal_id: ProposalId,
) -> Result<Option<ActivationReceipt>, StoreError> {
    conn.query_row(
        "SELECT receipt_id, proposal_id, commit_id, predecessor_commit_id, generation, created_at
         FROM activation_receipts WHERE proposal_id = ?1",
        params![proposal_id.to_string()],
        |row| {
            let receipt_id: String = row.get(0)?;
            let proposal_id: String = row.get(1)?;
            let commit_id: String = row.get(2)?;
            let predecessor_commit_id: Option<String> = row.get(3)?;
            let generation: i64 = row.get(4)?;
            let created_at: String = row.get(5)?;
            Ok((
                receipt_id,
                proposal_id,
                commit_id,
                predecessor_commit_id,
                generation,
                created_at,
            ))
        },
    )
    .optional()?
    .map(
        |(receipt_id, proposal_id, commit_id, predecessor_commit_id, generation, created_at)| {
            Ok(ActivationReceipt {
                receipt_id: receipt_id.parse()?,
                proposal_id: proposal_id.parse()?,
                commit_id: commit_id.parse()?,
                predecessor_commit_id: predecessor_commit_id.map(|s| s.parse()).transpose()?,
                generation: generation as u64,
                created_at: UtcTimestamp::parse_rfc3339(&created_at)?,
            })
        },
    )
    .transpose()
}

#[allow(clippy::too_many_lines)]
fn activate_inner(
    conn: &Connection,
    id_gen: &dyn IdGenerator,
    clock: &dyn WallClock,
    proposal: MutationProposal,
) -> Result<ActivationOutcome, StoreError> {
    conn.execute_batch("BEGIN IMMEDIATE;")?;

    let inner_result: Result<ActivationOutcome, StoreError> = (|| {
        if failpoint::hit(Failpoint::AfterBegin) {
            return Err(StoreError::Failpoint(Failpoint::AfterBegin.code().into()));
        }

        // Idempotent replay: identical idempotency key never creates a
        // second commit or a second decision.
        if let Some(outcome) = find_prior_outcome(conn, &proposal.idempotency_key)? {
            return Ok(outcome);
        }

        // Persist the proposal itself before deciding, so every decision
        // (accept or reject) is traceable back to exactly one durable
        // proposal row.
        conn.execute(
            "INSERT INTO mutation_proposals
                (proposal_id, individual_id, domain, operation, candidate_json,
                 expected_commit_id, expected_generation, origin_class, requested_by,
                 policy_version, idempotency_key, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                proposal.proposal_id.to_string(),
                proposal.individual_id.to_string(),
                serde_json::to_string(&proposal.domain)?,
                serde_json::to_string(&proposal.operation)?,
                serde_json::to_string(&proposal.candidate)?,
                proposal.expected_commit_id.map(|c| c.to_string()),
                proposal.expected_generation as i64,
                serde_json::to_string(&proposal.origin_class)?,
                proposal.requested_by,
                proposal.policy_version.0,
                proposal.idempotency_key,
                proposal.created_at.to_rfc3339(),
            ],
        )?;
        if failpoint::hit(Failpoint::AfterProposalInsert) {
            return Err(StoreError::Failpoint(
                Failpoint::AfterProposalInsert.code().into(),
            ));
        }

        insert_audit_event(
            conn,
            &AuditEvent {
                individual_id: Some(proposal.individual_id),
                kind: AuditEventKind::MutationProposed,
                proposal_id: Some(proposal.proposal_id),
                commit_id: None,
                detail: serde_json::json!({ "domain": proposal.domain, "operation": proposal.operation }),
                created_at: clock.now_utc(),
            },
        )?;

        let head = load_head_row(conn, proposal.individual_id)?;
        let expected_matches = proposal.expected_generation == head.generation
            && proposal
                .expected_commit_id
                .map(|c| c == head.commit_id)
                .unwrap_or(false);

        if !expected_matches {
            let decision_time = clock.now_utc();
            record_decision(
                conn,
                proposal.proposal_id,
                Disposition::Reject,
                ReasonCode::StalePredecessor,
                decision_time,
            )?;
            insert_audit_event(
                conn,
                &AuditEvent {
                    individual_id: Some(proposal.individual_id),
                    kind: AuditEventKind::MutationRejected,
                    proposal_id: Some(proposal.proposal_id),
                    commit_id: None,
                    detail: serde_json::json!({ "reason_code": ReasonCode::StalePredecessor }),
                    created_at: decision_time,
                },
            )?;
            return Ok(ActivationOutcome::Rejected {
                proposal_id: proposal.proposal_id,
                reason_code: ReasonCode::StalePredecessor,
            });
        }

        let decision_time = clock.now_utc();
        record_decision(
            conn,
            proposal.proposal_id,
            Disposition::Accept,
            ReasonCode::Accepted,
            decision_time,
        )?;

        // Durable state record append point (plan §7.1). No `state_records`
        // table exists in this wave's schema (plan §6.3 is deferred to
        // W2); this failpoint marks the position the future insert will
        // occupy so the crash-window structure matches the plan even
        // though there is not yet a row to write here.
        if failpoint::hit(Failpoint::AfterStateInsert) {
            return Err(StoreError::Failpoint(
                Failpoint::AfterStateInsert.code().into(),
            ));
        }

        let new_generation = head.generation + 1;
        let new_commit_id = CommitId::new(id_gen);
        let commit_time = clock.now_utc();
        conn.execute(
            "INSERT INTO canonical_commits
                (commit_id, individual_id, generation, predecessor_commit_id, proposal_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                new_commit_id.to_string(),
                proposal.individual_id.to_string(),
                new_generation as i64,
                head.commit_id.to_string(),
                proposal.proposal_id.to_string(),
                commit_time.to_rfc3339(),
            ],
        )?;
        if failpoint::hit(Failpoint::AfterCommitRecordInsert) {
            return Err(StoreError::Failpoint(
                Failpoint::AfterCommitRecordInsert.code().into(),
            ));
        }

        // Conditional head update: re-assert the expected predecessor in
        // the WHERE clause so a concurrent writer that somehow slipped
        // past the check above still cannot silently overwrite the head
        // (defense in depth for invariant 2; the primary defense is the
        // `BEGIN IMMEDIATE` writer lock itself).
        let updated = conn.execute(
            "UPDATE continuity_heads
             SET commit_id = ?1, generation = ?2, updated_at = ?3
             WHERE individual_id = ?4 AND commit_id = ?5 AND generation = ?6",
            params![
                new_commit_id.to_string(),
                new_generation as i64,
                commit_time.to_rfc3339(),
                proposal.individual_id.to_string(),
                head.commit_id.to_string(),
                head.generation as i64,
            ],
        )?;
        if updated != 1 {
            return Err(StoreError::InvariantViolation(format!(
                "expected exactly one head row updated for {}, got {updated}",
                proposal.individual_id
            )));
        }
        if failpoint::hit(Failpoint::AfterHeadUpdate) {
            return Err(StoreError::Failpoint(
                Failpoint::AfterHeadUpdate.code().into(),
            ));
        }

        let receipt = ActivationReceipt {
            receipt_id: ReceiptId::new(id_gen),
            proposal_id: proposal.proposal_id,
            commit_id: new_commit_id,
            predecessor_commit_id: Some(head.commit_id),
            generation: new_generation,
            created_at: commit_time,
        };
        conn.execute(
            "INSERT INTO activation_receipts
                (receipt_id, proposal_id, commit_id, predecessor_commit_id, generation, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                receipt.receipt_id.to_string(),
                receipt.proposal_id.to_string(),
                receipt.commit_id.to_string(),
                receipt.predecessor_commit_id.map(|c| c.to_string()),
                receipt.generation as i64,
                receipt.created_at.to_rfc3339(),
            ],
        )?;
        if failpoint::hit(Failpoint::AfterReceiptInsert) {
            return Err(StoreError::Failpoint(
                Failpoint::AfterReceiptInsert.code().into(),
            ));
        }

        insert_audit_event(
            conn,
            &AuditEvent {
                individual_id: Some(proposal.individual_id),
                kind: AuditEventKind::ContinuityActivated,
                proposal_id: Some(proposal.proposal_id),
                commit_id: Some(new_commit_id),
                detail: serde_json::json!({ "generation": new_generation }),
                created_at: commit_time,
            },
        )?;

        Ok(ActivationOutcome::Activated(receipt))
    })();

    match inner_result {
        Ok(outcome) => {
            conn.execute_batch("COMMIT;")?;
            if failpoint::hit(Failpoint::AfterSqlCommitBeforeAck) {
                // The write is already durable at this point; only the
                // in-process acknowledgement to the caller is "lost". A
                // retry with the same proposal/idempotency key must find
                // this exact outcome via `find_prior_outcome` above, not
                // create a second commit.
                return Err(StoreError::Failpoint(
                    Failpoint::AfterSqlCommitBeforeAck.code().into(),
                ));
            }
            Ok(outcome)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(e)
        }
    }
}

fn record_decision(
    conn: &Connection,
    proposal_id: ProposalId,
    disposition: Disposition,
    reason_code: ReasonCode,
    decided_at: UtcTimestamp,
) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO mutation_decisions (proposal_id, disposition, reason_code, decided_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            proposal_id.to_string(),
            serde_json::to_string(&disposition)?,
            serde_json::to_string(&reason_code)?,
            decided_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

/// Reads back a canonical commit row, used by integration tests to
/// assert lineage shape.
pub fn load_commit(
    conn: &Connection,
    commit_id: CommitId,
) -> Result<Option<CanonicalCommit>, StoreError> {
    conn.query_row(
        "SELECT commit_id, individual_id, generation, predecessor_commit_id, proposal_id, created_at
         FROM canonical_commits WHERE commit_id = ?1",
        params![commit_id.to_string()],
        |row| {
            let commit_id: String = row.get(0)?;
            let individual_id: String = row.get(1)?;
            let generation: i64 = row.get(2)?;
            let predecessor_commit_id: Option<String> = row.get(3)?;
            let proposal_id: Option<String> = row.get(4)?;
            let created_at: String = row.get(5)?;
            Ok((
                commit_id,
                individual_id,
                generation,
                predecessor_commit_id,
                proposal_id,
                created_at,
            ))
        },
    )
    .optional()?
    .map(
        |(commit_id, individual_id, generation, predecessor_commit_id, proposal_id, created_at)| {
            Ok(CanonicalCommit {
                commit_id: commit_id.parse()?,
                individual_id: individual_id.parse()?,
                generation: generation as u64,
                predecessor_commit_id: predecessor_commit_id.map(|s| s.parse()).transpose()?,
                proposal_id: proposal_id.map(|s| s.parse()).transpose()?,
                created_at: UtcTimestamp::parse_rfc3339(&created_at)?,
            })
        },
    )
    .transpose()
}

// Re-exported so tests in this crate's `tests/` directory can construct
// domains/operations without reaching into `kamimusuhi_core` for every
// single symbol.
pub use kamimusuhi_core::mutation::{MutationDomain as Domain, MutationOperation as Operation};
