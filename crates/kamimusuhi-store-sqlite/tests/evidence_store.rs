//! `EvidenceStore` on SQLite, exercised independently of the Continuity
//! Kernel: sessions, turns, append-oriented records and lineage links.
//!
//! Appending evidence is a different act from changing canonical state
//! (plan §6.2, module doc on `kamimusuhi_store_sqlite::evidence`): these
//! tests pin that boundary down directly, rather than only through
//! activation tests that happen to append evidence along the way.

mod common;

use common::*;
use kamimusuhi_core::continuity::{ContinuityStore, NewIndividual};
use kamimusuhi_core::evidence::{
    EvidenceError, EvidenceKind, EvidenceRelation, EvidenceSource, EvidenceStore, NewEvidence,
    NewEvidenceLink, NewSession, NewTurn, RetentionClass,
};
use kamimusuhi_core::ids::{BootId, CommitId, EvidenceId, IndividualId, SessionId, TurnId};
use kamimusuhi_core::mutation::OriginClass;

const OTHER_INDIVIDUAL: IndividualId = IndividualId::from_u128(0xA2);
const OTHER_ROOT: CommitId = CommitId::from_u128(0xC2);
const OTHER_SESSION: SessionId = SessionId::from_u128(0x52);
const OTHER_TURN: TurnId = TurnId::from_u128(0x72);

#[test]
fn appended_evidence_round_trips_every_field_through_get() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);

    let evidence_id = EvidenceId::from_u128(0xE2);
    let record = NewEvidence {
        evidence_id,
        individual_id: INDIVIDUAL,
        session_id: Some(SESSION),
        turn_id: Some(TURN),
        kind: EvidenceKind::AgentUtterance,
        origin_class: OriginClass::Observed,
        payload: serde_json::json!({ "text": "承知しました" }),
        source: EvidenceSource {
            source_id: Some("fixture-channel".to_owned()),
            source_sequence: Some(3),
            content_digest: Some("digest-abc".to_owned()),
        },
        retention_class: RetentionClass::Sensitive,
    };
    let stored = store.append(record.clone()).unwrap();

    let fetched = store
        .get(evidence_id)
        .unwrap()
        .expect("evidence must exist");
    assert_eq!(fetched, stored);
    assert_eq!(fetched.individual_id, INDIVIDUAL);
    assert_eq!(fetched.session_id, Some(SESSION));
    assert_eq!(fetched.turn_id, Some(TURN));
    assert_eq!(fetched.kind, EvidenceKind::AgentUtterance);
    assert_eq!(fetched.origin_class, OriginClass::Observed);
    assert_eq!(
        fetched.payload,
        serde_json::json!({ "text": "承知しました" })
    );
    assert_eq!(fetched.source.source_id.as_deref(), Some("fixture-channel"));
    assert_eq!(fetched.source.source_sequence, Some(3));
    assert_eq!(fetched.source.content_digest.as_deref(), Some("digest-abc"));
    assert_eq!(fetched.retention_class, RetentionClass::Sensitive);
}

#[test]
fn reappending_an_identical_record_is_a_no_op() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);

    let evidence_id = EvidenceId::from_u128(0xE2);
    let record = user_utterance(evidence_id, "同じ内容");
    let first = store.append(record.clone()).unwrap();
    let second = store.append(record).unwrap();
    assert_eq!(first, second);

    // A no-op re-append does not multiply the row.
    let counts = row_counts(&db.path);
    // fixture utterance (EVIDENCE) + this one.
    assert_eq!(counts.evidence, 2);
}

#[test]
fn reappending_a_different_record_under_the_same_id_is_rejected() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);

    let evidence_id = EvidenceId::from_u128(0xE2);
    store
        .append(user_utterance(evidence_id, "最初の内容"))
        .unwrap();

    let conflicting = user_utterance(evidence_id, "違う内容");
    let err = store.append(conflicting).unwrap_err();
    assert_eq!(err, EvidenceError::AlreadyExists(evidence_id));
}

#[test]
fn appending_evidence_does_not_advance_the_continuity_head() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);
    let before_head = store.load_head(INDIVIDUAL).unwrap();
    let before_commits = store.commits(INDIVIDUAL).unwrap();

    store
        .append(user_utterance(EvidenceId::from_u128(0xE2), "追加の発話"))
        .unwrap();

    let after_head = store.load_head(INDIVIDUAL).unwrap();
    assert_eq!(after_head, before_head);
    assert_eq!(after_head, boot.head);
    assert_eq!(store.commits(INDIVIDUAL).unwrap(), before_commits);
}

#[test]
fn links_round_trip_from_and_to() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);

    let summary_id = EvidenceId::from_u128(0xE2);
    store
        .append(NewEvidence {
            evidence_id: summary_id,
            individual_id: INDIVIDUAL,
            session_id: None,
            turn_id: None,
            kind: EvidenceKind::Summary,
            origin_class: OriginClass::Inferred,
            payload: serde_json::json!({ "text": "お茶が好きだという要約" }),
            source: EvidenceSource::default(),
            retention_class: RetentionClass::Standard,
        })
        .unwrap();

    let link = store
        .link(NewEvidenceLink {
            from_evidence_id: summary_id,
            to_evidence_id: EVIDENCE,
            relation: EvidenceRelation::DerivedFrom,
        })
        .unwrap();
    assert_eq!(link.from_evidence_id, summary_id);
    assert_eq!(link.to_evidence_id, EVIDENCE);
    assert_eq!(link.relation, EvidenceRelation::DerivedFrom);

    let from = store.links_from(summary_id).unwrap();
    assert_eq!(from, vec![link]);
    let to = store.links_to(EVIDENCE).unwrap();
    assert_eq!(to, vec![link]);

    // The reverse direction is empty: links are declared by the newer record.
    assert!(store.links_from(EVIDENCE).unwrap().is_empty());
    assert!(store.links_to(summary_id).unwrap().is_empty());
}

#[test]
fn linking_a_record_to_itself_is_rejected() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);

    let err = store
        .link(NewEvidenceLink {
            from_evidence_id: EVIDENCE,
            to_evidence_id: EVIDENCE,
            relation: EvidenceRelation::DerivedFrom,
        })
        .unwrap_err();
    assert!(matches!(err, EvidenceError::Invalid { .. }));
}

#[test]
fn linking_across_two_individuals_is_rejected() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);
    store
        .create_individual(NewIndividual {
            individual_id: OTHER_INDIVIDUAL,
            root_commit_id: OTHER_ROOT,
            node_id: NODE,
            boot_id: BootId::from_u128(0xB9),
        })
        .unwrap();
    let other_evidence = EvidenceId::from_u128(0xE9);
    store
        .append(NewEvidence {
            evidence_id: other_evidence,
            individual_id: OTHER_INDIVIDUAL,
            session_id: None,
            turn_id: None,
            kind: EvidenceKind::UserUtterance,
            origin_class: OriginClass::Reported,
            payload: serde_json::json!({ "text": "別人の発話" }),
            source: EvidenceSource::default(),
            retention_class: RetentionClass::Standard,
        })
        .unwrap();

    let err = store
        .link(NewEvidenceLink {
            from_evidence_id: other_evidence,
            to_evidence_id: EVIDENCE,
            relation: EvidenceRelation::DerivedFrom,
        })
        .unwrap_err();
    assert!(matches!(err, EvidenceError::Invalid { .. }));
}

#[test]
fn evidence_for_an_unknown_individual_is_rejected() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    // No individual created at all.
    let unknown = IndividualId::from_u128(0xFF);
    let err = store
        .append(NewEvidence {
            evidence_id: EvidenceId::from_u128(0xE2),
            individual_id: unknown,
            session_id: None,
            turn_id: None,
            kind: EvidenceKind::UserUtterance,
            origin_class: OriginClass::Reported,
            payload: serde_json::json!({ "text": "誰の発話でもない" }),
            source: EvidenceSource::default(),
            retention_class: RetentionClass::Standard,
        })
        .unwrap_err();
    assert_eq!(err, EvidenceError::IndividualNotFound(unknown));
}

#[test]
fn a_turn_in_another_individuals_session_is_rejected() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);
    store
        .create_individual(NewIndividual {
            individual_id: OTHER_INDIVIDUAL,
            root_commit_id: OTHER_ROOT,
            node_id: NODE,
            boot_id: BootId::from_u128(0xB9),
        })
        .unwrap();
    store
        .open_session(NewSession {
            session_id: OTHER_SESSION,
            individual_id: OTHER_INDIVIDUAL,
        })
        .unwrap();

    // A turn claiming to belong to INDIVIDUAL but placed in OTHER_INDIVIDUAL's
    // session must be refused, not silently reassigned.
    let err = store
        .record_turn(NewTurn {
            turn_id: OTHER_TURN,
            session_id: OTHER_SESSION,
            individual_id: INDIVIDUAL,
            sequence: 0,
        })
        .unwrap_err();
    assert!(matches!(err, EvidenceError::Invalid { .. }));
}

/// Event identity is the `EvidenceId`, never the content.
///
/// Two people can say 「はい」 twice, and a re-delivered event can carry the
/// same digest as the one it repeats. Collapsing either of those into one
/// record would silently lose an event, so only an explicit identity match
/// makes an append a no-op.
#[test]
fn identical_payloads_in_different_turns_are_different_evidence() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);

    let second_turn = TurnId::from_u128(0x73);
    store
        .record_turn(NewTurn {
            turn_id: second_turn,
            session_id: SESSION,
            individual_id: INDIVIDUAL,
            sequence: 1,
        })
        .unwrap();

    let same_text = "はい";
    let same_digest = Some("digest-identical".to_owned());

    let mut first = user_utterance(EvidenceId::from_u128(0xE4), same_text);
    first.turn_id = Some(TURN);
    first.source.source_sequence = Some(0);
    first.source.content_digest = same_digest.clone();

    let mut second = user_utterance(EvidenceId::from_u128(0xE5), same_text);
    second.turn_id = Some(second_turn);
    second.source.source_sequence = Some(1);
    second.source.content_digest = same_digest;

    let a = store.append(first).unwrap();
    let b = store.append(second).unwrap();

    assert_ne!(a.evidence_id, b.evidence_id);
    assert_eq!(a.payload, b.payload);
    assert_eq!(a.source.content_digest, b.source.content_digest);
    assert_eq!(a.turn_id, Some(TURN));
    assert_eq!(b.turn_id, Some(second_turn));
    // Both survive: an equal digest is an integrity aid, not an identity.
    assert!(store.get(a.evidence_id).unwrap().is_some());
    assert!(store.get(b.evidence_id).unwrap().is_some());
}

/// A retry of the same event is a no-op even when the wall clock moved on:
/// `created_at` is assigned by the store and is not part of the identity.
#[test]
fn retrying_one_event_stores_it_once() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);

    let evidence_id = EvidenceId::from_u128(0xE6);
    let record = user_utterance(evidence_id, "はい");
    let first = store.append(record.clone()).unwrap();
    for _ in 0..3 {
        assert_eq!(store.append(record.clone()).unwrap(), first);
    }

    let conn = rusqlite::Connection::open(&db.path).unwrap();
    let stored: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM evidence_records WHERE evidence_id = ?1",
            rusqlite::params![evidence_id.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stored, 1);
}

/// A retry that claims the same identity but carries different content is a
/// conflict, not an overwrite: the stored record is left exactly as it was.
#[test]
fn same_identity_with_different_content_fails_closed() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);

    let evidence_id = EvidenceId::from_u128(0xE7);
    let original = store.append(user_utterance(evidence_id, "はい")).unwrap();

    for conflicting in [
        user_utterance(evidence_id, "いいえ"),
        NewEvidence {
            kind: EvidenceKind::AgentUtterance,
            ..user_utterance(evidence_id, "はい")
        },
        NewEvidence {
            turn_id: None,
            ..user_utterance(evidence_id, "はい")
        },
    ] {
        assert_eq!(
            store.append(conflicting),
            Err(EvidenceError::AlreadyExists(evidence_id))
        );
    }
    assert_eq!(store.get(evidence_id).unwrap(), Some(original));
}
