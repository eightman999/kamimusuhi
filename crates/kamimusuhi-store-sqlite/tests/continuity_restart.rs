//! Restart recovery and fail-closed corruption detection.

mod common;

use common::*;
use kamimusuhi_core::audit::AuditKind;
use kamimusuhi_core::continuity::{
    ActivationOutcome, ContinuityError, ContinuityStore, Generation,
};
use kamimusuhi_core::ids::{CommitId, ProposalId};
use rusqlite::{Connection, params};

#[test]
fn process_restart_restores_lineage_then_advances_with_a_new_writer_epoch() {
    let db = TempDb::new();
    let (a_head, a_commits) = {
        let store_a = open(&db.path, 1);
        let boot = bootstrap(&store_a, BOOT_A);
        let kernel_a = kernel(store_a);
        let proposal = relationship_fact(100, boot.head.expected(), boot.writer, "ほうじ茶");
        assert!(matches!(
            kernel_a.submit(&proposal).unwrap(),
            ActivationOutcome::Activated(_)
        ));
        (
            kernel_a.store().load_head(INDIVIDUAL).unwrap(),
            kernel_a.store().commits(INDIVIDUAL).unwrap(),
        )
    };

    let store_b = open(&db.path, 2);
    assert_eq!(store_b.load_head(INDIVIDUAL).unwrap(), a_head);
    assert_eq!(store_b.commits(INDIVIDUAL).unwrap(), a_commits);
    assert_eq!(a_head.individual_id, INDIVIDUAL);
    assert_eq!(a_head.generation, Generation(1));
    assert_eq!(a_head.writer_epoch.0, 1);

    let writer_b = store_b
        .claim_writer_epoch(INDIVIDUAL, NODE, BOOT_B)
        .unwrap();
    assert_eq!(writer_b.writer_epoch.0, 2);
    let head_b = store_b.load_head(INDIVIDUAL).unwrap();
    let kernel_b = kernel(store_b);
    let proposal_b = relationship_fact(101, head_b.expected(), writer_b, "玄米茶");
    let ActivationOutcome::Activated(receipt_b) = kernel_b.submit(&proposal_b).unwrap() else {
        panic!("process B proposal was not activated");
    };
    assert_eq!(receipt_b.generation, Generation(2));
    assert_eq!(receipt_b.predecessor_commit_id, a_head.commit_id);
    let kinds: Vec<AuditKind> = kernel_b
        .store()
        .audit_events(INDIVIDUAL)
        .unwrap()
        .into_iter()
        .map(|event| event.kind)
        .collect();
    assert_eq!(
        kinds,
        vec![
            AuditKind::IndividualCreated,
            AuditKind::WriterEpochClaimed,
            AuditKind::MutationProposed,
            AuditKind::ContinuityActivated,
            AuditKind::WriterEpochClaimed,
            AuditKind::MutationProposed,
            AuditKind::ContinuityActivated,
        ]
    );
}

#[test]
fn missing_continuity_head_is_corruption_not_implicit_recovery() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);
    drop(store);
    let raw = Connection::open(&db.path).unwrap();
    raw.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    raw.execute(
        "DELETE FROM continuity_heads WHERE individual_id = ?1",
        params![INDIVIDUAL.to_string()],
    )
    .unwrap();
    drop(raw);

    let reopened = open(&db.path, 2);
    assert!(matches!(
        reopened.load_head(INDIVIDUAL),
        Err(ContinuityError::Corrupt { .. })
    ));
}

#[test]
fn orphan_commit_beyond_head_is_corruption_not_auto_repaired() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);
    drop(store);
    let raw = Connection::open(&db.path).unwrap();
    raw.execute_batch("PRAGMA foreign_keys = OFF").unwrap();
    raw.execute(
        "INSERT INTO canonical_commits(
            commit_id, individual_id, generation, predecessor_commit_id, proposal_id, created_at
         ) VALUES (?1, ?2, 5, ?3, ?4, 0)",
        params![
            CommitId::from_u128(0xC5).to_string(),
            INDIVIDUAL.to_string(),
            ROOT_COMMIT.to_string(),
            ProposalId::from_u128(0xF5).to_string(),
        ],
    )
    .unwrap();
    drop(raw);

    let reopened = open(&db.path, 2);
    assert!(matches!(
        reopened.load_head(INDIVIDUAL),
        Err(ContinuityError::Corrupt { .. })
    ));
}
