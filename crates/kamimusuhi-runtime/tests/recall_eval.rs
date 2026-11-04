//! Declared-link recall evaluation end to end: for each link-declaring
//! episodic record the evaluator rebuilds the world as it existed just
//! before the record's creation and scores the declared targets against
//! what the recall path would have offered. Records created after T must
//! never surface, and a record with no prior utterance falls back to its
//! own text — flagged in the report.

use std::time::Duration;

use kamimusuhi_core::digest::content_digest;
use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceSource, EvidenceStore, NewEvidence, RetentionClass,
};
use kamimusuhi_core::ids::{EvidenceId, MemoryId, ProposalId};
use kamimusuhi_core::memory::{MemoryQuery, MemoryRepository};
use kamimusuhi_core::mutation::{
    MutationDomain, MutationOperation, MutationPolicyV0, MutationProposal, OriginClass,
};
use kamimusuhi_runtime::recall_eval::{self, QuerySource, RecallEvalRow};
use kamimusuhi_runtime::{ResourceImplementation, Runtime, RuntimeOptions};
use serde_json::json;

const SOURCE_ID: &str = "text-chat:alice";
const QUERY: &str = "通勤手段はロードバイクです";

/// Deterministic ids on a real clock: `created_at` drives the pre-T world
/// cut, so every fixture write gets its own timestamp.
fn settle() {
    std::thread::sleep(Duration::from_millis(5));
}

fn append_utterance(runtime: &Runtime, text: &str) -> EvidenceId {
    let evidence_id = EvidenceId::generate(runtime.ids().as_ref());
    runtime
        .store()
        .append(NewEvidence {
            evidence_id,
            individual_id: runtime.individual_id(),
            session_id: None,
            turn_id: None,
            kind: EvidenceKind::UserUtterance,
            origin_class: OriginClass::Reported,
            payload: json!({"text": text}),
            source: EvidenceSource {
                source_id: Some(SOURCE_ID.to_owned()),
                source_sequence: None,
                content_digest: Some(content_digest(text.as_bytes())),
            },
            retention_class: RetentionClass::Standard,
        })
        .unwrap();
    evidence_id
}

/// Activate one state record through the canonical path and return the id
/// the activation minted (looked up by the payload's marker).
fn activate(
    runtime: &Runtime,
    domain: MutationDomain,
    subject: &str,
    candidate: serde_json::Value,
    evidence_refs: Vec<EvidenceId>,
) -> MemoryId {
    let writer = runtime.claim_writer().unwrap();
    let proposal_id = ProposalId::generate(runtime.ids().as_ref());
    let proposal = MutationProposal {
        proposal_id,
        individual_id: runtime.individual_id(),
        domain,
        operation: MutationOperation::Capture,
        subject_key: Some(subject.to_owned()),
        candidate: candidate.clone(),
        expected_head: runtime.head().unwrap().expected(),
        evidence_refs,
        supersedes: None,
        origin_class: OriginClass::Reported,
        requested_by: writer,
        policy_version: MutationPolicyV0::VERSION,
        idempotency_key: format!("recall-eval-fixture-{proposal_id}"),
        created_at: runtime.now(),
    };
    assert!(
        runtime
            .kernel()
            .submit(&proposal)
            .unwrap()
            .receipt()
            .is_some(),
        "the fixture record must activate"
    );
    MemoryRepository::retrieve(
        runtime.store(),
        &MemoryQuery::current(runtime.individual_id())
            .in_domain(domain)
            .about(subject),
    )
    .unwrap()
    .into_iter()
    .find(|memory| memory.record.payload == candidate)
    .map(|memory| memory.record.state_record_id)
    .expect("the just-activated record must be retrievable")
}

fn row(report: &recall_eval::RecallEvalReport, id: MemoryId) -> &RecallEvalRow {
    report
        .records
        .iter()
        .find(|row| row.state_record_id == id)
        .expect("every link-declaring record must be evaluated")
}

#[test]
fn declared_targets_are_scored_against_the_world_at_creation_time() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Runtime::init(
        dir.path(),
        RuntimeOptions::deterministic_ids(81),
        ResourceImplementation::FakeA,
    )
    .unwrap();

    // Timeline, oldest first:
    //   U   user utterance — the input preceding every evaluated record
    //   B   target record: no lexical overlap, reachable only via links
    //   S   lexical seed declaring state:B and the shared tag
    //   TG  shares S's tag — the tag-frame hit
    //   P   second record pointing at B — together with S it makes B a knot
    //   A   evaluated record declaring state:B, evidence:U and state:TG
    //   C   another-subject record declaring state:B (never surfaceable)
    //   L   post-A lexical match — must stay out of A's world
    let utterance = append_utterance(&runtime, QUERY);
    settle();
    let target_b = activate(
        &runtime,
        MutationDomain::Episodic,
        "alice",
        json!({"text": "xyzzy plugh", "marker": "b"}),
        vec![utterance],
    );
    settle();
    let seed_s = activate(
        &runtime,
        MutationDomain::Episodic,
        "alice",
        json!({
            "text": QUERY,
            "links": [format!("state:{target_b}")],
            "tags": ["commute"],
            "marker": "s",
        }),
        vec![utterance],
    );
    settle();
    let tag_tg = activate(
        &runtime,
        MutationDomain::Episodic,
        "alice",
        json!({"text": "pomegranate sprocket", "tags": ["commute"], "marker": "tg"}),
        vec![utterance],
    );
    settle();
    let pointer_p = activate(
        &runtime,
        MutationDomain::Episodic,
        "alice",
        json!({"text": "quux corge", "links": [format!("state:{target_b}")], "marker": "p"}),
        vec![utterance],
    );
    settle();
    let record_a = activate(
        &runtime,
        MutationDomain::Episodic,
        "alice",
        json!({
            "text": "grault",
            "links": [
                format!("state:{target_b}"),
                format!("evidence:{utterance}"),
                format!("state:{tag_tg}"),
            ],
            "marker": "a",
        }),
        vec![utterance],
    );
    settle();
    let record_c = activate(
        &runtime,
        MutationDomain::Episodic,
        "bob",
        json!({"text": "fred", "links": [format!("state:{target_b}")], "marker": "c"}),
        vec![utterance],
    );
    settle();
    let late_l = activate(
        &runtime,
        MutationDomain::Episodic,
        "alice",
        json!({
            "text": QUERY,
            "links": [format!("state:{target_b}")],
            "marker": "l",
        }),
        vec![utterance],
    );

    let report = recall_eval::evaluate(&runtime, None).unwrap();

    // S, P, A, C and L each declared a link; B and TG declared none.
    let summary = &report.summary;
    assert_eq!(summary.evaluated, 5);
    assert!(summary.mean_recall > 0.0 && summary.any_hit_rate > 0.0);
    assert!(!report.limits.is_empty());

    // A at T: S is the lexical seed, so B surfaces through the link frame
    // (in-degree 2 — S and P — a knot), the utterance through a citing
    // record, and TG through the tag frame.
    let a = row(&report, record_a);
    assert_eq!(a.query_source, QuerySource::PriorUserUtterance);
    assert_eq!(a.query_text, QUERY);
    assert_eq!(a.query_evidence_id, Some(utterance));
    assert_eq!(a.targets.len(), 3);
    let by_link = |link: &str| a.targets.iter().find(|t| t.link == link).unwrap();
    let b_hit = by_link(&format!("state:{target_b}"));
    assert!(b_hit.hit);
    assert_eq!(b_hit.via.as_deref(), Some("link_hop_1"));
    assert_eq!(b_hit.in_degree, Some(2), "S and P both declare state:B");
    let evidence_hit = by_link(&format!("evidence:{utterance}"));
    assert!(evidence_hit.hit);
    assert!(
        evidence_hit.resolved_to.contains(&seed_s) && evidence_hit.resolved_to.contains(&target_b),
        "the evidence target resolves to its pre-T citers"
    );
    let tg_hit = by_link(&format!("state:{tag_tg}"));
    assert!(tg_hit.hit);
    assert_eq!(tg_hit.via.as_deref(), Some("tag"));
    assert_eq!(a.hit_ratio, 1.0);
    assert!((a.declared_hit_ratio - 2.0 / 3.0).abs() < 1e-9);
    assert!(a.any_hit);

    // The post-T record L must not surface in A's world even though it is
    // a perfect lexical match for the replayed query.
    let surfaced: Vec<MemoryId> = a.surfaced.iter().map(|s| s.state_record_id).collect();
    assert!(
        !surfaced.contains(&late_l),
        "a record created after T must never surface: {surfaced:?}"
    );
    // Nor may the evaluated record itself appear in its own offer set.
    assert!(!surfaced.contains(&record_a));

    // P's world already contains S and B: the seed walks to B.
    assert_eq!(row(&report, pointer_p).hit_ratio, 1.0);

    // S declared state:B before S existed as a seed: the world holds B
    // alone, nothing seeds, the target misses. Recall is a lower bound —
    // the miss is honest.
    let s = row(&report, seed_s);
    assert_eq!(s.hit_ratio, 0.0);
    assert!(!s.any_hit);

    // C is about another subject with no prior utterance: the query falls
    // back to the record's own text, the subject-bound world is empty, and
    // the cross-subject declared target can never surface.
    let c = row(&report, record_c);
    assert_eq!(c.query_source, QuerySource::RecordTextFallback);
    assert_eq!(c.hit_ratio, 0.0);
    assert_eq!(c.targets.len(), 1);
    assert!(!c.targets[0].hit);

    // L is the newest evaluated record: its world contains S->B, so the
    // same declared link that missed for S hits here.
    let l = row(&report, late_l);
    assert_eq!(l.hit_ratio, 1.0);
    assert_eq!(l.targets[0].via.as_deref(), Some("link_hop_1"));

    // Frame-level accounting across all rows.
    assert_eq!(
        summary.link_frame_hits, 3,
        "B via the link frame for P, A and L"
    );
    assert_eq!(summary.tag_frame_hits, 1, "TG via the tag frame for A");
    assert_eq!(
        summary.knot_hits, 2,
        "B is a knot in A's world (in-degree 2) and in L's (in-degree 3)"
    );
    assert_eq!(summary.no_prior_input_count, 1);
    assert!((summary.mean_recall - 0.6).abs() < 1e-9, "{summary:?}");
    assert!(
        (summary.declared_link_recall + summary.tag_frame_recall - summary.mean_recall).abs()
            < 1e-9,
        "the declared/tag split must add up to mean recall"
    );
    assert!((summary.any_hit_rate - 0.6).abs() < 1e-9);

    // --limit keeps the most recent N evaluated records.
    let limited = recall_eval::evaluate(&runtime, Some(1)).unwrap();
    assert_eq!(limited.summary.evaluated, 1);
    assert_eq!(limited.records[0].state_record_id, late_l);
}

#[test]
fn a_store_with_no_declared_links_reports_an_empty_evaluation() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Runtime::init(
        dir.path(),
        RuntimeOptions::deterministic_ids(82),
        ResourceImplementation::FakeA,
    )
    .unwrap();
    let utterance = append_utterance(&runtime, QUERY);
    activate(
        &runtime,
        MutationDomain::Episodic,
        "alice",
        json!({"text": "no links here", "marker": "plain"}),
        vec![utterance],
    );

    let report = recall_eval::evaluate(&runtime, None).unwrap();
    assert_eq!(report.summary.evaluated, 0);
    assert_eq!(report.summary.mean_recall, 0.0);
    assert!(report.records.is_empty());
}
