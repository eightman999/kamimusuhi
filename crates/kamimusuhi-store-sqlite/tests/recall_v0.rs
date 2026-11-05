//! Recall v0 foundation: declared payload metadata rides inside the record's
//! `payload_json` unchanged, and `last_used_at`/`use_count` bookkeeping lives
//! in the `state_record_usage` side table — never on the canonical
//! `state_records` row (`kamimusuhi_core::memory::MemoryUsage`).

mod common;

use common::*;
use kamimusuhi_core::continuity::{
    ActivationOutcome, ContinuityStore, ExpectedHead, WriterIdentity,
};
use kamimusuhi_core::ids::{EvidenceId, MemoryId, ProposalId, SchemaVersion};
use kamimusuhi_core::memory::{
    MemoryLink, MemoryQuery, MemoryRepository, MemoryUsage, RecordUsage,
};
use kamimusuhi_core::mutation::{
    MutationDomain, MutationOperation, MutationPolicyV0, MutationProposal, OriginClass,
};
use kamimusuhi_core::time::UtcTimestamp;
use kamimusuhi_store_sqlite::SqliteStore;
use kamimusuhi_testkit::FixedClock;

fn episode(
    proposal_id: u128,
    expected_head: ExpectedHead,
    writer: WriterIdentity,
    candidate: serde_json::Value,
) -> MutationProposal {
    MutationProposal {
        proposal_id: ProposalId::from_u128(proposal_id),
        individual_id: INDIVIDUAL,
        domain: MutationDomain::Episodic,
        operation: MutationOperation::Capture,
        subject_key: None,
        candidate,
        expected_head,
        evidence_refs: vec![EVIDENCE],
        supersedes: None,
        origin_class: OriginClass::Reported,
        requested_by: writer,
        policy_version: MutationPolicyV0::VERSION,
        idempotency_key: format!("idem-{proposal_id}"),
        created_at: UtcTimestamp::from_unix_millis(FixedClock::BASELINE_UNIX_MILLIS),
    }
}

fn episodic_ids(store: &SqliteStore) -> Vec<MemoryId> {
    store
        .retrieve(&MemoryQuery::current(INDIVIDUAL).in_domain(MutationDomain::Episodic))
        .unwrap()
        .into_iter()
        .map(|m| m.record.state_record_id)
        .collect()
}

#[test]
fn schema_v6_creates_the_usage_side_table_on_an_existing_database() {
    let db = TempDb::new();
    {
        let store = open(&db.path, 1);
        bootstrap(&store, BOOT_A);
    }

    // Reopening an existing database applies 0006 cleanly and reports v6.
    let store = open(&db.path, 2);
    assert_eq!(store.schema_version(), SchemaVersion(6));
    assert!(
        store
            .table_names()
            .unwrap()
            .iter()
            .any(|t| t == "state_record_usage"),
        "state_record_usage must exist after migration"
    );

    // The side table enforces the FK to state_records.
    let raw = rusqlite::Connection::open(&db.path).unwrap();
    raw.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    let err = raw
        .execute(
            "INSERT INTO state_record_usage(state_record_id, last_used_at, use_count)
             VALUES ('missing', 0, 1)",
            [],
        )
        .unwrap_err();
    assert!(err.to_string().contains("FOREIGN KEY"), "{err}");
}

#[test]
fn declared_metadata_is_readable_from_the_stored_payload() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);
    let kernel = kernel(store);

    let candidate = serde_json::json!({
        "text": "お茶の好みについて話した",
        "tags": ["roadmap", "hai"],
        "links": [
            "evidence:000000000000000000000000000000e1",
            "state:00000000000000000000000000000099",
            "evidence:not-hex"
        ],
        "valid_from": 1_730_000_000,
        "valid_to": 1_732_000_000
    });
    let capture = episode(200, boot.head.expected(), boot.writer, candidate.clone());
    assert!(matches!(
        kernel.submit(&capture).unwrap(),
        ActivationOutcome::Activated(_)
    ));

    let record = kernel
        .store()
        .get_state_record(episodic_ids(kernel.store())[0])
        .unwrap()
        .expect("capture must be retrievable");

    // The write path is untouched: payload arrived verbatim, accessors parse it.
    assert_eq!(record.payload, candidate);
    assert_eq!(record.tags(), vec!["roadmap", "hai"]);
    assert_eq!(
        record.links(),
        vec![
            MemoryLink::Evidence(EvidenceId::from_u128(0xE1)),
            MemoryLink::State(MemoryId::from_u128(0x99)),
        ]
    );
    assert_eq!(
        record.valid_from(),
        Some(UtcTimestamp::from_unix_millis(1_730_000_000_000))
    );
    assert_eq!(
        record.valid_to(),
        Some(UtcTimestamp::from_unix_millis(1_732_000_000_000))
    );
    assert!(record.is_valid_at(UtcTimestamp::from_unix_millis(1_731_000_000_000)));
    assert!(!record.is_valid_at(UtcTimestamp::from_unix_millis(1_732_000_000_000)));
}

#[test]
fn usage_bookkeeping_upserts_survives_reopen_and_never_touches_the_record_row() {
    let db = TempDb::new();

    let (first, second) = {
        let store = open(&db.path, 1);
        let boot = bootstrap(&store, BOOT_A);
        let kernel = kernel(store);
        let one = episode(
            300,
            boot.head.expected(),
            boot.writer,
            serde_json::json!({ "text": "first" }),
        );
        assert!(matches!(
            kernel.submit(&one).unwrap(),
            ActivationOutcome::Activated(_)
        ));
        let head = kernel.store().load_head(INDIVIDUAL).unwrap();
        let two = episode(
            301,
            head.expected(),
            boot.writer,
            serde_json::json!({ "text": "second" }),
        );
        assert!(matches!(
            kernel.submit(&two).unwrap(),
            ActivationOutcome::Activated(_)
        ));
        let ids = episodic_ids(kernel.store());
        assert_eq!(ids.len(), 2);
        (ids[0], ids[1])
    };

    let now_a = UtcTimestamp::from_unix_millis(1_000_000);
    let now_b = UtcTimestamp::from_unix_millis(2_000_000);
    {
        let store = open(&db.path, 2);
        assert_eq!(store.usage(&first), None);
        store.mark_used(&[], now_a); // empty slice is a no-op
        store.mark_used(&[first], now_a);
        store.mark_used(&[first, second], now_b);
        assert_eq!(
            store.usage(&first),
            Some(RecordUsage {
                last_used_at: now_b,
                use_count: 2,
            })
        );
        assert_eq!(
            store.usage(&second),
            Some(RecordUsage {
                last_used_at: now_b,
                use_count: 1,
            })
        );
    }

    // A fresh "process" reads the same bookkeeping — it is durable.
    let store = open(&db.path, 3);
    assert_eq!(store.usage(&first).unwrap().use_count, 2);
    store.mark_used(&[first], UtcTimestamp::from_unix_millis(3_000_000));
    assert_eq!(store.usage(&first).unwrap().use_count, 3);

    // Batch read returns only the requested, existing ids — in request order.
    let usages = store.usages(&[second, first, MemoryId::from_u128(0xDEAD)]);
    assert_eq!(usages.len(), 2);
    assert_eq!(usages[0].0, second);
    assert_eq!(usages[1].0, first);
    assert_eq!(usages[1].1.use_count, 3);

    // The canonical row is untouched: created_at is still the activation
    // timestamp, and no usage column exists on state_records.
    let record = store.get_state_record(first).unwrap().unwrap();
    assert_eq!(
        record.created_at,
        UtcTimestamp::from_unix_millis(FixedClock::BASELINE_UNIX_MILLIS)
    );
    let raw = rusqlite::Connection::open(&db.path).unwrap();
    let columns: Vec<String> = raw
        .prepare("PRAGMA table_info(state_records)")
        .unwrap()
        .query_map([], |r| r.get::<_, String>(1))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(!columns.iter().any(|c| c.contains("use")), "{columns:?}");
}

#[test]
fn get_state_records_batches_and_preserves_request_order() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);
    let kernel = kernel(store);
    for (proposal_id, text) in [(400, "one"), (401, "two")] {
        let head = kernel.store().load_head(INDIVIDUAL).unwrap();
        let capture = episode(
            proposal_id,
            head.expected(),
            boot.writer,
            serde_json::json!({ "text": text }),
        );
        assert!(matches!(
            kernel.submit(&capture).unwrap(),
            ActivationOutcome::Activated(_)
        ));
    }
    let mut ids = episodic_ids(kernel.store());
    assert_eq!(ids.len(), 2);
    ids.reverse(); // request order differs from retrieval order
    let records = kernel.store().get_state_records(&ids);
    assert_eq!(
        records
            .iter()
            .map(|r| r.state_record_id)
            .collect::<Vec<_>>(),
        ids,
        "batch fetch must preserve the requested order"
    );
    // Absent ids are skipped, not errors.
    let partial = kernel
        .store()
        .get_state_records(&[ids[0], MemoryId::from_u128(0xDEAD)]);
    assert_eq!(partial.len(), 1);
    assert!(kernel.store().get_state_records(&[]).is_empty());
}
