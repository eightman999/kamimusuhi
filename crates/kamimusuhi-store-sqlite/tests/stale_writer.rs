//! Stale-writer and lock-vs-identity-conflict tests (plan §7.2 invariant
//! 7, §15.3 T02).
//!
//! Known scope gap: plan §15.3 T03 ("old `writer_epoch`/`boot_id` fixture
//! の mutation を拒否") is not exercised here. `MutationProposal` in this
//! wave does not carry a writer epoch/boot id field, and
//! `MutationPolicyV0` has no epoch-fencing rule yet -- both are new
//! surface area the plan does not specify for W1's minimal proposal
//! shape. This is left for the wave that introduces multi-writer/epoch
//! fencing (plan §22 "Continuity authority -> 後段: multi-node writer
//! fencing").

mod support;

use std::sync::Arc;
use std::thread;

use kamimusuhi_core::continuity::{ActivationOutcome, ContinuityError, ContinuityStore};
use kamimusuhi_core::mutation::ReasonCode;
use kamimusuhi_testkit::fixed_ids::SequentialIdGenerator;

#[test]
fn concurrent_writers_from_the_same_generation_only_one_commits() {
    // Real concurrency (OS threads), not just sequential calls: two
    // threads race to activate a proposal built against the same
    // expected generation. The `BEGIN IMMEDIATE` writer lock must
    // serialize them so exactly one wins and the other observes a stale
    // predecessor -- never two commits at generation 1.
    let store = Arc::new(support::open_memory_store());
    let individual = support::create_root(&store);

    let id_gen_a = SequentialIdGenerator::new(10);
    let id_gen_b = SequentialIdGenerator::new(20);
    let proposal_a = support::relationship_fact_proposal(
        &id_gen_a,
        individual.individual_id,
        individual.root_commit_id,
        0,
        "concurrent-a",
    );
    let proposal_b = support::relationship_fact_proposal(
        &id_gen_b,
        individual.individual_id,
        individual.root_commit_id,
        0,
        "concurrent-b",
    );

    let store_a = Arc::clone(&store);
    let handle_a = thread::spawn(move || store_a.activate(proposal_a));
    let store_b = Arc::clone(&store);
    let handle_b = thread::spawn(move || store_b.activate(proposal_b));

    let result_a = handle_a.join().unwrap().unwrap();
    let result_b = handle_b.join().unwrap().unwrap();

    let outcomes = [result_a, result_b];
    let activated = outcomes
        .iter()
        .filter(|o| matches!(o, ActivationOutcome::Activated(_)))
        .count();
    let rejected = outcomes
        .iter()
        .filter(|o| {
            matches!(
                o,
                ActivationOutcome::Rejected {
                    reason_code: ReasonCode::StalePredecessor,
                    ..
                }
            )
        })
        .count();

    assert_eq!(activated, 1, "exactly one concurrent proposal may win");
    assert_eq!(
        rejected, 1,
        "the loser must be a stale-predecessor rejection"
    );

    let head = store.load_head(individual.individual_id).unwrap();
    assert_eq!(head.generation, 1);
    assert_eq!(
        store
            .count_commits_for_individual(individual.individual_id)
            .unwrap(),
        2
    );
}

#[test]
fn sqlite_lock_contention_is_reported_as_busy_not_identity_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("busy.sqlite");
    let store = support::open_store_at(&db_path);
    let individual = support::create_root(&store);

    // A second, independent connection to the same file holds the
    // writer lock without committing, simulating another process mid
    // transaction.
    let blocker = rusqlite::Connection::open(&db_path).unwrap();
    blocker
        .busy_timeout(std::time::Duration::from_millis(0))
        .unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE;").unwrap();

    let id_gen = SequentialIdGenerator::new(30);
    let proposal = support::relationship_fact_proposal(
        &id_gen,
        individual.individual_id,
        individual.root_commit_id,
        0,
        "busy-case",
    );

    let result = store.activate(proposal.clone());
    match result {
        Err(ContinuityError::Busy(_)) => {}
        other => panic!("expected ContinuityError::Busy, got {other:?}"),
    }

    // Release the blocking transaction and confirm the identical
    // proposal now succeeds normally -- the earlier failure was a lock,
    // not a rejected/consumed proposal.
    blocker.execute_batch("ROLLBACK;").unwrap();
    let outcome = store.activate(proposal).unwrap();
    assert!(matches!(outcome, ActivationOutcome::Activated(_)));
}
