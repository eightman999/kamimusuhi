//! Replacing a cognitive resource is a substitution, never an identity
//! migration (issue #16 Wave 3, plan §7, §9.2).
//!
//! Swapping the resource behind a slot must change the material a call
//! returns and its attribution, and change nothing else: not the individual,
//! not the continuity head, not any durable memory. Both the successful and
//! the failed call must be durably recorded, and a restart must not lose any
//! of it.

mod common;

use std::sync::Arc;

use common::*;
use kamimusuhi_core::continuity::{ActivationOutcome, ContinuityStore};
use kamimusuhi_core::ids::{ProposalId, ResourceCallId};
use kamimusuhi_core::memory::{MemoryQuery, MemoryRepository};
use kamimusuhi_core::mutation::{
    MutationDomain, MutationOperation, MutationPolicyV0, MutationProposal, OriginClass,
};
use kamimusuhi_core::resources::{
    ResourceCallLog, ResourceOutcome, ResourceRegistry, ResourceRequest, ResourceSlot,
};
use kamimusuhi_core::time::UtcTimestamp;
use kamimusuhi_testkit::fake_resource::{
    FAKE_A_RESOURCE_ID, FAKE_B_RESOURCE_ID, UNAVAILABLE_RESOURCE_ID,
};
use kamimusuhi_testkit::{FakeResource, FixedClock, UnavailableResource};

fn request() -> ResourceRequest {
    ResourceRequest::new(
        INDIVIDUAL,
        "summarize-turn",
        serde_json::json!({ "text": "お茶の話" }),
    )
}

#[test]
fn replacing_the_resource_does_not_change_the_individual() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);
    let kernel = kernel(store);

    // Durable memory to compare before and after: one relationship fact and
    // one episodic capture.
    let fact = relationship_fact(100, boot.head.expected(), boot.writer, "ほうじ茶");
    let ActivationOutcome::Activated(_) = kernel.submit(&fact).unwrap() else {
        panic!("expected activation");
    };
    let head_after_fact = kernel.store().load_head(INDIVIDUAL).unwrap();

    let capture = MutationProposal {
        proposal_id: ProposalId::from_u128(101),
        individual_id: INDIVIDUAL,
        domain: MutationDomain::Episodic,
        operation: MutationOperation::Capture,
        subject_key: None,
        candidate: serde_json::json!({ "summary": "お茶の好みについて話した" }),
        expected_head: head_after_fact.expected(),
        evidence_refs: vec![EVIDENCE],
        supersedes: None,
        origin_class: OriginClass::Reported,
        requested_by: boot.writer,
        policy_version: MutationPolicyV0::VERSION,
        idempotency_key: "idem-101".to_owned(),
        created_at: UtcTimestamp::from_unix_millis(FixedClock::BASELINE_UNIX_MILLIS),
    };
    let ActivationOutcome::Activated(_) = kernel.submit(&capture).unwrap() else {
        panic!("expected activation");
    };

    let individual_before = INDIVIDUAL;
    let root_commit_before = ROOT_COMMIT;
    let head_before = kernel.store().load_head(INDIVIDUAL).unwrap();
    let memory_before = kernel
        .store()
        .retrieve(&MemoryQuery::current(INDIVIDUAL).including_history())
        .unwrap();

    // Register Fake A, invoke, replace with Fake B, invoke the same request.
    let mut registry = ResourceRegistry::new();
    let slot = ResourceSlot::new("reasoning");
    registry
        .register(slot.clone(), Arc::new(FakeResource::a()))
        .unwrap();

    let first = registry
        .invoke(
            &slot,
            &request(),
            kernel.store(),
            ResourceCallId::from_u128(1),
            UtcTimestamp::from_unix_millis(1_000),
            UtcTimestamp::from_unix_millis(1_100),
        )
        .unwrap();

    let previous = registry
        .replace(slot.clone(), Arc::new(FakeResource::b()))
        .unwrap()
        .expect("the slot was filled");
    assert_eq!(previous.resource_id, FAKE_A_RESOURCE_ID);

    let second = registry
        .invoke(
            &slot,
            &request(),
            kernel.store(),
            ResourceCallId::from_u128(2),
            UtcTimestamp::from_unix_millis(2_000),
            UtcTimestamp::from_unix_millis(2_100),
        )
        .unwrap();

    // Content and attribution changed.
    assert_ne!(first.result.content, second.result.content);
    assert_eq!(first.result.resource_id, FAKE_A_RESOURCE_ID);
    assert_eq!(first.call.resource_id, FAKE_A_RESOURCE_ID);
    assert_eq!(second.result.resource_id, FAKE_B_RESOURCE_ID);
    assert_eq!(second.call.resource_id, FAKE_B_RESOURCE_ID);
    // Same question either way, different answer.
    assert_eq!(first.call.request_digest, second.call.request_digest);
    assert_ne!(first.call.result_digest, second.call.result_digest);

    // Identity, lineage and durable memory are untouched by the replacement.
    assert_eq!(individual_before, INDIVIDUAL);
    assert_eq!(root_commit_before, ROOT_COMMIT);
    let head_after = kernel.store().load_head(INDIVIDUAL).unwrap();
    assert_eq!(head_before, head_after);
    let memory_after = kernel
        .store()
        .retrieve(&MemoryQuery::current(INDIVIDUAL).including_history())
        .unwrap();
    assert_eq!(memory_before, memory_after);

    // Both calls are durably recorded, in order, with the right attribution.
    let calls = ResourceCallLog::calls(kernel.store(), INDIVIDUAL).unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].resource_call_id, first.call.resource_call_id);
    assert_eq!(calls[0].resource_id, FAKE_A_RESOURCE_ID);
    assert_eq!(calls[0].slot, slot);
    assert_eq!(calls[0].adapter, "fake");
    assert_eq!(calls[0].purpose, "summarize-turn");
    assert_eq!(calls[1].resource_call_id, second.call.resource_call_id);
    assert_eq!(calls[1].resource_id, FAKE_B_RESOURCE_ID);
    assert_eq!(calls[1].slot, slot);
    assert_eq!(calls[1].adapter, "fake");
    assert_eq!(calls[1].purpose, "summarize-turn");

    let counts = row_counts(&db.path);
    assert_eq!(counts.commits, 3, "root + fact + capture, no more");

    // A failed call, in a second slot, still records an error row.
    let failing_slot = ResourceSlot::new("unavailable");
    registry
        .register(failing_slot.clone(), Arc::new(UnavailableResource))
        .unwrap();
    let head_before_failure = kernel.store().load_head(INDIVIDUAL).unwrap();
    let failure = registry.invoke(
        &failing_slot,
        &request(),
        kernel.store(),
        ResourceCallId::from_u128(3),
        UtcTimestamp::from_unix_millis(3_000),
        UtcTimestamp::from_unix_millis(3_100),
    );
    assert!(failure.is_err());
    let head_after_failure = kernel.store().load_head(INDIVIDUAL).unwrap();
    assert_eq!(head_before_failure, head_after_failure);

    let calls = ResourceCallLog::calls(kernel.store(), INDIVIDUAL).unwrap();
    assert_eq!(calls.len(), 3);
    let error_call = &calls[2];
    assert_eq!(error_call.resource_id, UNAVAILABLE_RESOURCE_ID);
    assert_eq!(error_call.outcome, ResourceOutcome::Error);
    assert!(error_call.error_code.is_some());
    assert_eq!(error_call.result_digest, None);

    let counts_after_failure = row_counts(&db.path);
    assert_eq!(
        counts_after_failure.commits, counts.commits,
        "a resource call, successful or not, never writes a canonical commit"
    );

    // Reopen from the same path: head, individual and memory are unchanged,
    // and the call records survived.
    drop(kernel);
    let reopened = open(&db.path, 2);
    let head_reopened = ContinuityStore::load_head(&reopened, INDIVIDUAL).unwrap();
    assert_eq!(head_reopened, head_after_failure);
    let memory_reopened = reopened
        .retrieve(&MemoryQuery::current(INDIVIDUAL).including_history())
        .unwrap();
    assert_eq!(memory_reopened, memory_after);
    let calls_reopened = ResourceCallLog::calls(&reopened, INDIVIDUAL).unwrap();
    assert_eq!(calls_reopened, calls);
}
