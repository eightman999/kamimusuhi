//! C0 projection integrity: independent SQL corruption/fault injection must
//! never become an acknowledged empty self model. These are isolated stores.

mod common;

use common::*;
use kamimusuhi_core::c0::{
    C0Error, ImprovementKind, ImprovementProposal, ProposalStatus, SelfField,
};
use kamimusuhi_core::evidence::{EvidenceKind, EvidenceRelation, EvidenceStore, NewEvidenceLink};
use kamimusuhi_core::ids::{C0ActivationId, C0ProposalId, EvidenceId};
use kamimusuhi_core::mutation::OriginClass;
use kamimusuhi_core::time::UtcTimestamp;
use kamimusuhi_store_sqlite::SqliteStore;
use rusqlite::{Connection, params};
use serde_json::json;

const SELF_EVIDENCE: EvidenceId = EvidenceId::from_u128(0xE2);

fn setup(store: &SqliteStore) {
    bootstrap(store, BOOT_A);
    let mut evidence = user_utterance(SELF_EVIDENCE, "I checked my work and found a lesson");
    evidence.kind = EvidenceKind::AgentUtterance;
    evidence.origin_class = OriginClass::Observed;
    store.append(evidence).unwrap();
}

fn now() -> UtcTimestamp {
    UtcTimestamp::from_unix_millis(1_730_000_000_000)
}

fn proposal(id: u128, key: &str, value: &str) -> ImprovementProposal {
    ImprovementProposal {
        proposal_id: C0ProposalId::from_u128(id),
        individual_id: INDIVIDUAL,
        kind: ImprovementKind::SelfUpdate,
        target: "capabilities".to_owned(),
        target_key: Some(key.to_owned()),
        old_value: None,
        proposed_value: json!(value),
        evidence_refs: vec![SELF_EVIDENCE],
        expected_effect: None,
        risk: None,
        confidence: Some(0.8),
        status: ProposalStatus::Pending,
        rejection_reason: None,
        reflection_id: None,
        created_at: now(),
        decided_at: None,
        activation_seq: None,
    }
}

fn activate(store: &SqliteStore, id: u128, key: &str, value: &str) {
    let p = proposal(id, key, value);
    store.c0_insert_proposal(&p).unwrap();
    store
        .c0_apply_activation(p.proposal_id, C0ActivationId::from_u128(id + 1000), now())
        .unwrap();
}

fn derived_counts(raw: &Connection) -> (i64, i64) {
    (
        raw.query_row("SELECT count(*) FROM c0_activations", [], |r| r.get(0))
            .unwrap(),
        raw.query_row(
            "SELECT count(*) FROM c0_proposals WHERE status = 'accepted'",
            [],
            |r| r.get(0),
        )
        .unwrap(),
    )
}

#[test]
fn syntactically_valid_snapshot_that_loses_a_lesson_is_refused_after_reopen() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    setup(&store);
    activate(
        &store,
        10,
        "lesson",
        "check the quoted source before judging",
    );
    let canonical_before = row_counts(&db.path);
    drop(store);
    let raw = Connection::open(&db.path).unwrap();
    raw.execute(
        "UPDATE c0_activations SET self_json = '{\"fields\":{}}'",
        [],
    )
    .unwrap();
    let store = open(&db.path, 2);
    let error = store.c0_view(INDIVIDUAL).unwrap_err();
    assert!(matches!(error, C0Error::CorruptRow(_)), "{error}");
    assert!(error.to_string().contains("projection mismatch"));
    assert_eq!(row_counts(&db.path), canonical_before);
    // No repair or silent recreation has occurred.
    assert_eq!(derived_counts(&raw), (1, 1));
    assert_eq!(store.c0_head(INDIVIDUAL).unwrap(), 1);
}

#[test]
fn malformed_head_cannot_become_an_empty_model_during_activation() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    setup(&store);
    activate(&store, 10, "lesson", "finish authorized work");
    let p = proposal(11, "another", "preserve other lessons");
    store.c0_insert_proposal(&p).unwrap();
    let raw = Connection::open(&db.path).unwrap();
    raw.execute("UPDATE c0_activations SET self_json = 'broken-json'", [])
        .unwrap();
    let before = derived_counts(&raw);
    assert!(
        store
            .c0_apply_activation(p.proposal_id, C0ActivationId::from_u128(1011), now())
            .is_err()
    );
    assert_eq!(derived_counts(&raw), before);
    assert_eq!(store.c0_head(INDIVIDUAL).unwrap(), 1);
    assert_eq!(
        store.c0_proposal(p.proposal_id).unwrap().unwrap().status,
        ProposalStatus::Pending
    );
}

#[test]
fn corrupted_proposal_payloads_and_evidence_are_not_repaired_to_null_or_empty() {
    for column in [
        "proposed_value_json",
        "evidence_refs_json",
        "old_value_json",
    ] {
        let db = TempDb::new();
        let store = open(&db.path, 1);
        setup(&store);
        let p = proposal(10, "lesson", "preserve provenance");
        store.c0_insert_proposal(&p).unwrap();
        let raw = Connection::open(&db.path).unwrap();
        raw.execute(
            &format!("UPDATE c0_proposals SET {column} = 'broken-json'"),
            [],
        )
        .unwrap();
        assert!(store.c0_proposal(p.proposal_id).is_err(), "{column}");
        assert!(
            store
                .c0_apply_activation(p.proposal_id, C0ActivationId::from_u128(1010), now())
                .is_err(),
            "{column}"
        );
        assert_eq!(derived_counts(&raw), (0, 0));
        assert_eq!(store.c0_head(INDIVIDUAL).unwrap(), 0);
    }
}

#[test]
fn write_landing_fault_rolls_back_snapshot_status_and_head_together() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    setup(&store);
    activate(&store, 10, "lesson", "finish authorized work");
    let p = proposal(11, "another", "check persistence separately from response");
    store.c0_insert_proposal(&p).unwrap();
    let canonical_before = row_counts(&db.path);
    let raw = Connection::open(&db.path).unwrap();
    raw.execute_batch(
        "CREATE TRIGGER simulate_lost_projection AFTER INSERT ON c0_activations
         BEGIN UPDATE c0_activations SET self_json = '{\"fields\":{}}'
         WHERE individual_id = NEW.individual_id AND activation_seq = NEW.activation_seq; END;",
    )
    .unwrap();
    let error = store
        .c0_apply_activation(p.proposal_id, C0ActivationId::from_u128(1011), now())
        .unwrap_err();
    assert!(error.to_string().contains("projection mismatch"), "{error}");
    assert_eq!(derived_counts(&raw), (1, 1));
    assert_eq!(store.c0_head(INDIVIDUAL).unwrap(), 1);
    assert_eq!(
        store.c0_proposal(p.proposal_id).unwrap().unwrap().status,
        ProposalStatus::Pending
    );
    assert_eq!(row_counts(&db.path), canonical_before);
    assert_eq!(
        store.c0_view(INDIVIDUAL).unwrap().self_model.entry_count(),
        1
    );
}

#[test]
fn a_head_write_that_does_not_land_is_not_acknowledged() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    setup(&store);
    activate(&store, 10, "lesson", "finish authorized work");
    let p = proposal(11, "another", "verify the head actually moved");
    store.c0_insert_proposal(&p).unwrap();
    let raw = Connection::open(&db.path).unwrap();
    raw.execute_batch(
        "CREATE TRIGGER simulate_unlanded_head AFTER UPDATE ON c0_head
         BEGIN UPDATE c0_head SET activation_seq = OLD.activation_seq
         WHERE individual_id = NEW.individual_id; END;",
    )
    .unwrap();
    let before = store.c0_view(INDIVIDUAL).unwrap();
    let error = store
        .c0_apply_activation(p.proposal_id, C0ActivationId::from_u128(1011), now())
        .unwrap_err();
    assert!(
        error.to_string().contains("activation readback mismatch"),
        "{error}"
    );
    assert_eq!(derived_counts(&raw), (1, 1));
    assert_eq!(store.c0_view(INDIVIDUAL).unwrap(), before);
    assert_eq!(
        store.c0_proposal(p.proposal_id).unwrap().unwrap().status,
        ProposalStatus::Pending
    );
}

#[test]
fn concurrently_staged_self_claims_are_revalidated_against_the_latest_view() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    setup(&store);
    let first = proposal(10, "first", "preserve original evidence");
    let duplicate = proposal(11, "another-key", "preserve original evidence");
    store.c0_insert_proposal(&first).unwrap();
    store.c0_insert_proposal(&duplicate).unwrap();
    store
        .c0_apply_activation(first.proposal_id, C0ActivationId::from_u128(1010), now())
        .unwrap();
    let before = store.c0_view(INDIVIDUAL).unwrap();
    let error = store
        .c0_apply_activation(
            duplicate.proposal_id,
            C0ActivationId::from_u128(1011),
            now(),
        )
        .unwrap_err();
    assert!(
        error.to_string().contains("duplicates another key"),
        "{error}"
    );
    assert_eq!(store.c0_view(INDIVIDUAL).unwrap(), before);
    assert_eq!(
        store
            .c0_proposal(duplicate.proposal_id)
            .unwrap()
            .unwrap()
            .status,
        ProposalStatus::Pending
    );
}

#[test]
fn a_correction_between_staging_and_activation_revokes_derived_support() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    setup(&store);
    store
        .link(NewEvidenceLink {
            from_evidence_id: SELF_EVIDENCE,
            to_evidence_id: EVIDENCE,
            relation: EvidenceRelation::DerivedFrom,
        })
        .unwrap();
    let p = proposal(10, "lesson", "a claim derived from the conversation");
    store.c0_insert_proposal(&p).unwrap();
    let correction = EvidenceId::from_u128(0xE3);
    store
        .append(user_utterance(
            correction,
            "That earlier account was incorrect",
        ))
        .unwrap();
    store
        .link(NewEvidenceLink {
            from_evidence_id: correction,
            to_evidence_id: EVIDENCE,
            relation: EvidenceRelation::Corrects,
        })
        .unwrap();
    let canonical_before = row_counts(&db.path);
    let error = store
        .c0_apply_activation(p.proposal_id, C0ActivationId::from_u128(1010), now())
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("current, individual-owned rooted evidence"),
        "{error}"
    );
    assert_eq!(
        store.c0_view(INDIVIDUAL).unwrap().self_model.entry_count(),
        0
    );
    assert_eq!(store.c0_head(INDIVIDUAL).unwrap(), 0);
    assert_eq!(
        store.c0_proposal(p.proposal_id).unwrap().unwrap().status,
        ProposalStatus::Pending
    );
    assert_eq!(row_counts(&db.path), canonical_before);
}

#[test]
fn restart_and_branching_preserve_untouched_lessons_and_history() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    setup(&store);
    activate(&store, 10, "first", "retain original evidence");
    activate(&store, 11, "second", "report only verified work");
    let complete = store.c0_view(INDIVIDUAL).unwrap();
    drop(store);
    let store = open(&db.path, 2);
    assert_eq!(store.c0_view(INDIVIDUAL).unwrap(), complete);
    store.c0_rollback(INDIVIDUAL, now()).unwrap();
    // Branch off activation 1: activation 2 remains historical, never replayed
    // onto this branch merely because its sequence is lower than the new head.
    activate(&store, 12, "third", "continue from the restored view");
    let view = store.c0_view(INDIVIDUAL).unwrap();
    let entries = view.self_model.field(SelfField::Capabilities);
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().any(|e| e.key.as_deref() == Some("first")));
    assert!(entries.iter().any(|e| e.key.as_deref() == Some("third")));
    assert!(!entries.iter().any(|e| e.key.as_deref() == Some("second")));
    assert_eq!(store.c0_activations(INDIVIDUAL).unwrap().len(), 3);
    // Damage only the dormant branch; current reads stay valid.
    Connection::open(&db.path)
        .unwrap()
        .execute(
            "UPDATE c0_activations SET self_json = 'broken-json' WHERE activation_seq = ?1",
            params![2],
        )
        .unwrap();
    assert_eq!(store.c0_view(INDIVIDUAL).unwrap(), view);
}
