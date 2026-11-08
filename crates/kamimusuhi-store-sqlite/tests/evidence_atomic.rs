//! Emitted speech and the sources that may later invalidate it must land
//! together. Independent SQL failure injection exercises the real store.

mod common;

use common::*;
use kamimusuhi_core::continuity::{ContinuityStore, NewIndividual};
use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceRelation, EvidenceStore, NewEvidence, NewEvidenceLink,
};
use kamimusuhi_core::ids::{BootId, CommitId, EvidenceId, IndividualId};
use kamimusuhi_core::mutation::OriginClass;
use rusqlite::{Connection, params};

const SECOND: EvidenceId = EvidenceId::from_u128(0xE2);
const EMITTED: EvidenceId = EvidenceId::from_u128(0xE3);

fn utterance() -> NewEvidence {
    let mut record = user_utterance(EMITTED, "I will carry this promise forward");
    record.kind = EvidenceKind::AgentUtterance;
    record.origin_class = OriginClass::Observed;
    record
}

fn links() -> Vec<NewEvidenceLink> {
    [EVIDENCE, SECOND]
        .into_iter()
        .map(|target| NewEvidenceLink {
            from_evidence_id: EMITTED,
            to_evidence_id: target,
            relation: EvidenceRelation::DerivedFrom,
        })
        .collect()
}

fn generated_response() -> NewEvidence {
    let mut record = utterance();
    record.kind = EvidenceKind::SystemEvent;
    // Synthetic decision probabilities exercise the same f32 -> JSON f64
    // boundary as real conversation traces, without storing real dialogue.
    record.payload = serde_json::json!({
        "event": "response_generated",
        "text": "synthetic response",
        "conversation_trace": {
            "invocation_gate": {
                "confidence": 0.95f32,
                "probabilities": {
                    "SPEAK": 0.95f32,
                    "OBSERVE_MORE": 0.03f32,
                    "WAIT": 0.02f32
                }
            },
            "provider_telemetry": {
                "primary": {"success_rate": 1.0f64, "ewma_latency_ms": 5179.25f64}
            }
        }
    });
    record
}

#[test]
fn decision_trace_floats_survive_atomic_readback_retry_and_restart_exactly() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);
    let link = NewEvidenceLink {
        from_evidence_id: EMITTED,
        to_evidence_id: EVIDENCE,
        relation: EvidenceRelation::DerivedFrom,
    };
    let expected_payload = generated_response().payload;
    let before = row_counts(&db.path);
    let stored = store
        .append_with_links(generated_response(), std::slice::from_ref(&link))
        .unwrap();
    assert_eq!(stored.payload, expected_payload);
    assert_eq!(
        store
            .append_with_links(generated_response(), std::slice::from_ref(&link))
            .unwrap(),
        stored
    );
    assert_eq!(row_counts(&db.path).evidence, before.evidence + 1);
    assert_eq!(store.links_from(EMITTED).unwrap().len(), 1);
    drop(store);
    let reopened = open(&db.path, 2);
    assert_eq!(reopened.get(EMITTED).unwrap(), Some(stored));
    assert_eq!(reopened.links_from(EMITTED).unwrap().len(), 1);
}

#[test]
fn altered_decision_trace_float_is_still_rejected_and_rolled_back() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);
    let raw = Connection::open(&db.path).unwrap();
    raw.execute_batch(&format!(
        "CREATE TRIGGER alter_probability AFTER INSERT ON evidence_records
         WHEN NEW.evidence_id = '{}'
         BEGIN UPDATE evidence_records SET payload_json = json_set(payload_json,
           '$.conversation_trace.invocation_gate.probabilities.OBSERVE_MORE', 0.04)
           WHERE evidence_id = NEW.evidence_id; END;",
        EMITTED
    ))
    .unwrap();
    let before = row_counts(&db.path);
    let error = store
        .append_with_links(
            generated_response(),
            &[NewEvidenceLink {
                from_evidence_id: EMITTED,
                to_evidence_id: EVIDENCE,
                relation: EvidenceRelation::DerivedFrom,
            }],
        )
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("appended evidence readback mismatch")
    );
    assert_eq!(row_counts(&db.path), before);
    assert!(store.get(EMITTED).unwrap().is_none());
    assert!(store.links_from(EMITTED).unwrap().is_empty());
}

#[test]
fn atomic_append_survives_restart_and_identical_retry_without_duplicates() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);
    store
        .append(user_utterance(SECOND, "a second source"))
        .unwrap();
    let canonical_before = row_counts(&db.path);
    let expected = store.append_with_links(utterance(), &links()).unwrap();
    assert_eq!(
        store.append_with_links(utterance(), &links()).unwrap(),
        expected
    );
    assert_eq!(store.links_from(EMITTED).unwrap().len(), 2);
    let after = row_counts(&db.path);
    assert_eq!(after.evidence, canonical_before.evidence + 1);
    assert_eq!(after.commits, canonical_before.commits);
    assert_eq!(after.head_generation, canonical_before.head_generation);
    drop(store);
    let reopened = open(&db.path, 2);
    assert_eq!(reopened.get(EMITTED).unwrap(), Some(expected));
    assert_eq!(reopened.links_from(EMITTED).unwrap().len(), 2);
}

#[test]
fn failure_on_second_link_rolls_back_emitted_evidence_and_the_first_link() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);
    store
        .append(user_utterance(SECOND, "a second source"))
        .unwrap();
    let raw = Connection::open(&db.path).unwrap();
    raw.execute_batch(&format!(
        "CREATE TRIGGER stop_second_link BEFORE INSERT ON evidence_links
         WHEN NEW.to_evidence_id = '{}'
         BEGIN SELECT RAISE(FAIL, 'injected lineage write failure'); END;",
        SECOND
    ))
    .unwrap();
    let before = row_counts(&db.path);
    assert!(store.append_with_links(utterance(), &links()).is_err());
    assert_eq!(row_counts(&db.path), before);
    assert!(store.get(EMITTED).unwrap().is_none());
    assert!(store.links_from(EMITTED).unwrap().is_empty());
    let count: i64 = raw
        .query_row(
            "SELECT count(*) FROM evidence_links WHERE from_evidence_id = ?1",
            params![EMITTED.to_string()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    raw.execute_batch("DROP TRIGGER stop_second_link;").unwrap();
    store.append_with_links(utterance(), &links()).unwrap();
    assert_eq!(store.links_from(EMITTED).unwrap().len(), 2);
}

#[test]
fn invalid_origin_missing_target_and_foreign_target_leave_no_emitted_record() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);
    let other = IndividualId::from_u128(0xA2);
    store
        .create_individual(NewIndividual {
            individual_id: other,
            root_commit_id: CommitId::from_u128(0xC2),
            node_id: NODE,
            boot_id: BootId::from_u128(0xB2),
        })
        .unwrap();
    let mut foreign = user_utterance(SECOND, "foreign source");
    foreign.individual_id = other;
    foreign.session_id = None;
    foreign.turn_id = None;
    store.append(foreign).unwrap();
    let before = row_counts(&db.path);
    for link in [
        NewEvidenceLink {
            from_evidence_id: EVIDENCE,
            to_evidence_id: SECOND,
            relation: EvidenceRelation::DerivedFrom,
        },
        NewEvidenceLink {
            from_evidence_id: EMITTED,
            to_evidence_id: EvidenceId::from_u128(0xFFFF),
            relation: EvidenceRelation::DerivedFrom,
        },
        NewEvidenceLink {
            from_evidence_id: EMITTED,
            to_evidence_id: SECOND,
            relation: EvidenceRelation::DerivedFrom,
        },
    ] {
        assert!(store.append_with_links(utterance(), &[link]).is_err());
        assert!(store.get(EMITTED).unwrap().is_none());
        assert!(store.links_from(EMITTED).unwrap().is_empty());
        assert_eq!(row_counts(&db.path), before);
    }
}

#[test]
fn a_later_link_trigger_cannot_silently_remove_an_earlier_link() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);
    store
        .append(user_utterance(SECOND, "a second source"))
        .unwrap();
    let raw = Connection::open(&db.path).unwrap();
    raw.execute_batch(&format!(
        "CREATE TRIGGER lose_first_link AFTER INSERT ON evidence_links
         WHEN NEW.to_evidence_id = '{}'
         BEGIN DELETE FROM evidence_links WHERE from_evidence_id = NEW.from_evidence_id
           AND to_evidence_id = '{}'; END;",
        SECOND, EVIDENCE
    ))
    .unwrap();
    let before = row_counts(&db.path);
    let error = store.append_with_links(utterance(), &links()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("atomic lineage readback mismatch")
    );
    assert_eq!(row_counts(&db.path), before);
    assert!(store.get(EMITTED).unwrap().is_none());
    assert!(store.links_from(EMITTED).unwrap().is_empty());
}
