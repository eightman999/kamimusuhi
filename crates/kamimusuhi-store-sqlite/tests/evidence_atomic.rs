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
