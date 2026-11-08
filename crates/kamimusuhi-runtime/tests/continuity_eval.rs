//! Paired-capture fixtures: evaluation mechanics, never real model scores.
use std::collections::BTreeSet;

use kamimusuhi_core::digest::content_digest;
use kamimusuhi_runtime::continuity_eval::*;

fn fixture() -> ComparisonInput {
    let hash = content_digest(b"frozen-context");
    let case = ContinuityCase {
        case_id: "corrected-promise".into(),
        input: "前の約束を確認して".into(),
        context_digest: hash.clone(),
        required_text: vec!["訂正済み".into()],
        forbidden_text: vec!["旧情報が正しい".into()],
        required_evidence_ids: vec!["evidence:fixture-source".into()],
        commitment_text: vec!["明日までに確認".into()],
        human_axes: BTreeSet::from([HumanAxis::VoiceContinuity, HumanAxis::RelationalContinuity]),
    };
    let output = RecordedOutput {
        case_id: case.case_id.clone(),
        input: case.input.clone(),
        context_digest: hash.clone(),
        text: "訂正済み。明日までに確認します。".into(),
        cited_evidence_ids: case.required_evidence_ids.clone(),
        human_reviews: vec![],
    };
    let conditions = RunConditions {
        model: "fixture-before".into(),
        prompt_digest: hash.clone(),
        sampling_digest: hash.clone(),
        tools_digest: hash.clone(),
        memory_digest: hash,
    };
    let baseline = RecordedRun {
        run_id: "before".into(),
        capture_kind: CaptureKind::Fixture,
        condition_digest: conditions.digest(),
        conditions,
        outputs: vec![output],
    };
    let mut candidate = baseline.clone();
    candidate.run_id = "after".into();
    candidate.conditions.model = "fixture-after".into();
    candidate.condition_digest = candidate.conditions.digest();
    ComparisonInput {
        format_version: FORMAT_VERSION,
        suite_id: "fixture-only".into(),
        cases: vec![case],
        allowed_condition_changes: BTreeSet::from([ConditionComponent::Model]),
        baseline,
        candidate,
    }
}

fn reviews(run: &mut RecordedRun) {
    for output in &mut run.outputs {
        for axis in [HumanAxis::VoiceContinuity, HumanAxis::RelationalContinuity] {
            for id in ["human-a", "human-b"] {
                output.human_reviews.push(HumanReview {
                    reviewer_id: id.into(),
                    axis,
                    judgment: Judgment::Pass,
                    rationale: "fixture annotation only".into(),
                });
            }
        }
    }
}

#[test]
fn absent_or_one_human_review_stays_unverified_despite_exact_text_agreement() {
    let mut input = fixture();
    let report = compare(&input).unwrap();
    assert_eq!(report.candidate_status, EvaluationStatus::Unverified);
    assert!(!report.cases[0].changed_output);
    reviews(&mut input.candidate);
    input.candidate.outputs[0]
        .human_reviews
        .retain(|r| r.reviewer_id == "human-a");
    assert_eq!(
        compare(&input).unwrap().candidate_status,
        EvaluationStatus::Unverified
    );
}

#[test]
fn omitted_human_axes_cannot_turn_mechanical_checks_into_behavioral_pass() {
    let mut input = fixture();
    input.cases[0].human_axes.clear();
    assert_eq!(
        compare(&input).unwrap().candidate_status,
        EvaluationStatus::Unverified
    );
}

#[test]
fn two_reviewers_and_mechanical_contracts_pass_with_conditions_and_limitations_preserved() {
    let mut input = fixture();
    reviews(&mut input.baseline);
    reviews(&mut input.candidate);
    let report = compare(&input).unwrap();
    assert_eq!(report.candidate_status, EvaluationStatus::Pass);
    assert_eq!(report.changed_conditions, vec![ConditionComponent::Model]);
    assert_eq!(report.candidate_conditions.model, "fixture-after");
    assert!(report.limits.iter().any(|s| s.contains("not real-model")));
    assert!(report.cases[0].regressions.is_empty());
    assert_eq!(report.dataset_digest.len(), 71);
}

#[test]
fn correction_and_promise_regressions_fail_even_with_positive_human_reviews() {
    let mut input = fixture();
    reviews(&mut input.baseline);
    reviews(&mut input.candidate);
    input.candidate.outputs[0].text = "旧情報が正しい".into();
    input.candidate.outputs[0].cited_evidence_ids.clear();
    let report = compare(&input).unwrap();
    assert_eq!(report.candidate_status, EvaluationStatus::Fail);
    assert_eq!(report.cases[0].regressions.len(), 4);
    assert!(report.cases[0].changed_output);
}

#[test]
fn a_human_detected_relational_regression_fails_despite_identical_text() {
    let mut input = fixture();
    reviews(&mut input.baseline);
    reviews(&mut input.candidate);
    input.candidate.outputs[0].human_reviews[2].judgment = Judgment::Fail;
    let report = compare(&input).unwrap();
    assert_eq!(report.candidate_status, EvaluationStatus::Fail);
    assert!(!report.cases[0].changed_output);
    assert!(
        report.cases[0]
            .regressions
            .iter()
            .any(|r| r.contains("RelationalContinuity"))
    );
}

#[test]
fn changed_context_or_input_cannot_be_scored_as_a_model_effect() {
    let mut input = fixture();
    input.candidate.outputs[0].input.push('!');
    assert_eq!(
        compare(&input).unwrap_err().code,
        "INPUT_OR_CONTEXT_MISMATCH"
    );
    input = fixture();
    input.candidate.outputs[0].context_digest = content_digest(b"different memory");
    assert_eq!(
        compare(&input).unwrap_err().code,
        "INPUT_OR_CONTEXT_MISMATCH"
    );
}

#[test]
fn manifest_mismatch_and_unapproved_changes_are_refused() {
    let mut input = fixture();
    input.candidate.conditions.prompt_digest = content_digest(b"new prompt");
    assert_eq!(
        compare(&input).unwrap_err().code,
        "CONDITION_DIGEST_MISMATCH"
    );
    input.candidate.condition_digest = input.candidate.conditions.digest();
    assert_eq!(
        compare(&input).unwrap_err().code,
        "UNAPPROVED_CONDITION_CHANGE"
    );
    input
        .allowed_condition_changes
        .insert(ConditionComponent::Prompt);
    assert!(compare(&input).is_ok());
}

#[test]
fn missing_extra_duplicate_and_empty_outputs_are_not_passes() {
    let mut input = fixture();
    input.candidate.outputs.clear();
    assert_eq!(
        compare(&input).unwrap_err().code,
        "MISSING_OR_EXTRA_OUTPUTS"
    );
    input = fixture();
    input.candidate.outputs[0].text = "  ".into();
    assert_eq!(compare(&input).unwrap_err().code, "EMPTY_OR_EXCESS_OUTPUT");
    input = fixture();
    input.candidate.outputs[0].case_id = "unknown".into();
    assert_eq!(compare(&input).unwrap_err().code, "UNKNOWN_OUTPUT_CASE");
    input = fixture();
    let mut second = input.cases[0].clone();
    second.case_id = "another-case".into();
    input.cases.push(second);
    input
        .candidate
        .outputs
        .push(input.candidate.outputs[0].clone());
    input
        .baseline
        .outputs
        .push(input.baseline.outputs[0].clone());
    assert_eq!(compare(&input).unwrap_err().code, "DUPLICATE_OUTPUT_CASE");
}

#[test]
fn duplicate_reviewers_and_vacuous_contracts_are_refused() {
    let mut input = fixture();
    reviews(&mut input.candidate);
    input.candidate.outputs[0].human_reviews[1].reviewer_id = " human-a ".into();
    assert_eq!(
        compare(&input).unwrap_err().code,
        "INVALID_OR_DUPLICATE_HUMAN_REVIEW"
    );
    input = fixture();
    let case = &mut input.cases[0];
    case.required_text.clear();
    case.forbidden_text.clear();
    case.required_evidence_ids.clear();
    case.commitment_text.clear();
    case.human_axes.clear();
    assert_eq!(compare(&input).unwrap_err().code, "EMPTY_CASE_CONTRACT");
}

#[test]
fn cli_emits_unverified_report_and_refuses_invalid_input_without_an_api() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("paired.json");
    let report = dir.path().join("report.json");
    std::fs::write(&source, serde_json::to_vec(&fixture()).unwrap()).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_continuity-eval"))
        .args(["--input"])
        .arg(&source)
        .args(["--out"])
        .arg(&report)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report).unwrap()).unwrap();
    assert_eq!(value["candidate_status"], "unverified");
    assert!(value["cases"][0].get("text").is_none());
    let before = std::fs::read(&source).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_continuity-eval"))
        .args(["--input"])
        .arg(&source)
        .args(["--out"])
        .arg(&source)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(std::fs::read(source).unwrap(), before);
}

#[test]
fn example_capture_is_explicitly_a_fixture_and_stays_unverified() {
    let input: ComparisonInput =
        serde_json::from_str(include_str!("fixtures/continuity-paired.json")).unwrap();
    let report = compare(&input).unwrap();
    assert_eq!(report.capture_kind, CaptureKind::Fixture);
    assert_eq!(report.candidate_status, EvaluationStatus::Unverified);
    let mut mixed = input;
    mixed.candidate.capture_kind = CaptureKind::ModelOutput;
    assert_eq!(compare(&mixed).unwrap_err().code, "MIXED_CAPTURE_KINDS");
}

#[test]
fn candidate_reviews_do_not_fill_in_missing_baseline_reviews() {
    let mut input = fixture();
    reviews(&mut input.candidate);
    let report = compare(&input).unwrap();
    assert_eq!(report.candidate_status, EvaluationStatus::Pass);
    assert_eq!(report.comparison_status, EvaluationStatus::Unverified);
}

#[test]
#[cfg(unix)]
fn cli_does_not_overwrite_a_hardlinked_input() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("input.json");
    let alias = dir.path().join("alias.json");
    let original = serde_json::to_vec(&fixture()).unwrap();
    std::fs::write(&input, &original).unwrap();
    std::fs::hard_link(&input, &alias).unwrap();
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_continuity-eval"))
        .arg("--input")
        .arg(&input)
        .arg("--out")
        .arg(alias)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(2));
    assert_eq!(std::fs::read(input).unwrap(), original);
}
