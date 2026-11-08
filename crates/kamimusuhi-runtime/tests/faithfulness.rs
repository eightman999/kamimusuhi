//! FI2 integration: untrusted projections cannot bypass the central draft
//! boundary; accepted state retains its original lineage and corrections.

use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceLookup, EvidenceRelation, EvidenceSource, EvidenceStore, NewEvidence,
    NewEvidenceLink, RetentionClass,
};
use kamimusuhi_core::ids::{EvidenceId, TurnId};
use kamimusuhi_core::memory::{LifecycleState, MemoryQuery, MemoryRepository};
use kamimusuhi_core::mutation::{MutationDomain, MutationOperation, OriginClass};
use kamimusuhi_core::persona::ProposalDraft;
use kamimusuhi_runtime::c0;
use kamimusuhi_runtime::faithfulness::{self, FaithfulnessVerdict};
use kamimusuhi_runtime::{ResourceImplementation, Runtime, RuntimeOptions};
use serde_json::{Value, json};

fn init() -> (tempfile::TempDir, Runtime) {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Runtime::init(
        dir.path(),
        RuntimeOptions::deterministic(0xF120),
        ResourceImplementation::FakeA,
    )
    .unwrap();
    (dir, runtime)
}

fn source(
    runtime: &Runtime,
    kind: EvidenceKind,
    payload: Value,
    parents: &[EvidenceId],
) -> EvidenceId {
    let id = EvidenceId::generate(runtime.ids().as_ref());
    runtime
        .store()
        .append(NewEvidence {
            evidence_id: id,
            individual_id: runtime.individual_id(),
            session_id: None,
            turn_id: None,
            kind,
            origin_class: if matches!(kind, EvidenceKind::Summary | EvidenceKind::Reflection) {
                OriginClass::Inferred
            } else {
                OriginClass::Reported
            },
            payload,
            source: EvidenceSource {
                source_id: Some("text-chat:alice".to_owned()),
                ..EvidenceSource::default()
            },
            retention_class: RetentionClass::Standard,
        })
        .unwrap();
    for parent in parents {
        runtime
            .store()
            .link(NewEvidenceLink {
                from_evidence_id: id,
                to_evidence_id: *parent,
                relation: EvidenceRelation::DerivedFrom,
            })
            .unwrap();
    }
    id
}

fn draft(candidate: Value, refs: Vec<EvidenceId>) -> ProposalDraft {
    ProposalDraft {
        domain: MutationDomain::Episodic,
        operation: MutationOperation::Capture,
        subject_key: Some("alice".to_owned()),
        candidate,
        evidence_refs: refs,
        supersedes: None,
        origin_class: OriginClass::Reported,
    }
}

fn assess(
    runtime: &Runtime,
    candidate: Value,
    refs: Vec<EvidenceId>,
) -> faithfulness::FaithfulnessReport {
    faithfulness::assess_draft(runtime, &draft(candidate, refs))
        .unwrap()
        .unwrap()
}

fn claim() -> Value {
    json!({"subject":"alice", "predicate":"likes", "object":"tea", "polarity":false,
        "certainty":"uncertain", "scope":"this week", "sequence":7})
}

#[test]
fn exact_whole_sources_preserve_negation_scope_and_uncertainty() {
    let (_dir, runtime) = init();
    let text = "今週は、おそらく紅茶を好きではありません。これは質問ではなく自分の申告です。";
    let id = source(
        &runtime,
        EvidenceKind::UserUtterance,
        json!({"text":text}),
        &[],
    );
    let good = json!({"summary":text,"source_quotes":[{"evidence_id":id,"quote":text}]});
    assert!(assess(&runtime, good, vec![id]).is_faithful());
    for altered in [
        "紅茶を好き",
        "紅茶を好きではありません",
        "紅茶は絶対に嫌い",
        "毎週、紅茶を好きではありません",
    ] {
        let report = assess(
            &runtime,
            json!({"summary":altered,"source_quotes":[{"evidence_id":id,"quote":text}]}),
            vec![id],
        );
        assert_eq!(report.verdict, FaithfulnessVerdict::Unverifiable);
    }
    let partial = assess(
        &runtime,
        json!({"summary":"紅茶を好き","source_quotes":[{"evidence_id":id,"quote":"紅茶を好き"}]}),
        vec![id],
    );
    assert_eq!(partial.reason_code, "FI2_QUOTE_DRIFT");
    assert_eq!(partial.verdict, FaithfulnessVerdict::Mutated);
}

#[test]
fn structured_claim_changes_are_detected_without_a_language_judge() {
    let (_dir, runtime) = init();
    let original = claim();
    let id = source(
        &runtime,
        EvidenceKind::UserUtterance,
        json!({"text":"structured testimony", "claim":original}),
        &[],
    );
    let good = json!({"claims":[{"evidence_id":id,"claim":original}]});
    assert!(assess(&runtime, good, vec![id]).is_faithful());
    for (field, value, expected) in [
        ("polarity", json!(true), "FI2_POLARITY_FLIP"),
        ("object", json!("coffee"), "FI2_OBJECT_DRIFT"),
        ("scope", json!("always"), "FI2_SCOPE_DRIFT"),
        ("sequence", json!(8), "FI2_SEQUENCE_DRIFT"),
        ("certainty", json!("certain"), "FI2_UNCERTAINTY_NORMALIZED"),
        ("subject", json!("bob"), "FI2_FABRICATED_CLAIM"),
    ] {
        let mut changed = original.clone();
        changed[field] = value;
        let report = assess(
            &runtime,
            json!({"claims":[{"evidence_id":id,"claim":changed}]}),
            vec![id],
        );
        assert_eq!(report.verdict, FaithfulnessVerdict::Mutated);
        assert_eq!(report.reason_code, expected);
    }
}

#[test]
fn one_unknown_reference_refuses_the_whole_item_before_writer_claim() {
    let (_dir, runtime) = init();
    let id = source(
        &runtime,
        EvidenceKind::UserUtterance,
        json!({"text":"exact"}),
        &[],
    );
    let unknown = EvidenceId::from_u128(0xDEAD);
    let before = runtime.head().unwrap();
    let mut writer = None;
    let outcomes = c0::submit_drafts(
        &runtime,
        None,
        None,
        &mut writer,
        vec![draft(json!({"summary":"exact"}), vec![id, unknown])],
    )
    .unwrap();
    assert!(!outcomes[0].activated);
    assert_eq!(outcomes[0].reason_code, "FI2_UNKNOWN_SOURCE");
    assert!(writer.is_none());
    let after = runtime.head().unwrap();
    assert_eq!(before.expected(), after.expected());
    assert_eq!(before.writer_epoch, after.writer_epoch);
    assert!(
        MemoryRepository::retrieve(
            runtime.store(),
            &MemoryQuery::current(runtime.individual_id())
        )
        .unwrap()
        .is_empty()
    );
}

#[test]
fn an_existing_but_unpresented_id_is_not_repaired_into_a_valid_draft() {
    let (_dir, runtime) = init();
    let id = source(
        &runtime,
        EvidenceKind::UserUtterance,
        json!({"text":"exact"}),
        &[],
    );
    let mut drafts = vec![draft(json!({"summary":"exact"}), vec![id])];
    faithfulness::reject_unpresented_references(&mut drafts, &Default::default());
    let outcomes = c0::submit_drafts(&runtime, None, None, &mut None, drafts).unwrap();
    assert!(!outcomes[0].activated);
    assert_eq!(outcomes[0].reason_code, "FI2_UNPRESENTED_REFERENCE");
}

#[test]
fn extra_semantic_fields_and_unbound_claims_cannot_hide_beside_a_good_quote() {
    let (_dir, runtime) = init();
    let id = source(
        &runtime,
        EvidenceKind::UserUtterance,
        json!({"text":"exact"}),
        &[],
    );
    assert_eq!(
        assess(
            &runtime,
            json!({"summary":"exact","fact":"fabricated"}),
            vec![id]
        )
        .reason_code,
        "FI2_UNSUPPORTED_FIELD"
    );
    assert_eq!(
        assess(
            &runtime,
            json!({"claims":[{"evidence_id":id,"claim":claim()}]}),
            vec![id]
        )
        .reason_code,
        "FI2_NO_STRUCTURED_SOURCE"
    );
    assert_eq!(
        assess(
            &runtime,
            json!({"summary":"exact","source_quotes":[]}),
            vec![id]
        )
        .reason_code,
        "FI2_INCOMPLETE_QUOTES"
    );
}

#[test]
fn derived_sources_are_revalidated_instead_of_trusting_their_own_verdict() {
    let (_dir, runtime) = init();
    let id = source(
        &runtime,
        EvidenceKind::UserUtterance,
        json!({"text":"I do not like tea"}),
        &[],
    );
    let bad = source(
        &runtime,
        EvidenceKind::Summary,
        json!({"summary":"I like tea","faithful":true}),
        &[id],
    );
    let report = assess(&runtime, json!({"summary":"I like tea"}), vec![bad]);
    assert_eq!(report.reason_code, "FI2_UNVERIFIED_DERIVED_SOURCE");
    let good = source(
        &runtime,
        EvidenceKind::Summary,
        json!({"summary":"I do not like tea"}),
        &[id],
    );
    assert!(assess(&runtime, json!({"summary":"I do not like tea"}), vec![good]).is_faithful());
}

#[test]
fn faithful_multihop_projection_remains_in_lineage_and_correction_invalidates_it_after_restart() {
    let (dir, runtime) = init();
    let raw = source(
        &runtime,
        EvidenceKind::UserUtterance,
        json!({"text":"I do not like tea"}),
        &[],
    );
    let s1 = source(
        &runtime,
        EvidenceKind::Summary,
        json!({"summary":"I do not like tea"}),
        &[raw],
    );
    let s2 = source(
        &runtime,
        EvidenceKind::Summary,
        json!({"summary":"I do not like tea"}),
        &[s1],
    );
    let mut fact = draft(json!({"preference":"not tea"}), vec![raw]);
    fact.domain = MutationDomain::Relationship;
    fact.operation = MutationOperation::Fact;
    let submit = |candidate| {
        c0::submit_drafts(
            &runtime,
            None,
            Some(TurnId::generate(runtime.ids().as_ref())),
            &mut None,
            vec![candidate],
        )
        .unwrap()
    };
    assert!(submit(fact)[0].activated);
    let target = MemoryRepository::retrieve(
        runtime.store(),
        &MemoryQuery::current(runtime.individual_id()).in_domain(MutationDomain::Relationship),
    )
    .unwrap()[0]
        .record
        .state_record_id;
    assert!(
        submit(draft(
            json!({"summary":"I do not like tea\nI do not like tea"}),
            vec![s2, raw]
        ))[0]
            .activated
    );
    assert_eq!(
        runtime
            .store()
            .facts(&[s1, s2])
            .unwrap()
            .independent_support_count(),
        1
    );
    let stored = MemoryRepository::retrieve(
        runtime.store(),
        &MemoryQuery::current(runtime.individual_id()).in_domain(MutationDomain::Episodic),
    )
    .unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(
        stored[0]
            .record
            .evidence_refs
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
        [s2, raw].into_iter().collect()
    );
    let episode = stored[0].record.state_record_id;
    let fresh = source(
        &runtime,
        EvidenceKind::UserUtterance,
        json!({"text":"Correction: I now like tea"}),
        &[],
    );
    let correction = source(
        &runtime,
        EvidenceKind::Correction,
        json!({"text":"Correction: I now like tea"}),
        &[],
    );
    runtime
        .store()
        .link(NewEvidenceLink {
            from_evidence_id: correction,
            to_evidence_id: raw,
            relation: EvidenceRelation::Corrects,
        })
        .unwrap();
    let mut replacement = draft(json!({"preference":"tea"}), vec![correction, fresh]);
    replacement.domain = MutationDomain::Relationship;
    replacement.operation = MutationOperation::Correction;
    replacement.supersedes = Some(target);
    assert!(submit(replacement)[0].activated);
    assert_eq!(
        runtime
            .store()
            .get_state_record(episode)
            .unwrap()
            .unwrap()
            .lifecycle_state,
        LifecycleState::Invalidated
    );
    assert_eq!(
        assess(&runtime, json!({"summary":"I do not like tea"}), vec![s2]).reason_code,
        "FI2_CORRECTED_SOURCE"
    );
    drop(runtime);
    let runtime = Runtime::open(dir.path(), RuntimeOptions::deterministic(0xF220)).unwrap();
    assert!(
        runtime
            .store()
            .facts(&[s2])
            .unwrap()
            .get(s2)
            .unwrap()
            .is_corrected
    );
    assert_eq!(
        runtime
            .store()
            .get_state_record(episode)
            .unwrap()
            .unwrap()
            .lifecycle_state,
        LifecycleState::Invalidated
    );
    assert_eq!(
        runtime.store().get(raw).unwrap().unwrap().payload["text"],
        "I do not like tea"
    );
}

#[test]
fn count_projection_cannot_multiply_duplicate_roots() {
    let (_dir, runtime) = init();
    let a = source(
        &runtime,
        EvidenceKind::UserUtterance,
        json!({"text":"one utterance"}),
        &[],
    );
    let b = source(
        &runtime,
        EvidenceKind::UserUtterance,
        json!({"text":"one utterance"}),
        &[],
    );
    runtime
        .store()
        .link(NewEvidenceLink {
            from_evidence_id: b,
            to_evidence_id: a,
            relation: EvidenceRelation::Duplicates,
        })
        .unwrap();
    assert_eq!(
        assess(
            &runtime,
            json!({"summary":"2 utterances from this subject","turns":2}),
            vec![a, b]
        )
        .reason_code,
        "FI2_COUNT_DRIFT"
    );
    assert!(
        assess(
            &runtime,
            json!({"summary":"1 utterances from this subject","turns":1}),
            vec![a, b]
        )
        .is_faithful()
    );
}

#[test]
fn a_full_quote_cannot_be_reassigned_to_a_different_subject() {
    let (_dir, runtime) = init();
    let id = source(
        &runtime,
        EvidenceKind::UserUtterance,
        json!({"text":"I do not like tea"}),
        &[],
    );
    let mut candidate = draft(json!({"summary":"I do not like tea"}), vec![id]);
    candidate.subject_key = Some("bob".to_owned());
    let report = faithfulness::assess_draft(&runtime, &candidate)
        .unwrap()
        .unwrap();
    assert_eq!(report.reason_code, "FI2_SUBJECT_DRIFT");
}

#[test]
fn model_supplied_validity_cannot_change_the_time_scope_of_a_quote() {
    let (_dir, runtime) = init();
    let id = source(
        &runtime,
        EvidenceKind::UserUtterance,
        json!({"text":"I do not like tea"}),
        &[],
    );
    assert_eq!(
        assess(
            &runtime,
            json!({"summary":"I do not like tea","valid_from":123}),
            vec![id]
        )
        .reason_code,
        "FI2_UNSUPPORTED_TIME_SCOPE"
    );
    let bound = source(
        &runtime,
        EvidenceKind::UserUtterance,
        json!({"text":"I do not like tea","valid_from":123}),
        &[],
    );
    assert!(
        assess(
            &runtime,
            json!({"summary":"I do not like tea","valid_from":123}),
            vec![bound]
        )
        .is_faithful()
    );
}

#[test]
fn invalid_source_time_cannot_authorize_an_invalid_projection_time() {
    let (_dir, runtime) = init();
    for metadata in [
        json!({"valid_from":"nonsense"}),
        json!({"valid_from":200,"valid_to":100}),
    ] {
        let mut payload = metadata.clone();
        payload["text"] = json!("source");
        let id = source(&runtime, EvidenceKind::UserUtterance, payload, &[]);
        let mut candidate = metadata;
        candidate["summary"] = json!("source");
        assert_eq!(
            assess(&runtime, candidate, vec![id]).reason_code,
            "FI2_UNSUPPORTED_TIME_SCOPE"
        );
    }
}

#[test]
fn quotations_follow_record_chronology_not_model_reference_order() {
    let (_dir, runtime) = init();
    let a = source(
        &runtime,
        EvidenceKind::UserUtterance,
        json!({"text":"first"}),
        &[],
    );
    let b = source(
        &runtime,
        EvidenceKind::UserUtterance,
        json!({"text":"second"}),
        &[],
    );
    // Deterministic clock ties are resolved by immutable EvidenceId, independent
    // of the model's ordering of references.
    let expected = if a < b {
        "first\nsecond"
    } else {
        "second\nfirst"
    };
    assert!(assess(&runtime, json!({"summary":expected}), vec![b, a]).is_faithful());
}

#[test]
fn nested_projection_cannot_bypass_outer_time_scope_validation() {
    let (_dir, runtime) = init();
    let id = source(
        &runtime,
        EvidenceKind::UserUtterance,
        json!({"text":"source"}),
        &[],
    );
    assert_eq!(
        assess(
            &runtime,
            json!({"projection":{"summary":"source"},"valid_from":123}),
            vec![id]
        )
        .reason_code,
        "FI2_UNSUPPORTED_TIME_SCOPE"
    );
}

#[test]
fn nested_metadata_and_unbound_direct_links_fail_closed() {
    let (_dir, runtime) = init();
    let id = source(
        &runtime,
        EvidenceKind::UserUtterance,
        json!({"text":"source"}),
        &[],
    );
    for metadata in [
        json!({"links": [format!("evidence:{id}")]}),
        json!({"tags":["tag"]}),
        json!({"valid_from":1}),
    ] {
        let mut projection = metadata;
        projection["summary"] = json!("source");
        assert_eq!(
            assess(&runtime, json!({"projection":projection}), vec![id]).reason_code,
            "FI2_NESTED_METADATA"
        );
    }
    for link in [
        format!("evidence:{}", EvidenceId::from_u128(0xdead)),
        "state:0000000000000000000000000000dead".to_owned(),
    ] {
        assert_eq!(
            assess(
                &runtime,
                json!({"summary":"source","links":[link]}),
                vec![id]
            )
            .reason_code,
            "FI2_UNBOUND_LINK"
        );
    }
}
