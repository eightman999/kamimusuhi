//! Schema v7: the derived full-text index over canonical utterances. The
//! index is a rebuildable projection — these tests pin that it never touches
//! `evidence_records`, stays scoped to one individual and one source, leaves
//! ephemeral utterances out, and survives a VACUUM.

mod common;

use common::*;
use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceSource, EvidenceStore, NewEvidence, RetentionClass,
};
use kamimusuhi_core::ids::EvidenceId;
use kamimusuhi_core::mutation::OriginClass;
use kamimusuhi_store_sqlite::SqliteStore;
use kamimusuhi_store_sqlite::migrations::SUPPORTED_SCHEMA_VERSION;
use kamimusuhi_store_sqlite::utterance_index::{COMMON_GRAM_MIN_DOCS, UtteranceIndexSync};

const SOURCE: &str = "fixture-channel";

fn utterance(id: u128, text: &str) -> NewEvidence {
    let mut evidence = user_utterance(EvidenceId::from_u128(id), text);
    evidence.source.source_sequence = Some(id as u64);
    evidence
}

fn append(store: &SqliteStore, evidence: NewEvidence) -> EvidenceId {
    store.append(evidence).unwrap().evidence_id
}

fn hit_ids(store: &SqliteStore, source: &str, query: &str, limit: usize) -> Vec<EvidenceId> {
    store
        .utterance_index_search(INDIVIDUAL, source, query, limit)
        .unwrap()
        .into_iter()
        .map(|hit| hit.record.evidence_id)
        .collect()
}

#[test]
fn schema_v7_creates_the_derived_index_on_an_existing_database() {
    let db = TempDb::new();
    {
        let store = open(&db.path, 1);
        bootstrap(&store, BOOT_A);
    }
    let store = open(&db.path, 2);
    assert_eq!(store.schema_version(), SUPPORTED_SCHEMA_VERSION);
    let tables = store.table_names().unwrap();
    for table in ["utterance_index", "utterance_index_map"] {
        assert!(tables.iter().any(|t| t == table), "{table} must exist");
    }
    // An unsynced index answers nothing rather than failing.
    assert_eq!(store.utterance_index_len().unwrap(), 0);
    assert!(hit_ids(&store, SOURCE, "ほうじ茶", 5).is_empty());
}

#[test]
fn sync_indexes_standard_utterances_once_and_never_touches_evidence() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A); // one standard utterance
    append(&store, utterance(0x10, "通勤手段はロードバイクです"));
    // Ephemeral: deliberately not indexed, so the index cannot extend how
    // long it stays reachable.
    let mut ephemeral = utterance(0x11, "ロードバイクの話は内緒です");
    ephemeral.retention_class = RetentionClass::Ephemeral;
    append(&store, ephemeral);
    // Not an utterance at all.
    append(
        &store,
        NewEvidence {
            kind: EvidenceKind::SystemEvent,
            origin_class: OriginClass::Observed,
            payload: serde_json::json!({"text": "ロードバイク event"}),
            ..utterance(0x12, "")
        },
    );
    // An utterance with nothing to index still gets examined exactly once.
    append(&store, utterance(0x13, "。"));
    let before = row_counts(&db.path);

    // The row bound stops early and says so.
    assert_eq!(
        store.utterance_index_sync(1).unwrap(),
        UtteranceIndexSync {
            indexed: 1,
            pending: true
        }
    );
    assert_eq!(
        store.utterance_index_sync(100).unwrap(),
        UtteranceIndexSync {
            indexed: 2,
            pending: false
        }
    );
    // Idempotent: nothing new, nothing done.
    assert_eq!(
        store.utterance_index_sync(100).unwrap(),
        UtteranceIndexSync::default()
    );
    assert_eq!(store.utterance_index_len().unwrap(), 3);
    // Evidence and state rows are exactly what they were before indexing.
    let after = row_counts(&db.path);
    assert_eq!(after.evidence, before.evidence);
    assert_eq!(after.state_records, before.state_records);

    assert_eq!(
        hit_ids(&store, SOURCE, "ロードバイクで通勤", 5),
        vec![EvidenceId::from_u128(0x10)],
        "only the standard utterance is reachable through the index"
    );
    // A later append is picked up by the next sync only.
    append(&store, utterance(0x14, "ロードバイクを整備した"));
    assert_eq!(hit_ids(&store, SOURCE, "ロードバイク", 5).len(), 1);
    assert_eq!(store.utterance_index_sync(100).unwrap().indexed, 1);
    assert_eq!(hit_ids(&store, SOURCE, "ロードバイク", 5).len(), 2);
}

#[test]
fn search_is_scoped_to_one_source_and_ranks_the_closer_match_first() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);
    append(
        &store,
        utterance(0x20, "週末は川沿いをロードバイクで走った"),
    );
    append(
        &store,
        utterance(0x21, "ロードバイクのタイヤを交換した。ロードバイクは楽しい"),
    );
    append(&store, utterance(0x22, "今日はカレーを作った"));
    let mut elsewhere = utterance(0x23, "ロードバイクの話を別の場所でした");
    elsewhere.source = EvidenceSource {
        source_id: Some("other-channel".to_owned()),
        source_sequence: Some(1),
        content_digest: None,
    };
    append(&store, elsewhere);
    store.utterance_index_sync(100).unwrap();

    let hits = store
        .utterance_index_search(INDIVIDUAL, SOURCE, "ロードバイクのタイヤ", 5)
        .unwrap();
    let ids: Vec<_> = hits.iter().map(|hit| hit.record.evidence_id).collect();
    assert_eq!(
        ids,
        vec![EvidenceId::from_u128(0x21), EvidenceId::from_u128(0x20)]
    );
    // bm25() is negative and ascending: the better match is more negative.
    assert!(hits[0].bm25 < hits[1].bm25 && hits[1].bm25 < 0.0);
    // The same words in another source stay in that source.
    assert_eq!(
        hit_ids(&store, "other-channel", "ロードバイク", 5),
        vec![EvidenceId::from_u128(0x23)]
    );
    // The limit bounds the result; no usable bigram means no hits, not a scan.
    assert_eq!(hit_ids(&store, SOURCE, "ロードバイク", 1).len(), 1);
    assert!(hit_ids(&store, SOURCE, "ロードバイク", 0).is_empty());
    assert!(hit_ids(&store, SOURCE, "。！", 5).is_empty());
    assert!(hit_ids(&store, SOURCE, "zzzz qqqq", 5).is_empty());
}

#[test]
fn common_bigrams_are_dropped_once_the_index_can_tell() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);
    let filler = COMMON_GRAM_MIN_DOCS as u128 + 10;
    for index in 0..filler {
        append(
            &store,
            utterance(
                0x100 + index,
                &format!("こんにちは、今日の番号は{index}です"),
            ),
        );
    }
    let rare = append(&store, utterance(0x900, "こんにちは、輪行袋を買いました"));
    store.utterance_index_sync(1000).unwrap();

    // Every bigram of the greeting is in far more than 30% of the index.
    assert!(hit_ids(&store, SOURCE, "こんにちは", 5).is_empty());
    // With the common grams gone, only the rare words decide the match.
    assert_eq!(hit_ids(&store, SOURCE, "こんにちは、輪行袋", 5), vec![rare]);
}

#[test]
fn the_index_keeps_pointing_at_the_right_evidence_after_vacuum() {
    let db = TempDb::new();
    let target = {
        let store = open(&db.path, 1);
        bootstrap(&store, BOOT_A);
        append(&store, utterance(0x30, "朝は紅茶を飲む"));
        let target = append(&store, utterance(0x31, "通勤手段はロードバイクです"));
        append(&store, utterance(0x32, "夜は読書をする"));
        store.utterance_index_sync(100).unwrap();
        target
    };
    // `evidence_records` has only an implicit rowid, which VACUUM may
    // renumber. The index is keyed on evidence ids, so it must not care.
    rusqlite::Connection::open(&db.path)
        .unwrap()
        .execute_batch("VACUUM")
        .unwrap();
    let store = open(&db.path, 2);
    assert_eq!(hit_ids(&store, SOURCE, "ロードバイク通勤", 5), vec![target]);
    assert_eq!(
        store.utterance_index_sync(100).unwrap(),
        UtteranceIndexSync::default()
    );
}
