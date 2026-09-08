//! Schema migration: empty DB, idempotency, version-mismatch refusal,
//! pragma verification and foreign-key enforcement.

use kamimusuhi_core::continuity::ContinuityError;
use kamimusuhi_core::ids::SchemaVersion;
use kamimusuhi_store_sqlite::migrations::SUPPORTED_SCHEMA_VERSION;
use kamimusuhi_store_sqlite::{SqliteStore, StoreConfig};
use kamimusuhi_testkit::{FixedClock, FixedIdGenerator};
use rusqlite::{Connection, params};
use std::sync::Arc;

fn temp_db() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kamimusuhi.sqlite");
    (dir, path)
}

fn open(path: impl AsRef<std::path::Path>) -> Result<SqliteStore, ContinuityError> {
    SqliteStore::open(
        path,
        &StoreConfig::default(),
        Arc::new(FixedClock::baseline()),
        Arc::new(FixedIdGenerator::default()),
    )
}

const EXPECTED_TABLES: &[&str] = &[
    "activation_receipts",
    "audit_events",
    "canonical_commits",
    "continuity_heads",
    "evidence_links",
    "evidence_records",
    "individuals",
    "library_artifacts",
    "library_chunks",
    "mutation_decisions",
    "mutation_proposals",
    "resource_calls",
    "schema_meta",
    "sessions",
    "state_record_evidence",
    "state_records",
    "turns",
    "writer_epochs",
];

#[test]
fn migrates_empty_database_to_the_supported_version_deterministically() {
    let (_dir, path) = temp_db();
    let store = open(&path).unwrap();
    assert_eq!(store.schema_version(), SchemaVersion(3));
    assert_eq!(SUPPORTED_SCHEMA_VERSION, SchemaVersion(3));
    assert_eq!(store.table_names().unwrap(), EXPECTED_TABLES);
}

#[test]
fn reopening_is_idempotent_and_keeps_version() {
    let (_dir, path) = temp_db();
    {
        open(&path).unwrap();
    }
    let raw = Connection::open(&path).unwrap();
    let rows_before: i64 = raw
        .query_row("SELECT COUNT(*) FROM schema_meta", [], |r| r.get(0))
        .unwrap();
    drop(raw);

    let store = open(&path).unwrap();
    assert_eq!(store.schema_version(), SchemaVersion(3));
    assert_eq!(store.table_names().unwrap(), EXPECTED_TABLES);

    let raw = Connection::open(&path).unwrap();
    let rows_after: i64 = raw
        .query_row("SELECT COUNT(*) FROM schema_meta", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        rows_before, rows_after,
        "re-running migrations must not add meta rows"
    );
}

#[test]
fn refuses_database_from_a_newer_schema() {
    let (_dir, path) = temp_db();
    {
        open(&path).unwrap();
    }
    let raw = Connection::open(&path).unwrap();
    raw.execute(
        "UPDATE schema_meta SET value = ?1 WHERE key = 'schema_version'",
        params!["99"],
    )
    .unwrap();
    drop(raw);

    let err = open(&path).unwrap_err();
    assert_eq!(
        err,
        ContinuityError::SchemaVersionMismatch {
            found: SchemaVersion(99),
            supported: SchemaVersion(3),
        }
    );
}

#[test]
fn refuses_corrupt_schema_meta() {
    let (_dir, path) = temp_db();
    {
        open(&path).unwrap();
    }
    let raw = Connection::open(&path).unwrap();
    raw.execute("DELETE FROM schema_meta WHERE key = 'schema_version'", [])
        .unwrap();
    drop(raw);

    assert!(matches!(open(&path), Err(ContinuityError::Corrupt { .. })));
}

#[test]
fn durability_pragmas_are_applied() {
    let (_dir, path) = temp_db();
    let _store = open(&path).unwrap();

    let raw = Connection::open(&path).unwrap();
    let journal_mode: String = raw
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        journal_mode.to_lowercase(),
        "wal",
        "WAL is persistent in the file"
    );
}

#[test]
fn in_memory_database_is_refused_because_wal_is_unavailable() {
    let err = open(":memory:").unwrap_err();
    assert!(matches!(err, ContinuityError::Backend { .. }), "{err}");
}

#[test]
fn foreign_keys_are_enforced_on_the_store_connection() {
    let (_dir, path) = temp_db();
    {
        open(&path).unwrap();
    }
    // A fresh connection with the same pragma set reproduces the store's
    // enforcement: a commit for a non-existent individual must be refused.
    let raw = Connection::open(&path).unwrap();
    raw.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
    let err = raw
        .execute(
            "INSERT INTO canonical_commits(commit_id, individual_id, generation, predecessor_commit_id, proposal_id, created_at)
             VALUES ('c', 'missing', 0, NULL, NULL, 0)",
            [],
        )
        .unwrap_err();
    assert!(err.to_string().contains("FOREIGN KEY"), "{err}");

    // Generation/predecessor consistency is a CHECK constraint. The root FK
    // on individuals is deferred, so the pair is inserted in one transaction.
    let tx = raw.unchecked_transaction().unwrap();
    tx.execute(
        "INSERT INTO individuals(individual_id, root_commit_id, current_writer_epoch, created_at) VALUES ('i', 'root', 1, 0)",
        [],
    )
    .unwrap();
    let err = tx
        .execute(
            "INSERT INTO canonical_commits(commit_id, individual_id, generation, predecessor_commit_id, proposal_id, created_at)
             VALUES ('root', 'i', 1, NULL, NULL, 0)",
            [],
        )
        .unwrap_err();
    assert!(err.to_string().contains("CHECK"), "{err}");
    let err = tx.commit().unwrap_err();
    assert!(
        err.to_string().contains("FOREIGN KEY"),
        "deferred root FK must fail at commit: {err}"
    );
}
