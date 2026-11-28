//! FIO write-path failure shapes against the Continuity Kernel store.
//!
//! See `docs/verification/write-fault-shapes-2026-11-27.md`. These tests complement
//! `transaction_failpoints.rs` (which retries the *same* proposal ID) with
//! the two questions that file does not ask:
//!
//! * (a) a failed activation must not leave its idempotency key behind, so a
//!   resend of the same logical mutation under a *new* proposal ID — what an
//!   upstream retry produces — is activated, not swallowed as a duplicate;
//! * (b) after failures at every in-transaction window interleaved with
//!   successes, the projection (`state_records`) and the ledgers
//!   (`canonical_commits`, `activation_receipts`, `mutation_decisions`,
//!   `audit_events`) still agree row for row when read back from disk.

mod common;

use std::path::Path;
use std::sync::Arc;

use common::*;
use kamimusuhi_core::continuity::{ActivationOutcome, ContinuityError, ContinuityStore};
use kamimusuhi_core::ids::ProposalId;
use kamimusuhi_core::mutation::{MutationDecision, MutationProposal};
use kamimusuhi_store_sqlite::failpoints::{Failpoint, FailpointHook, FailpointTriggered};

#[derive(Debug)]
struct FailAt(Failpoint);

impl FailpointHook for FailAt {
    fn hit(&self, point: Failpoint) -> Result<(), FailpointTriggered> {
        if point == self.0 {
            Err(FailpointTriggered(point))
        } else {
            Ok(())
        }
    }
}

fn accept(proposal: &MutationProposal) -> MutationDecision {
    MutationDecision::accept(proposal, proposal.policy_version, proposal.created_at)
}

fn scalar(path: &Path, sql: &str) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}

fn rows_with_key(path: &Path, key: &str) -> i64 {
    rusqlite::Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM activation_receipts WHERE idempotency_key = ?1",
            [key],
            |r| r.get(0),
        )
        .unwrap()
}

/// Shape (a): FIO records "this turn was handled" before the stages run, so
/// a resend after a mid-way failure is dropped as a duplicate. Here the key
/// lives only in `activation_receipts`, inside the activation transaction.
#[test]
fn a_failed_activation_does_not_reserve_its_idempotency_key_for_a_resend() {
    for point in Failpoint::ALL
        .into_iter()
        .filter(|point| !point.is_after_commit())
    {
        let db = TempDb::new();
        let store = open(&db.path, 1);
        let boot = bootstrap(&store, BOOT_A);
        let first = relationship_fact(100, boot.head.expected(), boot.writer, "ほうじ茶");
        let failing = store.with_failpoint_hook(Arc::new(FailAt(point)));
        assert!(
            matches!(
                failing.activate(&first, &accept(&first)),
                Err(ContinuityError::Backend { .. })
            ),
            "{point}"
        );
        drop(failing);

        // The upstream retries the same logical mutation with a fresh
        // proposal ID and the same idempotency key, from a new process.
        let restarted = open(&db.path, 2);
        let mut resend = relationship_fact(200, boot.head.expected(), boot.writer, "ほうじ茶");
        resend.idempotency_key = first.idempotency_key.clone();
        let outcome = restarted.activate(&resend, &accept(&resend)).unwrap();
        let ActivationOutcome::Activated(receipt) = outcome else {
            panic!("{point}: resend after failure was not activated: {outcome:?}");
        };
        assert_eq!(receipt.proposal_id, ProposalId::from_u128(200), "{point}");

        // Final state, read back from disk: the write landed exactly once
        // and nothing remembers the failed attempt as handled.
        assert_eq!(restarted.find_receipt(first.proposal_id).unwrap(), None);
        assert_eq!(restarted.find_decision(first.proposal_id).unwrap(), None);
        assert_eq!(rows_with_key(&db.path, &first.idempotency_key), 1);
        let counts = row_counts(&db.path);
        assert_eq!(counts.state_records, 1, "{point}");
        assert_eq!(counts.receipts, 1, "{point}");
        assert_eq!(counts.head_generation, 1, "{point}");
        let payload: String = rusqlite::Connection::open(&db.path)
            .unwrap()
            .query_row("SELECT payload_json FROM state_records", [], |r| r.get(0))
            .unwrap();
        assert!(payload.contains("ほうじ茶"), "{point}: {payload}");
    }
}

/// Shape (a), content variant: because the key is not reserved by a failed
/// attempt, a corrected resend (same key, different content) is a new
/// activation — it is not mistaken for a payload mismatch against a ghost.
#[test]
fn a_failed_activation_does_not_turn_a_corrected_resend_into_a_payload_mismatch() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);
    let first = relationship_fact(100, boot.head.expected(), boot.writer, "ほうじ茶");
    let failing = store.with_failpoint_hook(Arc::new(FailAt(Failpoint::AfterReceiptInsert)));
    assert!(failing.activate(&first, &accept(&first)).is_err());
    drop(failing);

    let restarted = open(&db.path, 2);
    let mut corrected = relationship_fact(201, boot.head.expected(), boot.writer, "麦茶");
    corrected.idempotency_key = first.idempotency_key.clone();
    assert!(matches!(
        restarted.activate(&corrected, &accept(&corrected)).unwrap(),
        ActivationOutcome::Activated(_)
    ));
    assert_eq!(
        scalar(
            &db.path,
            "SELECT count(*) FROM mutation_decisions WHERE disposition <> 'accept'"
        ),
        0,
        "no rejection was recorded against the failed attempt"
    );
    let payload: String = rusqlite::Connection::open(&db.path)
        .unwrap()
        .query_row("SELECT payload_json FROM state_records", [], |r| r.get(0))
        .unwrap();
    assert!(payload.contains("麦茶"), "{payload}");
}

/// Shape (b): FIO updates the projection first and appends the ledger
/// later, and a failed ledger append is never read back. Here the
/// projection row is written *before* the commit/receipt/audit rows, but in
/// the same transaction; this cross-checks the two sides after a run in
/// which every in-transaction window failed once before being retried.
#[test]
fn projection_and_ledgers_agree_after_failures_at_every_window() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);
    drop(store);

    let mut activated = 0u64;
    for (index, point) in Failpoint::ALL.into_iter().enumerate() {
        let seed = 10 + index as u64 * 2;
        let failing = open(&db.path, seed).with_failpoint_hook(Arc::new(FailAt(point)));
        let head = failing.load_head(INDIVIDUAL).unwrap();
        let id = 1_000 + index as u128;
        let proposal = relationship_fact(id, head.expected(), boot.writer, &format!("茶-{index}"));
        assert!(
            failing.activate(&proposal, &accept(&proposal)).is_err(),
            "{point}"
        );
        drop(failing);

        // Retry the same mutation from a new process.
        let restarted = open(&db.path, seed + 1);
        let outcome = restarted.activate(&proposal, &accept(&proposal)).unwrap();
        if point.is_after_commit() {
            assert!(
                matches!(outcome, ActivationOutcome::AlreadyActivated(_)),
                "{point}: {outcome:?}"
            );
        } else {
            assert!(
                matches!(outcome, ActivationOutcome::Activated(_)),
                "{point}: {outcome:?}"
            );
        }
        activated += 1;
    }

    let path = &db.path;
    let non_root_commits = scalar(
        path,
        "SELECT count(*) FROM canonical_commits WHERE proposal_id IS NOT NULL",
    );
    assert_eq!(non_root_commits, activated as i64);
    assert_eq!(row_counts(path).head_generation, activated as i64);
    // Projection ⊆ ledger and ledger ⊆ projection, one row each way.
    assert_eq!(
        scalar(
            path,
            "SELECT count(*) FROM state_records s
             LEFT JOIN canonical_commits c ON c.commit_id = s.created_commit_id
             WHERE c.commit_id IS NULL"
        ),
        0,
        "projection row without a canonical commit"
    );
    assert_eq!(
        scalar(
            path,
            "SELECT count(*) FROM canonical_commits c
             WHERE c.proposal_id IS NOT NULL AND (
               (SELECT count(*) FROM state_records s WHERE s.created_commit_id = c.commit_id) <> 1
               OR (SELECT count(*) FROM activation_receipts r WHERE r.commit_id = c.commit_id) <> 1
               OR (SELECT count(*) FROM mutation_decisions d
                   WHERE d.proposal_id = c.proposal_id AND d.disposition = 'accept') <> 1
               OR (SELECT count(*) FROM audit_events a
                   WHERE a.commit_id = c.commit_id AND a.kind = 'continuity.activated') <> 1)"
        ),
        0,
        "a commit whose projection, receipt, decision or audit is missing or doubled"
    );
    assert_eq!(
        scalar(path, "SELECT count(*) FROM mutation_proposals"),
        activated as i64,
        "a failed attempt left a proposal row behind"
    );
    assert_eq!(
        scalar(
            path,
            "SELECT count(*) FROM audit_events WHERE kind = 'mutation.proposed'"
        ),
        activated as i64
    );
}
