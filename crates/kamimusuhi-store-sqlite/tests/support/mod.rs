//! Shared fixtures for the continuity integration tests.
//!
//! Each integration test binary compiles this module independently and
//! typically uses only a subset of these helpers, so unused-function
//! warnings here are expected rather than a sign of dead code in the
//! crate itself.
#![allow(dead_code)]

use kamimusuhi_core::continuity::Individual;
use kamimusuhi_core::ids::{
    CommitId, EvidenceId, IdGenerator, IndividualId, PolicyVersion, ProposalId,
};
use kamimusuhi_core::mutation::{MutationDomain, MutationOperation, MutationProposal, OriginClass};
use kamimusuhi_core::time::UtcTimestamp;
use kamimusuhi_store_sqlite::SqliteContinuityStore;
use kamimusuhi_testkit::fixed_clock::FixedClock;
use kamimusuhi_testkit::fixed_ids::SequentialIdGenerator;

pub fn open_store_at(path: &std::path::Path) -> SqliteContinuityStore {
    SqliteContinuityStore::open(
        path,
        Box::new(FixedClock::at("2026-09-08T00:00:00Z")),
        Box::new(SequentialIdGenerator::new(1)),
    )
    .expect("store opens")
}

pub fn open_memory_store() -> SqliteContinuityStore {
    SqliteContinuityStore::open_in_memory(
        Box::new(FixedClock::at("2026-09-08T00:00:00Z")),
        Box::new(SequentialIdGenerator::new(1)),
    )
    .expect("store opens")
}

/// Builds a well-formed relationship-fact proposal targeting the given
/// expected predecessor. `idempotency_key` should be unique per logical
/// proposal in a test unless the test is deliberately exercising retry
/// behaviour.
pub fn relationship_fact_proposal(
    id_gen: &dyn IdGenerator,
    individual_id: IndividualId,
    expected_commit_id: CommitId,
    expected_generation: u64,
    idempotency_key: &str,
) -> MutationProposal {
    MutationProposal {
        proposal_id: ProposalId::new(id_gen),
        individual_id,
        domain: MutationDomain::Relationship,
        operation: MutationOperation::Fact,
        candidate: serde_json::json!({ "preference": "hoji-cha" }),
        evidence_refs: vec![EvidenceId::new(id_gen)],
        expected_commit_id: Some(expected_commit_id),
        expected_generation,
        origin_class: OriginClass::CanonicalInteraction,
        requested_by: "fake-persona-core".to_string(),
        policy_version: PolicyVersion(0),
        idempotency_key: idempotency_key.to_string(),
        created_at: UtcTimestamp::parse_rfc3339("2026-09-08T00:00:00Z").unwrap(),
    }
}

pub fn create_root(store: &SqliteContinuityStore) -> Individual {
    store.create_root_individual().expect("root creation")
}
