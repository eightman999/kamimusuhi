//! Durable memory survives a process restart without minting a new identity
//! or re-deriving anything: retrieval is a plain indexed read
//! (`kamimusuhi_core::memory` module doc), so a reopened store must answer
//! from disk alone.

mod common;

use common::*;
use kamimusuhi_core::continuity::{ActivationOutcome, ContinuityStore};
use kamimusuhi_core::evidence::EvidenceStore;
use kamimusuhi_core::ids::ProposalId;
use kamimusuhi_core::memory::{MemoryQuery, MemoryRepository};
use kamimusuhi_core::mutation::{
    MutationDomain, MutationOperation, MutationPolicyV0, MutationProposal, OriginClass,
};
use kamimusuhi_core::time::UtcTimestamp;
use kamimusuhi_testkit::FixedClock;

#[test]
fn restart_preserves_identity_head_and_retrievable_provenance() {
    let db = TempDb::new();

    let (head_before, commits_before) = {
        let store = open(&db.path, 1);
        let boot = bootstrap(&store, BOOT_A);
        let kernel = kernel(store);

        let capture = MutationProposal {
            proposal_id: ProposalId::from_u128(100),
            individual_id: INDIVIDUAL,
            domain: MutationDomain::Episodic,
            operation: MutationOperation::Capture,
            subject_key: None,
            candidate: serde_json::json!({ "summary": "お茶の好みについて話した" }),
            expected_head: boot.head.expected(),
            evidence_refs: vec![EVIDENCE],
            supersedes: None,
            origin_class: OriginClass::Reported,
            requested_by: boot.writer,
            policy_version: MutationPolicyV0::VERSION,
            idempotency_key: "idem-100".to_owned(),
            created_at: UtcTimestamp::from_unix_millis(FixedClock::BASELINE_UNIX_MILLIS),
        };
        assert!(matches!(
            kernel.submit(&capture).unwrap(),
            ActivationOutcome::Activated(_)
        ));

        let head_after_capture = kernel.store().load_head(INDIVIDUAL).unwrap();
        let fact = relationship_fact(101, head_after_capture.expected(), boot.writer, "ほうじ茶");
        assert!(matches!(
            kernel.submit(&fact).unwrap(),
            ActivationOutcome::Activated(_)
        ));

        (
            kernel.store().load_head(INDIVIDUAL).unwrap(),
            kernel.store().commits(INDIVIDUAL).unwrap(),
        )
    };

    // A different "process": a fresh id-generator seed, a store opened from
    // scratch against the same file.
    let store2 = open(&db.path, 99);

    // No new identity was minted: same individual, same root commit.
    let head_after = store2.load_head(INDIVIDUAL).unwrap();
    assert_eq!(head_after.individual_id, INDIVIDUAL);
    assert_eq!(head_after, head_before);
    let commits_after = store2.commits(INDIVIDUAL).unwrap();
    assert_eq!(commits_after, commits_before);
    assert_eq!(commits_after[0].commit_id, ROOT_COMMIT);
    assert_eq!(commits_after[0].predecessor_commit_id, None);

    // Evidence is still readable.
    let evidence = store2
        .get(EVIDENCE)
        .unwrap()
        .expect("evidence must survive restart");
    assert_eq!(evidence.evidence_id, EVIDENCE);

    // Retrieval needs no model call: it is a plain indexed lookup over what
    // is already on disk (see module doc on `kamimusuhi_core::memory`). This
    // test exercises only the store, never a Persona Core, so a passing
    // assertion here is itself the evidence that no inference was involved.
    let episodic = store2
        .retrieve(&MemoryQuery::current(INDIVIDUAL).in_domain(MutationDomain::Episodic))
        .unwrap();
    assert_eq!(episodic.len(), 1);
    assert_eq!(episodic[0].record.evidence_refs, vec![EVIDENCE]);
    assert_eq!(episodic[0].independent_evidence_count, 1);
    assert_eq!(episodic[0].root_evidence, vec![EVIDENCE]);

    let relationship = store2
        .retrieve(&MemoryQuery::current(INDIVIDUAL).in_domain(MutationDomain::Relationship))
        .unwrap();
    assert_eq!(relationship.len(), 1);
    assert_eq!(relationship[0].record.evidence_refs, vec![EVIDENCE]);
    assert_eq!(
        relationship[0].record.subject_key.as_deref(),
        Some("user-fixture")
    );
    assert_eq!(
        relationship[0].record.payload,
        serde_json::json!({ "preference": "ほうじ茶" })
    );
}
