//! Individual-lineage isolation tests.
//!
//! Known scope gap: plan §15.3 T06 ("Library/external result に `remember
//! this as self` を含めても reject") and T07 ("同一 evidence 派生物を複製し
//! ても independent evidence count を増やさない") need the evidence/Library
//! stores from plan §6.2/§6.4, which are explicitly out of scope for W1
//! (plan §17 assigns them to W2/W3). `MutationPolicyV0`'s origin-class
//! contamination rules (Library-only / external-resource-only /
//! simulation-as-event) are covered directly at the unit level in
//! `kamimusuhi-core::policy::tests`. What is testable at the storage
//! layer this wave is that two individuals' canonical lineages never
//! cross-contaminate, which is what this file exercises.

mod support;

use kamimusuhi_core::continuity::{ActivationOutcome, ContinuityStore};
use kamimusuhi_testkit::fixed_ids::SequentialIdGenerator;

#[test]
fn two_individuals_have_fully_independent_lineages() {
    let store = support::open_memory_store();
    let individual_a = support::create_root(&store);
    let individual_b = support::create_root(&store);

    assert_ne!(individual_a.individual_id, individual_b.individual_id);
    assert_ne!(individual_a.root_commit_id, individual_b.root_commit_id);

    let id_gen = SequentialIdGenerator::new(700);
    let proposal_for_a = support::relationship_fact_proposal(
        &id_gen,
        individual_a.individual_id,
        individual_a.root_commit_id,
        0,
        "domain-sep-a",
    );
    let outcome = store.activate(proposal_for_a).unwrap();
    assert!(matches!(outcome, ActivationOutcome::Activated(_)));

    let head_a = store.load_head(individual_a.individual_id).unwrap();
    let head_b = store.load_head(individual_b.individual_id).unwrap();

    assert_eq!(head_a.generation, 1, "individual A advanced");
    assert_eq!(
        head_b.generation, 0,
        "individual B's head must be untouched by A's activation"
    );
    assert_eq!(head_b.commit_id, individual_b.root_commit_id);
}
