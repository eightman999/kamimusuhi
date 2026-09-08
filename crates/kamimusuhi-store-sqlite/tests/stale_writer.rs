//! Store/kernel regressions for stale predecessors, writer fencing and policy versions.

mod common;

use common::*;
use kamimusuhi_core::audit::AuditKind;
use kamimusuhi_core::continuity::{ActivationOutcome, ContinuityStore, Generation, WriterEpoch};
use kamimusuhi_core::ids::PolicyVersion;
use kamimusuhi_core::mutation::{MutationDecision, ReasonCode};
use rusqlite::Connection;

fn assert_stale_predecessor(
    outcome: ActivationOutcome,
) -> kamimusuhi_core::mutation::MutationDecision {
    let ActivationOutcome::Rejected(decision) = outcome else {
        panic!("expected STALE_PREDECESSOR rejection, got {outcome:?}");
    };
    assert_eq!(decision.reason_code, ReasonCode::StalePredecessor);
    decision
}

#[test]
fn t02_p1_then_p2_rejects_p2_without_forking() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);
    let kernel = kernel(store);
    let p1 = relationship_fact(100, boot.head.expected(), boot.writer, "ほうじ茶");
    let p2 = relationship_fact(101, boot.head.expected(), boot.writer, "玄米茶");

    assert!(matches!(
        kernel.submit(&p1).unwrap(),
        ActivationOutcome::Activated(_)
    ));
    let decision = assert_stale_predecessor(kernel.submit(&p2).unwrap());
    assert_eq!(
        kernel.store().find_decision(p2.proposal_id).unwrap(),
        Some(decision)
    );
    assert_eq!(
        kernel.store().load_head(INDIVIDUAL).unwrap().generation,
        Generation(1)
    );
    assert_eq!(kernel.store().commits(INDIVIDUAL).unwrap().len(), 2);
}

#[test]
fn t02_p2_then_p1_rejects_p1_without_forking() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);
    let kernel = kernel(store);
    let p1 = relationship_fact(100, boot.head.expected(), boot.writer, "ほうじ茶");
    let p2 = relationship_fact(101, boot.head.expected(), boot.writer, "玄米茶");

    assert!(matches!(
        kernel.submit(&p2).unwrap(),
        ActivationOutcome::Activated(_)
    ));
    let decision = assert_stale_predecessor(kernel.submit(&p1).unwrap());
    assert_eq!(
        kernel.store().find_decision(p1.proposal_id).unwrap(),
        Some(decision)
    );
    assert_eq!(
        kernel.store().load_head(INDIVIDUAL).unwrap().generation,
        Generation(1)
    );
    assert_eq!(kernel.store().commits(INDIVIDUAL).unwrap().len(), 2);
}

#[test]
fn t15_stale_generation_zero_is_rejected_by_kernel_and_store() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);
    let kernel = kernel(store);
    let accepted = relationship_fact(100, boot.head.expected(), boot.writer, "ほうじ茶");
    assert!(matches!(
        kernel.submit(&accepted).unwrap(),
        ActivationOutcome::Activated(_)
    ));

    let via_kernel = relationship_fact(101, boot.head.expected(), boot.writer, "玄米茶");
    let kernel_decision = assert_stale_predecessor(kernel.submit(&via_kernel).unwrap());
    assert_eq!(
        kernel
            .store()
            .find_decision(via_kernel.proposal_id)
            .unwrap(),
        Some(kernel_decision)
    );

    let via_store = relationship_fact(102, boot.head.expected(), boot.writer, "麦茶");
    let store_decision = assert_stale_predecessor(
        kernel
            .store()
            .activate(
                &via_store,
                &MutationDecision::accept(
                    &via_store,
                    via_store.policy_version,
                    via_store.created_at,
                ),
            )
            .unwrap(),
    );
    assert_eq!(
        kernel.store().find_decision(via_store.proposal_id).unwrap(),
        Some(store_decision)
    );
    assert_eq!(
        kernel.store().load_head(INDIVIDUAL).unwrap().generation,
        Generation(1)
    );
}

#[test]
fn t03_writer_takeover_fences_epoch_one_and_preserves_the_individual() {
    let db = TempDb::new();
    let store_a = open(&db.path, 1);
    let boot = bootstrap(&store_a, BOOT_A);
    let store_b = open(&db.path, 2);
    let writer_b = store_b
        .claim_writer_epoch(INDIVIDUAL, NODE, BOOT_B)
        .unwrap();
    assert_eq!(writer_b.writer_epoch, WriterEpoch(2));
    let events = store_b.audit_events(INDIVIDUAL).unwrap();
    let claim = events.last().unwrap();
    assert_eq!(claim.kind, AuditKind::WriterEpochClaimed);
    assert_eq!(claim.payload["previous_epoch"], serde_json::json!(1));

    let kernel_a = kernel(store_a);
    let current = kernel_a.store().load_head(INDIVIDUAL).unwrap();
    let via_kernel = relationship_fact(100, current.expected(), boot.writer, "ほうじ茶");
    let kernel_decision = {
        let ActivationOutcome::Rejected(decision) = kernel_a.submit(&via_kernel).unwrap() else {
            panic!("old writer kernel submission was not rejected");
        };
        decision
    };
    assert_eq!(kernel_decision.reason_code, ReasonCode::StaleWriterEpoch);
    assert_eq!(
        kernel_a
            .store()
            .find_decision(via_kernel.proposal_id)
            .unwrap(),
        Some(kernel_decision)
    );

    let via_store = relationship_fact(101, current.expected(), boot.writer, "玄米茶");
    let store_decision = {
        let ActivationOutcome::Rejected(decision) = kernel_a
            .store()
            .activate(
                &via_store,
                &MutationDecision::accept(
                    &via_store,
                    via_store.policy_version,
                    via_store.created_at,
                ),
            )
            .unwrap()
        else {
            panic!("old writer store activation was not rejected");
        };
        decision
    };
    assert_eq!(store_decision.reason_code, ReasonCode::StaleWriterEpoch);
    assert_eq!(kernel_a.store().load_head(INDIVIDUAL).unwrap(), current);

    let raw = Connection::open(&db.path).unwrap();
    let individuals: i64 = raw
        .query_row("SELECT COUNT(*) FROM individuals", [], |row| row.get(0))
        .unwrap();
    assert_eq!(individuals, 1);

    let kernel_b = kernel(store_b);
    let proposal_b = relationship_fact(102, current.expected(), writer_b, "ほうじ茶");
    assert!(matches!(
        kernel_b.submit(&proposal_b).unwrap(),
        ActivationOutcome::Activated(_)
    ));
    assert_eq!(
        kernel_b.store().load_head(INDIVIDUAL).unwrap().generation,
        Generation(1)
    );
}

#[test]
fn t25_policy_version_mismatch_is_durable_without_head_movement() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);
    let kernel = kernel(store);
    let mut proposal = relationship_fact(100, boot.head.expected(), boot.writer, "ほうじ茶");
    proposal.policy_version = PolicyVersion(1);

    let ActivationOutcome::Rejected(decision) = kernel.submit(&proposal).unwrap() else {
        panic!("policy version mismatch was not rejected");
    };
    assert_eq!(decision.reason_code, ReasonCode::PolicyVersionMismatch);
    assert_eq!(
        kernel.store().find_decision(proposal.proposal_id).unwrap(),
        Some(decision)
    );
    assert_eq!(kernel.store().load_head(INDIVIDUAL).unwrap(), boot.head);
}
