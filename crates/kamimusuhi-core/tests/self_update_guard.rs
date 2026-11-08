//! Bounds and aggregate-preservation checks for always-carried self state.
//! These are syntactic guards, not a natural-language truth evaluator.

use std::collections::BTreeMap;

use kamimusuhi_core::c0::{
    ImprovementDraft, ImprovementKind, OperativeView, SELF_ENTRY_MAX_BYTES, SELF_MAX_ENTRIES,
    SELF_SNAPSHOT_MAX_BYTES, SelfEntry, SelfField, validate_improvement_draft,
};
use kamimusuhi_core::evidence::EvidenceKind;
use kamimusuhi_core::ids::EvidenceId;
use serde_json::{Value, json};

fn draft(key: Option<&str>, value: Value) -> ImprovementDraft {
    ImprovementDraft {
        kind: ImprovementKind::SelfUpdate,
        target: "values".to_owned(),
        target_key: key.map(str::to_owned),
        proposed_value: value,
        evidence_refs: vec![EvidenceId::from_u128(1)],
        expected_effect: None,
        risk: None,
        confidence: Some(0.8),
    }
}

fn view(key: Option<&str>, value: Value) -> OperativeView {
    let mut view = OperativeView::default();
    view.self_model.apply(
        SelfField::Values,
        SelfEntry {
            key: key.map(str::to_owned),
            value,
            confidence: Some(0.8),
            evidence_refs: vec![EvidenceId::from_u128(1)],
            proposal_id: None,
        },
    );
    view
}

fn validate(draft: &ImprovementDraft, current: &OperativeView) -> Result<(), String> {
    let evidence = BTreeMap::from([(EvidenceId::from_u128(1), EvidenceKind::Reflection)]);
    validate_improvement_draft(draft, &evidence, current)
        .map(|_| ())
        .map_err(|rejection| rejection.reason)
}

#[test]
fn bulk_update_cannot_drop_lesson_object_keys_or_nested_members() {
    let old = view(
        None,
        json!({
            "finish_work": true,
            "verification": {"read_source": true, "check_result": true},
            "lessons": ["keep provenance", "recheck corrections"]
        }),
    );
    for value in [
        json!({"finish_work": true}),
        json!({"finish_work": true, "verification": {"read_source": true}, "lessons": ["keep provenance", "recheck corrections"]}),
        json!({"finish_work": true, "verification": {"read_source": true, "check_result": true}, "lessons": ["keep provenance"]}),
        json!("one replacement sentence"),
    ] {
        assert!(
            validate(&draft(None, value), &old)
                .unwrap_err()
                .contains("aggregate members")
        );
    }
    assert_eq!(
        old.self_model.field(SelfField::Values)[0].value["lessons"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn safe_extension_and_explicit_scalar_correction_remain_allowed() {
    let old = view(
        Some("procedure"),
        json!({"lesson": "check logs", "steps": ["read", "run"]}),
    );
    assert!(
        validate(
            &draft(
                Some("procedure"),
                json!({"lesson": "check source logs", "steps": ["read", "run", "verify"]})
            ),
            &old
        )
        .is_ok()
    );
    assert!(
        validate(
            &draft(Some("separate_lesson"), json!("keep user edits")),
            &old
        )
        .is_ok()
    );
    assert!(
        validate(
            &draft(Some("procedure"), json!("corrected statement")),
            &view(Some("procedure"), json!("old statement"))
        )
        .is_ok()
    );
}

#[test]
fn duplicate_keys_and_noop_changes_do_not_grow_the_core() {
    let old = view(Some("finish"), json!("finish the work"));
    assert!(
        validate(&draft(Some("finish"), json!("finish  the\nwork")), &old)
            .unwrap_err()
            .contains("no content change")
    );
    assert!(
        validate(&draft(Some("finish_again"), json!("finish the work")), &old)
            .unwrap_err()
            .contains("duplicates")
    );
    // This guard does not pretend to infer semantic equivalence.
    assert!(
        validate(
            &draft(Some("different_words"), json!("complete the task")),
            &old
        )
        .is_ok()
    );
}

#[test]
fn unchanged_text_can_replace_corrected_provenance_with_new_presented_evidence() {
    let old = view(Some("work"), json!("verify the result"));
    let mut replacement = draft(Some("work"), json!("verify the result"));
    replacement.evidence_refs = vec![EvidenceId::from_u128(2)];
    let evidence = BTreeMap::from([(EvidenceId::from_u128(2), EvidenceKind::Reflection)]);
    assert!(validate_improvement_draft(&replacement, &evidence, &old).is_ok());
}

#[test]
fn list_member_multiplicity_is_preserved() {
    let old = view(None, json!(["verify", "verify"]));
    assert!(
        validate(&draft(None, json!(["verify", "new"])), &old)
            .unwrap_err()
            .contains("aggregate members")
    );
    assert!(validate(&draft(None, json!(["new", "verify", "verify"])), &old).is_ok());
}

#[test]
fn utf8_serialized_value_and_key_bounds_are_not_character_counts() {
    let empty = OperativeView::default();
    let boundary = "x".repeat(SELF_ENTRY_MAX_BYTES - 2);
    assert!(validate(&draft(Some("boundary"), json!(boundary)), &empty).is_ok());
    assert!(
        validate(
            &draft(
                Some("too_large"),
                json!("界".repeat(SELF_ENTRY_MAX_BYTES / 3 + 1))
            ),
            &empty
        )
        .unwrap_err()
        .contains("2048")
    );
    for key in [" ".to_owned(), " trailing ".to_owned(), "界".repeat(43)] {
        assert!(
            validate(&draft(Some(&key), json!("value")), &empty)
                .unwrap_err()
                .contains("self key")
        );
    }
}

#[test]
fn entry_count_cap_allows_a_replacement_but_refuses_growth() {
    let mut current = OperativeView::default();
    for n in 0..SELF_MAX_ENTRIES {
        current.self_model.apply(
            SelfField::Values,
            SelfEntry {
                key: Some(format!("key-{n}")),
                value: json!(format!("lesson {n}")),
                confidence: None,
                evidence_refs: vec![EvidenceId::from_u128(1)],
                proposal_id: None,
            },
        );
    }
    assert!(
        validate(&draft(Some("one_more"), json!("new lesson")), &current)
            .unwrap_err()
            .contains("64 entries")
    );
    assert!(validate(&draft(Some("key-0"), json!("updated lesson")), &current).is_ok());
    assert_eq!(current.self_model.entry_count(), SELF_MAX_ENTRIES);
}

#[test]
fn total_snapshot_cap_includes_provenance_not_just_text() {
    let mut current = OperativeView::default();
    for n in 0..15 {
        current.self_model.apply(
            SelfField::Values,
            SelfEntry {
                key: Some(format!("key-{n}")),
                value: json!(format!("{n}:{}", "x".repeat(1600))),
                confidence: Some(0.8),
                evidence_refs: vec![EvidenceId::from_u128(1)],
                proposal_id: None,
            },
        );
    }
    assert!(serde_json::to_vec(&current.self_model).unwrap().len() > SELF_SNAPSHOT_MAX_BYTES);
    assert!(
        validate(&draft(Some("new"), json!("new lesson")), &current)
            .unwrap_err()
            .contains("24 KiB")
    );
}

#[test]
fn refs_are_bounded_distinct_and_still_first_party() {
    let empty = OperativeView::default();
    let mut item = draft(Some("lesson"), json!("check source"));
    item.evidence_refs = vec![EvidenceId::from_u128(1); 2];
    assert!(validate(&item, &empty).unwrap_err().contains("distinct"));
    item.evidence_refs = (1..=9).map(EvidenceId::from_u128).collect();
    assert!(validate(&item, &empty).unwrap_err().contains("eight"));
    item.evidence_refs = vec![EvidenceId::from_u128(1)];
    let user = BTreeMap::from([(EvidenceId::from_u128(1), EvidenceKind::UserUtterance)]);
    assert!(
        validate_improvement_draft(&item, &user, &empty)
            .unwrap_err()
            .reason
            .contains("first-party")
    );
}
