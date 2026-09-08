//! Shared fixtures for store integration tests.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kamimusuhi_core::continuity::{
    ContinuityError, ContinuityKernel, ContinuityStore, ExpectedHead, IndividualBootstrap,
    NewIndividual, WriterIdentity,
};
use kamimusuhi_core::ids::{BootId, CommitId, EvidenceId, IndividualId, NodeId, ProposalId};
use kamimusuhi_core::mutation::{
    MutationDomain, MutationOperation, MutationPolicyV0, MutationProposal, OriginClass,
};
use kamimusuhi_core::time::UtcTimestamp;
use kamimusuhi_store_sqlite::{SqliteStore, StoreConfig};
use kamimusuhi_testkit::{FixedClock, FixedIdGenerator};

pub const INDIVIDUAL: IndividualId = IndividualId::from_u128(0xA1);
pub const ROOT_COMMIT: CommitId = CommitId::from_u128(0xC0);
pub const NODE: NodeId = NodeId::from_u128(0x0E);
pub const BOOT_A: BootId = BootId::from_u128(0xB1);
pub const BOOT_B: BootId = BootId::from_u128(0xB2);
pub const EVIDENCE: EvidenceId = EvidenceId::from_u128(0xE1);

pub struct TempDb {
    _dir: tempfile::TempDir,
    pub path: PathBuf,
}

impl TempDb {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kamimusuhi.sqlite");
        Self { _dir: dir, path }
    }
}

pub fn clock() -> Arc<FixedClock> {
    Arc::new(FixedClock::baseline())
}

/// Open a store with a fixed clock and a fixed ID sequence seeded per opener
/// so two "processes" never generate colliding IDs.
pub fn open_store(path: &Path, id_seed: u64) -> Result<SqliteStore, ContinuityError> {
    SqliteStore::open(
        path,
        &StoreConfig::default(),
        clock(),
        Arc::new(FixedIdGenerator::new(id_seed)),
    )
}

pub fn open(path: &Path, id_seed: u64) -> SqliteStore {
    open_store(path, id_seed).unwrap()
}

pub fn bootstrap(store: &SqliteStore, boot: BootId) -> IndividualBootstrap {
    store
        .create_individual(NewIndividual {
            individual_id: INDIVIDUAL,
            root_commit_id: ROOT_COMMIT,
            node_id: NODE,
            boot_id: boot,
        })
        .unwrap()
}

pub type Kernel = ContinuityKernel<SqliteStore, MutationPolicyV0, FixedClock>;

pub fn kernel(store: SqliteStore) -> Kernel {
    ContinuityKernel::new(store, MutationPolicyV0, FixedClock::baseline())
}

pub fn relationship_fact(
    proposal_id: u128,
    expected_head: ExpectedHead,
    writer: WriterIdentity,
    preference: &str,
) -> MutationProposal {
    MutationProposal {
        proposal_id: ProposalId::from_u128(proposal_id),
        individual_id: INDIVIDUAL,
        domain: MutationDomain::Relationship,
        operation: MutationOperation::Fact,
        subject_key: Some("user-fixture".to_owned()),
        candidate: serde_json::json!({ "preference": preference }),
        expected_head,
        evidence_refs: vec![EVIDENCE],
        origin_class: OriginClass::Reported,
        requested_by: writer,
        policy_version: MutationPolicyV0::VERSION,
        idempotency_key: format!("idem-{proposal_id}"),
        created_at: UtcTimestamp::from_unix_millis(FixedClock::BASELINE_UNIX_MILLIS),
    }
}

/// Raw row counts for consistency assertions, read through an independent
/// connection so the store's own state cannot mask what is on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowCounts {
    pub commits: i64,
    pub heads: i64,
    pub proposals: i64,
    pub decisions: i64,
    pub receipts: i64,
    pub audits: i64,
    pub head_generation: i64,
}

pub fn row_counts(path: &Path) -> RowCounts {
    let conn = rusqlite::Connection::open(path).unwrap();
    let count = |table: &str| -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    };
    let head_generation: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(generation), -1) FROM continuity_heads",
            [],
            |r| r.get(0),
        )
        .unwrap();
    RowCounts {
        commits: count("canonical_commits"),
        heads: count("continuity_heads"),
        proposals: count("mutation_proposals"),
        decisions: count("mutation_decisions"),
        receipts: count("activation_receipts"),
        audits: count("audit_events"),
        head_generation,
    }
}
