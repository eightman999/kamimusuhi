//! `ContinuityStore` on SQLite: root creation, head recovery, writer fencing
//! and the atomic activation transaction (phase-1 plan §7).

use std::str::FromStr;

use kamimusuhi_core::audit::{AuditEvent, AuditKind};
use kamimusuhi_core::continuity::{
    ActivationOutcome, ActivationReceipt, CanonicalCommit, ContinuityError, ContinuityHead,
    ContinuityStore, ExpectedHead, Generation, Individual, IndividualBootstrap, NewIndividual,
    WriterEpoch, WriterIdentity,
};
use kamimusuhi_core::domain_separation::{SeparationContext, check_domain_separation};
use kamimusuhi_core::ids::{
    AuditEventId, BootId, CommitId, IndividualId, MemoryId, NodeId, PolicyVersion, ProposalId,
    ReceiptId,
};
use kamimusuhi_core::mutation::{Disposition, MutationDecision, MutationProposal, ReasonCode};
use kamimusuhi_core::time::UtcTimestamp;
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};

use crate::error::map_sqlite;
use crate::evidence::{corrected_by, facts_in};
use crate::failpoints::Failpoint;
use crate::memory::{
    insert_state_record, invalidate_records_rooted_in, mark_superseded, state_facts_in, state_kind,
};
use crate::store::SqliteStore;

fn corrupt(detail: impl Into<String>) -> ContinuityError {
    ContinuityError::Corrupt {
        detail: detail.into(),
    }
}

fn parse<T: FromStr>(field: &str, text: &str) -> Result<T, ContinuityError>
where
    T::Err: std::fmt::Display,
{
    text.parse::<T>()
        .map_err(|e| corrupt(format!("column {field} holds {text:?}: {e}")))
}

fn parse_opt<T: FromStr>(field: &str, text: Option<String>) -> Result<Option<T>, ContinuityError>
where
    T::Err: std::fmt::Display,
{
    text.map(|t| parse(field, &t)).transpose()
}

fn ts(millis: i64) -> UtcTimestamp {
    UtcTimestamp::from_unix_millis(millis)
}

fn to_i64(value: u64, field: &str) -> Result<i64, ContinuityError> {
    i64::try_from(value)
        .map_err(|_| corrupt(format!("{field} {value} does not fit SQLite INTEGER")))
}

fn from_i64(value: i64, field: &str) -> Result<u64, ContinuityError> {
    u64::try_from(value)
        .map_err(|_| corrupt(format!("column {field} holds negative value {value}")))
}

/// Snapshot of head + writer epoch as read inside a transaction.
struct HeadRow {
    commit_id: CommitId,
    generation: Generation,
    writer_epoch: WriterEpoch,
    updated_at: UtcTimestamp,
}

fn read_head_row(
    conn: &Connection,
    individual_id: IndividualId,
) -> Result<Option<HeadRow>, ContinuityError> {
    let id = individual_id.to_string();
    let epoch: Option<i64> = conn
        .query_row(
            "SELECT current_writer_epoch FROM individuals WHERE individual_id = ?1",
            params![id],
            |r| r.get(0),
        )
        .optional()
        .map_err(map_sqlite)?;
    let Some(epoch) = epoch else {
        return Ok(None);
    };
    let head: Option<(String, i64, i64)> = conn
        .query_row(
            "SELECT commit_id, generation, updated_at FROM continuity_heads WHERE individual_id = ?1",
            params![id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(map_sqlite)?;
    let Some((commit_id, generation, updated_at)) = head else {
        return Err(corrupt(format!(
            "individual {individual_id} exists but has no continuity head"
        )));
    };
    Ok(Some(HeadRow {
        commit_id: parse("continuity_heads.commit_id", &commit_id)?,
        generation: Generation(from_i64(generation, "continuity_heads.generation")?),
        writer_epoch: WriterEpoch(from_i64(epoch, "individuals.current_writer_epoch")?),
        updated_at: ts(updated_at),
    }))
}

fn receipt_from_row(
    row: &Row<'_>,
) -> rusqlite::Result<(String, String, String, String, String, i64, i64)> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
    ))
}

const RECEIPT_COLUMNS: &str = "receipt_id, individual_id, proposal_id, commit_id, predecessor_commit_id, generation, created_at";

fn build_receipt(
    (receipt_id, individual_id, proposal_id, commit_id, predecessor, generation, created_at): (
        String,
        String,
        String,
        String,
        String,
        i64,
        i64,
    ),
) -> Result<ActivationReceipt, ContinuityError> {
    Ok(ActivationReceipt {
        receipt_id: parse("activation_receipts.receipt_id", &receipt_id)?,
        individual_id: parse("activation_receipts.individual_id", &individual_id)?,
        proposal_id: parse("activation_receipts.proposal_id", &proposal_id)?,
        commit_id: parse("activation_receipts.commit_id", &commit_id)?,
        predecessor_commit_id: parse("activation_receipts.predecessor_commit_id", &predecessor)?,
        generation: Generation(from_i64(generation, "activation_receipts.generation")?),
        created_at: ts(created_at),
    })
}

fn find_receipt_in(
    conn: &Connection,
    proposal_id: ProposalId,
) -> Result<Option<ActivationReceipt>, ContinuityError> {
    conn.query_row(
        &format!("SELECT {RECEIPT_COLUMNS} FROM activation_receipts WHERE proposal_id = ?1"),
        params![proposal_id.to_string()],
        receipt_from_row,
    )
    .optional()
    .map_err(map_sqlite)?
    .map(build_receipt)
    .transpose()
}

fn find_receipt_by_key_in(
    conn: &Connection,
    individual_id: IndividualId,
    idempotency_key: &str,
) -> Result<Option<(ActivationReceipt, String)>, ContinuityError> {
    let row = conn
        .query_row(
            "SELECT r.receipt_id, r.individual_id, r.proposal_id, r.commit_id, r.predecessor_commit_id,
                    r.generation, r.created_at, p.payload_fingerprint
             FROM activation_receipts r
             JOIN mutation_proposals p ON p.proposal_id = r.proposal_id
             WHERE r.individual_id = ?1 AND r.idempotency_key = ?2",
            params![individual_id.to_string(), idempotency_key],
            |row| {
                Ok((
                    receipt_from_row(row)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .optional()
        .map_err(map_sqlite)?;
    row.map(|(r, fp)| Ok((build_receipt(r)?, fp))).transpose()
}

fn find_decision_in(
    conn: &Connection,
    proposal_id: ProposalId,
) -> Result<Option<MutationDecision>, ContinuityError> {
    let row: Option<(String, String, Option<String>, i64, i64)> = conn
        .query_row(
            "SELECT disposition, reason_code, detail, policy_version, decided_at
             FROM mutation_decisions WHERE proposal_id = ?1",
            params![proposal_id.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .optional()
        .map_err(map_sqlite)?;
    row.map(
        |(disposition, reason_code, detail, policy_version, decided_at)| {
            Ok(MutationDecision {
                proposal_id,
                disposition: parse("mutation_decisions.disposition", &disposition)?,
                reason_code: parse("mutation_decisions.reason_code", &reason_code)?,
                detail,
                policy_version: PolicyVersion(u32::try_from(policy_version).map_err(|_| {
                    corrupt(format!("policy_version {policy_version} out of range"))
                })?),
                decided_at: ts(decided_at),
            })
        },
    )
    .transpose()
}

fn insert_proposal(
    tx: &Transaction<'_>,
    proposal: &MutationProposal,
    now: UtcTimestamp,
) -> Result<(), ContinuityError> {
    let evidence_refs =
        serde_json::to_string(&proposal.evidence_refs).map_err(|e| ContinuityError::Backend {
            message: format!("serialize evidence_refs: {e}"),
        })?;
    let requested_by =
        serde_json::to_string(&proposal.requested_by).map_err(|e| ContinuityError::Backend {
            message: format!("serialize requested_by: {e}"),
        })?;
    tx.execute(
        "INSERT INTO mutation_proposals(
            proposal_id, individual_id, domain, operation, subject_key, candidate_json,
            expected_commit_id, expected_generation, evidence_refs_json, origin_class,
            requested_by_json, writer_epoch, policy_version, idempotency_key,
            payload_fingerprint, created_at, recorded_at, supersedes_state_record_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        params![
            proposal.proposal_id.to_string(),
            proposal.individual_id.to_string(),
            proposal.domain.as_str(),
            proposal.operation.as_str(),
            proposal.subject_key,
            proposal.candidate.to_string(),
            proposal.expected_head.commit_id.to_string(),
            to_i64(proposal.expected_head.generation.0, "expected_generation")?,
            evidence_refs,
            proposal.origin_class.as_str(),
            requested_by,
            to_i64(proposal.requested_by.writer_epoch.0, "writer_epoch")?,
            i64::from(proposal.policy_version.0),
            proposal.idempotency_key,
            proposal.payload_fingerprint(),
            proposal.created_at.unix_millis(),
            now.unix_millis(),
            proposal.supersedes.map(|s| s.to_string()),
        ],
    )
    .map_err(map_sqlite)?;
    Ok(())
}

fn insert_decision(
    tx: &Transaction<'_>,
    decision: &MutationDecision,
) -> Result<(), ContinuityError> {
    tx.execute(
        "INSERT INTO mutation_decisions(proposal_id, disposition, reason_code, detail, policy_version, decided_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            decision.proposal_id.to_string(),
            decision.disposition.as_str(),
            decision.reason_code.as_str(),
            decision.detail,
            i64::from(decision.policy_version.0),
            decision.decided_at.unix_millis(),
        ],
    )
    .map_err(map_sqlite)?;
    Ok(())
}

fn insert_audit(
    tx: &Transaction<'_>,
    store: &SqliteStore,
    individual_id: IndividualId,
    kind: AuditKind,
    refs: (Option<CommitId>, Option<ProposalId>),
    payload: serde_json::Value,
    now: UtcTimestamp,
) -> Result<(), ContinuityError> {
    let (commit_id, proposal_id) = refs;
    let audit_event_id = AuditEventId::generate(store.ids());
    tx.execute(
        "INSERT INTO audit_events(audit_event_id, individual_id, kind, commit_id, proposal_id, payload_json, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            audit_event_id.to_string(),
            individual_id.to_string(),
            kind.as_str(),
            commit_id.map(|c| c.to_string()),
            proposal_id.map(|p| p.to_string()),
            payload.to_string(),
            now.unix_millis(),
        ],
    )
    .map_err(map_sqlite)?;
    Ok(())
}

/// Record proposal + non-accepting decision + audit inside `tx`.
fn record_rejection_in(
    tx: &Transaction<'_>,
    store: &SqliteStore,
    proposal: &MutationProposal,
    decision: &MutationDecision,
    now: UtcTimestamp,
) -> Result<(), ContinuityError> {
    insert_proposal(tx, proposal, now)?;
    insert_decision(tx, decision)?;
    insert_audit(
        tx,
        store,
        proposal.individual_id,
        AuditKind::MutationProposed,
        (None, Some(proposal.proposal_id)),
        proposed_payload(proposal),
        now,
    )?;
    insert_audit(
        tx,
        store,
        proposal.individual_id,
        AuditKind::MutationRejected,
        (None, Some(proposal.proposal_id)),
        serde_json::json!({
            "disposition": decision.disposition,
            "reason_code": decision.reason_code,
            "detail": decision.detail,
            "policy_version": decision.policy_version,
        }),
        now,
    )
}

fn proposed_payload(proposal: &MutationProposal) -> serde_json::Value {
    serde_json::json!({
        "domain": proposal.domain,
        "operation": proposal.operation,
        "subject_key": proposal.subject_key,
        "expected_head": proposal.expected_head,
        "evidence_refs": proposal.evidence_refs,
        "supersedes": proposal.supersedes,
        "origin_class": proposal.origin_class,
        "requested_by": proposal.requested_by,
        "policy_version": proposal.policy_version,
        "idempotency_key": proposal.idempotency_key,
    })
}

impl SqliteStore {
    fn begin(conn: &mut Connection) -> Result<Transaction<'_>, ContinuityError> {
        conn.transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(map_sqlite)
    }
}

impl ContinuityStore for SqliteStore {
    fn create_individual(
        &self,
        request: NewIndividual,
    ) -> Result<IndividualBootstrap, ContinuityError> {
        if request.individual_id.is_nil()
            || request.root_commit_id.is_nil()
            || request.node_id.is_nil()
            || request.boot_id.is_nil()
        {
            return Err(ContinuityError::InvalidProposal {
                reason: "new individual has a nil id".to_owned(),
            });
        }
        let now = self.clock().now_utc();
        let mut conn = self.conn()?;
        let tx = Self::begin(&mut conn)?;

        if read_head_row(&tx, request.individual_id)?.is_some() {
            return Err(ContinuityError::IndividualAlreadyExists(
                request.individual_id,
            ));
        }

        let individual = request.individual_id.to_string();
        let root = request.root_commit_id.to_string();
        let epoch = WriterEpoch::INITIAL;
        tx.execute(
            "INSERT INTO individuals(individual_id, root_commit_id, current_writer_epoch, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![individual, root, to_i64(epoch.0, "writer_epoch")?, now.unix_millis()],
        )
        .map_err(map_sqlite)?;
        tx.execute(
            "INSERT INTO writer_epochs(individual_id, writer_epoch, node_id, boot_id, claimed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                individual,
                to_i64(epoch.0, "writer_epoch")?,
                request.node_id.to_string(),
                request.boot_id.to_string(),
                now.unix_millis()
            ],
        )
        .map_err(map_sqlite)?;
        tx.execute(
            "INSERT INTO canonical_commits(commit_id, individual_id, generation, predecessor_commit_id, proposal_id, created_at)
             VALUES (?1, ?2, 0, NULL, NULL, ?3)",
            params![root, individual, now.unix_millis()],
        )
        .map_err(map_sqlite)?;
        tx.execute(
            "INSERT INTO continuity_heads(individual_id, commit_id, generation, updated_at)
             VALUES (?1, ?2, 0, ?3)",
            params![individual, root, now.unix_millis()],
        )
        .map_err(map_sqlite)?;

        let writer = WriterIdentity {
            node_id: request.node_id,
            boot_id: request.boot_id,
            writer_epoch: epoch,
        };
        insert_audit(
            &tx,
            self,
            request.individual_id,
            AuditKind::IndividualCreated,
            (Some(request.root_commit_id), None),
            serde_json::json!({ "root_commit_id": request.root_commit_id, "writer": writer }),
            now,
        )?;
        insert_audit(
            &tx,
            self,
            request.individual_id,
            AuditKind::WriterEpochClaimed,
            (None, None),
            serde_json::json!({ "writer": writer, "previous_epoch": null }),
            now,
        )?;
        tx.commit().map_err(map_sqlite)?;

        Ok(IndividualBootstrap {
            individual: Individual {
                individual_id: request.individual_id,
                root_commit_id: request.root_commit_id,
                created_at: now,
            },
            head: ContinuityHead {
                individual_id: request.individual_id,
                commit_id: request.root_commit_id,
                generation: Generation::ROOT,
                writer_epoch: epoch,
                updated_at: now,
            },
            writer,
        })
    }

    fn load_head(&self, individual_id: IndividualId) -> Result<ContinuityHead, ContinuityError> {
        let conn = self.conn()?;
        let Some(head) = read_head_row(&conn, individual_id)? else {
            return Err(ContinuityError::IndividualNotFound(individual_id));
        };

        // Recovery validation: the head must point at a commit of this
        // individual with the same generation, and no commit may lie beyond it.
        let commit: Option<(String, i64)> = conn
            .query_row(
                "SELECT individual_id, generation FROM canonical_commits WHERE commit_id = ?1",
                params![head.commit_id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(map_sqlite)?;
        match commit {
            None => {
                return Err(corrupt(format!(
                    "head commit {} is missing",
                    head.commit_id
                )));
            }
            Some((owner, generation)) => {
                if owner != individual_id.to_string() {
                    return Err(corrupt(format!(
                        "head commit {} belongs to {owner}",
                        head.commit_id
                    )));
                }
                if from_i64(generation, "canonical_commits.generation")? != head.generation.0 {
                    return Err(corrupt(format!(
                        "head generation {} disagrees with commit generation {generation}",
                        head.generation
                    )));
                }
            }
        }
        let max_generation: i64 = conn
            .query_row(
                "SELECT MAX(generation) FROM canonical_commits WHERE individual_id = ?1",
                params![individual_id.to_string()],
                |r| r.get(0),
            )
            .map_err(map_sqlite)?;
        if from_i64(max_generation, "canonical_commits.generation")? != head.generation.0 {
            return Err(corrupt(format!(
                "commit generation {max_generation} exists beyond head {}",
                head.generation
            )));
        }

        Ok(ContinuityHead {
            individual_id,
            commit_id: head.commit_id,
            generation: head.generation,
            writer_epoch: head.writer_epoch,
            updated_at: head.updated_at,
        })
    }

    fn claim_writer_epoch(
        &self,
        individual_id: IndividualId,
        node_id: NodeId,
        boot_id: BootId,
    ) -> Result<WriterIdentity, ContinuityError> {
        if node_id.is_nil() || boot_id.is_nil() {
            return Err(ContinuityError::InvalidProposal {
                reason: "writer claim has a nil node_id or boot_id".to_owned(),
            });
        }
        let now = self.clock().now_utc();
        let mut conn = self.conn()?;
        let tx = Self::begin(&mut conn)?;
        let Some(head) = read_head_row(&tx, individual_id)? else {
            return Err(ContinuityError::IndividualNotFound(individual_id));
        };
        let next = head.writer_epoch.next();
        let changed = tx
            .execute(
                "UPDATE individuals SET current_writer_epoch = ?2
                 WHERE individual_id = ?1 AND current_writer_epoch = ?3",
                params![
                    individual_id.to_string(),
                    to_i64(next.0, "writer_epoch")?,
                    to_i64(head.writer_epoch.0, "writer_epoch")?
                ],
            )
            .map_err(map_sqlite)?;
        if changed != 1 {
            return Err(corrupt(
                "writer epoch changed inside an immediate transaction",
            ));
        }
        tx.execute(
            "INSERT INTO writer_epochs(individual_id, writer_epoch, node_id, boot_id, claimed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                individual_id.to_string(),
                to_i64(next.0, "writer_epoch")?,
                node_id.to_string(),
                boot_id.to_string(),
                now.unix_millis()
            ],
        )
        .map_err(map_sqlite)?;
        let writer = WriterIdentity {
            node_id,
            boot_id,
            writer_epoch: next,
        };
        insert_audit(
            &tx,
            self,
            individual_id,
            AuditKind::WriterEpochClaimed,
            (None, None),
            serde_json::json!({ "writer": writer, "previous_epoch": head.writer_epoch }),
            now,
        )?;
        tx.commit().map_err(map_sqlite)?;
        Ok(writer)
    }

    fn find_receipt(
        &self,
        proposal_id: ProposalId,
    ) -> Result<Option<ActivationReceipt>, ContinuityError> {
        let conn = self.conn()?;
        find_receipt_in(&conn, proposal_id)
    }

    fn find_decision(
        &self,
        proposal_id: ProposalId,
    ) -> Result<Option<MutationDecision>, ContinuityError> {
        let conn = self.conn()?;
        find_decision_in(&conn, proposal_id)
    }

    fn activate(
        &self,
        proposal: &MutationProposal,
        decision: &MutationDecision,
    ) -> Result<ActivationOutcome, ContinuityError> {
        if decision.disposition != Disposition::Accept {
            return Err(ContinuityError::DecisionNotAccepting(decision.disposition));
        }
        if decision.proposal_id != proposal.proposal_id {
            return Err(ContinuityError::InvalidProposal {
                reason: "decision does not belong to this proposal".to_owned(),
            });
        }
        proposal
            .validate_structure()
            .map_err(|reason| ContinuityError::InvalidProposal { reason })?;

        let now = self.clock().now_utc();
        let mut conn = self.conn()?;
        let tx = Self::begin(&mut conn)?;
        self.failpoint(Failpoint::AfterBegin)?;

        // Idempotency: same proposal, or same logical mutation under another ID.
        if let Some(receipt) = find_receipt_in(&tx, proposal.proposal_id)? {
            return Ok(ActivationOutcome::AlreadyActivated(receipt));
        }
        if let Some(recorded) = find_decision_in(&tx, proposal.proposal_id)? {
            return Ok(ActivationOutcome::Rejected(recorded));
        }
        if let Some((receipt, fingerprint)) =
            find_receipt_by_key_in(&tx, proposal.individual_id, &proposal.idempotency_key)?
        {
            if fingerprint == proposal.payload_fingerprint() {
                return Ok(ActivationOutcome::AlreadyActivated(receipt));
            }
            let rejection = MutationDecision::reject(
                proposal,
                ReasonCode::DuplicateProposalPayloadMismatch,
                format!(
                    "idempotency key {:?} was already activated as {} with different content",
                    proposal.idempotency_key, receipt.proposal_id
                ),
                decision.policy_version,
                now,
            );
            record_rejection_in(&tx, self, proposal, &rejection, now)?;
            tx.commit().map_err(map_sqlite)?;
            return Ok(ActivationOutcome::Rejected(rejection));
        }

        // Re-read head and writer fence under the write lock.
        let Some(head) = read_head_row(&tx, proposal.individual_id)? else {
            return Err(ContinuityError::IndividualNotFound(proposal.individual_id));
        };
        let current = ExpectedHead {
            commit_id: head.commit_id,
            generation: head.generation,
        };
        let fence_rejection = if proposal.requested_by.writer_epoch != head.writer_epoch {
            Some((
                ReasonCode::StaleWriterEpoch,
                format!(
                    "writer {} is fenced; current is {}",
                    proposal.requested_by.writer_epoch, head.writer_epoch
                ),
            ))
        } else if proposal.expected_head != current {
            Some((
                ReasonCode::StalePredecessor,
                format!(
                    "expected {}@{} but head is {}@{}",
                    proposal.expected_head.commit_id,
                    proposal.expected_head.generation,
                    current.commit_id,
                    current.generation
                ),
            ))
        } else {
            None
        };
        if let Some((code, detail)) = fence_rejection {
            let rejection =
                MutationDecision::reject(proposal, code, detail, decision.policy_version, now);
            record_rejection_in(&tx, self, proposal, &rejection, now)?;
            tx.commit().map_err(map_sqlite)?;
            return Ok(ActivationOutcome::Rejected(rejection));
        }

        // Domain separation is re-checked under the write lock: the policy
        // read evidence outside this transaction, and a decision must not be
        // applied against state that moved since.
        let separation = SeparationContext {
            evidence: facts_in(&tx, &proposal.evidence_refs)?,
            supersedes_target: proposal
                .supersedes
                .map(|id| state_facts_in(&tx, id))
                .transpose()?
                .flatten(),
        };
        if let Err(violation) = check_domain_separation(proposal, &separation) {
            let rejection = MutationDecision::reject(
                proposal,
                violation.reason_code,
                violation.detail,
                decision.policy_version,
                now,
            );
            record_rejection_in(&tx, self, proposal, &rejection, now)?;
            tx.commit().map_err(map_sqlite)?;
            return Ok(ActivationOutcome::Rejected(rejection));
        }

        insert_proposal(&tx, proposal, now)?;
        self.failpoint(Failpoint::AfterProposalInsert)?;
        insert_decision(&tx, decision)?;

        let commit_id = CommitId::generate(self.ids());
        let generation = head.generation.next();

        // Derived state. The FK to the commit row is DEFERRED, so the record
        // can name the commit that is written a few statements below.
        let state_record_id = MemoryId::generate(self.ids());
        insert_state_record(
            &tx,
            state_record_id,
            proposal.individual_id,
            proposal.domain,
            proposal.subject_key.as_deref(),
            state_kind(proposal.operation),
            &proposal.candidate,
            commit_id,
            proposal.supersedes,
            &proposal.evidence_refs,
            now,
        )?;
        if let Some(target) = proposal.supersedes {
            mark_superseded(&tx, target, state_record_id)?;
            insert_audit(
                &tx,
                self,
                proposal.individual_id,
                AuditKind::StateSuperseded,
                (Some(commit_id), Some(proposal.proposal_id)),
                serde_json::json!({
                    "reason": "explicit_correction",
                    "superseded_state_record_id": target,
                    "replacement_state_record_id": state_record_id,
                }),
                now,
            )?;
        }
        // Correcting a source takes what rested on it out of the current view
        // instead of leaving it standing as an independent fact (audit A03).
        let corrected = corrected_by(&tx, &proposal.evidence_refs)?;
        if !corrected.is_empty() {
            let invalidated = invalidate_records_rooted_in(
                &tx,
                proposal.individual_id,
                &corrected,
                Some(state_record_id),
            )?;
            if !invalidated.is_empty() {
                insert_audit(
                    &tx,
                    self,
                    proposal.individual_id,
                    AuditKind::StateSuperseded,
                    (Some(commit_id), Some(proposal.proposal_id)),
                    serde_json::json!({
                        "reason": "supporting_evidence_corrected",
                        "corrected_evidence": corrected,
                        "invalidated_state_record_ids": invalidated,
                    }),
                    now,
                )?;
            }
        }
        self.failpoint(Failpoint::AfterStateInsert)?;

        tx.execute(
            "INSERT INTO canonical_commits(commit_id, individual_id, generation, predecessor_commit_id, proposal_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                commit_id.to_string(),
                proposal.individual_id.to_string(),
                to_i64(generation.0, "generation")?,
                head.commit_id.to_string(),
                proposal.proposal_id.to_string(),
                now.unix_millis(),
            ],
        )
        .map_err(map_sqlite)?;
        self.failpoint(Failpoint::AfterCommitRecordInsert)?;

        let moved = tx
            .execute(
                "UPDATE continuity_heads SET commit_id = ?2, generation = ?3, updated_at = ?4
                 WHERE individual_id = ?1 AND commit_id = ?5 AND generation = ?6",
                params![
                    proposal.individual_id.to_string(),
                    commit_id.to_string(),
                    to_i64(generation.0, "generation")?,
                    now.unix_millis(),
                    head.commit_id.to_string(),
                    to_i64(head.generation.0, "generation")?,
                ],
            )
            .map_err(map_sqlite)?;
        if moved != 1 {
            return Err(corrupt(
                "continuity head changed inside an immediate transaction",
            ));
        }
        self.failpoint(Failpoint::AfterHeadUpdate)?;

        let receipt = ActivationReceipt {
            receipt_id: ReceiptId::generate(self.ids()),
            individual_id: proposal.individual_id,
            proposal_id: proposal.proposal_id,
            commit_id,
            predecessor_commit_id: head.commit_id,
            generation,
            created_at: now,
        };
        tx.execute(
            &format!(
                "INSERT INTO activation_receipts({RECEIPT_COLUMNS}, idempotency_key)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
            ),
            params![
                receipt.receipt_id.to_string(),
                receipt.individual_id.to_string(),
                receipt.proposal_id.to_string(),
                receipt.commit_id.to_string(),
                receipt.predecessor_commit_id.to_string(),
                to_i64(receipt.generation.0, "generation")?,
                receipt.created_at.unix_millis(),
                proposal.idempotency_key,
            ],
        )
        .map_err(map_sqlite)?;
        self.failpoint(Failpoint::AfterReceiptInsert)?;

        insert_audit(
            &tx,
            self,
            proposal.individual_id,
            AuditKind::MutationProposed,
            (None, Some(proposal.proposal_id)),
            proposed_payload(proposal),
            now,
        )?;
        insert_audit(
            &tx,
            self,
            proposal.individual_id,
            AuditKind::ContinuityActivated,
            (Some(commit_id), Some(proposal.proposal_id)),
            serde_json::json!({
                "receipt_id": receipt.receipt_id,
                "state_record_id": state_record_id,
                "predecessor_commit_id": receipt.predecessor_commit_id,
                "generation": receipt.generation,
                "policy_version": decision.policy_version,
                "reason_code": decision.reason_code,
            }),
            now,
        )?;

        tx.commit().map_err(map_sqlite)?;
        // The transition is durable from here; losing the ack must not lose it.
        self.failpoint(Failpoint::AfterSqlCommitBeforeAck)?;
        Ok(ActivationOutcome::Activated(receipt))
    }

    fn record_rejection(
        &self,
        proposal: &MutationProposal,
        decision: &MutationDecision,
    ) -> Result<(), ContinuityError> {
        if decision.disposition == Disposition::Accept {
            return Err(ContinuityError::InvalidProposal {
                reason: "record_rejection called with an accepting decision".to_owned(),
            });
        }
        if decision.proposal_id != proposal.proposal_id {
            return Err(ContinuityError::InvalidProposal {
                reason: "decision does not belong to this proposal".to_owned(),
            });
        }
        proposal
            .validate_structure()
            .map_err(|reason| ContinuityError::InvalidProposal { reason })?;
        let now = self.clock().now_utc();
        let mut conn = self.conn()?;
        let tx = Self::begin(&mut conn)?;
        if find_receipt_in(&tx, proposal.proposal_id)?.is_some() {
            return Err(ContinuityError::InvalidProposal {
                reason: "proposal was already activated".to_owned(),
            });
        }
        if let Some(recorded) = find_decision_in(&tx, proposal.proposal_id)? {
            if recorded == *decision {
                return Ok(());
            }
            return Err(ContinuityError::InvalidProposal {
                reason: "proposal already has a different durable decision".to_owned(),
            });
        }
        if read_head_row(&tx, proposal.individual_id)?.is_none() {
            return Err(ContinuityError::IndividualNotFound(proposal.individual_id));
        }
        record_rejection_in(&tx, self, proposal, decision, now)?;
        tx.commit().map_err(map_sqlite)
    }

    fn individuals(&self) -> Result<Vec<Individual>, ContinuityError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT individual_id, root_commit_id, created_at
                 FROM individuals ORDER BY created_at, individual_id",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })
            .map_err(map_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;
        rows.into_iter()
            .map(|(individual_id, root_commit_id, created_at)| {
                Ok(Individual {
                    individual_id: parse("individuals.individual_id", &individual_id)?,
                    root_commit_id: parse("individuals.root_commit_id", &root_commit_id)?,
                    created_at: ts(created_at),
                })
            })
            .collect()
    }

    fn commits(
        &self,
        individual_id: IndividualId,
    ) -> Result<Vec<CanonicalCommit>, ContinuityError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT commit_id, generation, predecessor_commit_id, proposal_id, created_at
                 FROM canonical_commits WHERE individual_id = ?1 ORDER BY generation",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![individual_id.to_string()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            })
            .map_err(map_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;
        rows.into_iter()
            .map(
                |(commit_id, generation, predecessor, proposal, created_at)| {
                    Ok(CanonicalCommit {
                        commit_id: parse("canonical_commits.commit_id", &commit_id)?,
                        individual_id,
                        generation: Generation(from_i64(
                            generation,
                            "canonical_commits.generation",
                        )?),
                        predecessor_commit_id: parse_opt(
                            "canonical_commits.predecessor_commit_id",
                            predecessor,
                        )?,
                        proposal_id: parse_opt("canonical_commits.proposal_id", proposal)?,
                        created_at: ts(created_at),
                    })
                },
            )
            .collect()
    }

    fn audit_events(
        &self,
        individual_id: IndividualId,
    ) -> Result<Vec<AuditEvent>, ContinuityError> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT audit_event_id, kind, commit_id, proposal_id, payload_json, created_at
                 FROM audit_events WHERE individual_id = ?1 ORDER BY audit_seq",
            )
            .map_err(map_sqlite)?;
        let rows = stmt
            .query_map(params![individual_id.to_string()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            })
            .map_err(map_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(map_sqlite)?;
        rows.into_iter()
            .map(|(id, kind, commit, proposal, payload, created_at)| {
                Ok(AuditEvent {
                    audit_event_id: parse("audit_events.audit_event_id", &id)?,
                    individual_id,
                    kind: parse("audit_events.kind", &kind)?,
                    commit_id: parse_opt("audit_events.commit_id", commit)?,
                    proposal_id: parse_opt("audit_events.proposal_id", proposal)?,
                    payload: serde_json::from_str(&payload)
                        .map_err(|e| corrupt(format!("audit payload is not JSON: {e}")))?,
                    created_at: ts(created_at),
                })
            })
            .collect()
    }
}
