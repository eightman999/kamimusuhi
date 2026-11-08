//! Paired, offline continuity evaluation of recorded outputs.
//!
//! Exact textual contracts and declared reference coverage are mechanical
//! checks, not semantic entailment or proof of personality. Relational and
//! voice continuity require independent human judgments; absent judgments
//! remain unverified. No provider or canonical store is opened here.

use std::collections::{BTreeMap, BTreeSet};

use kamimusuhi_core::digest::{content_digest, json_digest};
use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION: u32 = 1;
const MAX_CASES: usize = 512;
const MAX_TEXT_BYTES: usize = 128 * 1024;
const MIN_REVIEWERS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionComponent {
    Model,
    Prompt,
    Sampling,
    Tools,
    Memory,
}

/// Secret-free capture declarations. These attest the recorded conditions,
/// not remote model weight identity or the truth of an operator's capture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunConditions {
    pub model: String,
    pub prompt_digest: String,
    pub sampling_digest: String,
    pub tools_digest: String,
    pub memory_digest: String,
}

impl RunConditions {
    pub fn digest(&self) -> String {
        json_digest(&serde_json::to_value(self).expect("conditions serialize"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HumanAxis {
    VoiceContinuity,
    RelationalContinuity,
    TaskContinuity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Judgment {
    Pass,
    Fail,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanReview {
    /// Distinct identities must be assigned by the operator; strings alone
    /// cannot establish that two reviewers are independent human beings.
    pub reviewer_id: String,
    pub axis: HumanAxis,
    pub judgment: Judgment,
    pub rationale: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuityCase {
    pub case_id: String,
    pub input: String,
    /// Digest of the exact context delivered for this case, excluding the
    /// model/prompt changes explicitly authorized for the comparison.
    pub context_digest: String,
    #[serde(default)]
    pub required_text: Vec<String>,
    #[serde(default)]
    pub forbidden_text: Vec<String>,
    #[serde(default)]
    pub required_evidence_ids: Vec<String>,
    /// Concrete statements of outstanding commitments that must occur in
    /// the answer. This measures mention, not real-world task completion.
    #[serde(default)]
    pub commitment_text: Vec<String>,
    #[serde(default)]
    pub human_axes: BTreeSet<HumanAxis>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedOutput {
    pub case_id: String,
    pub input: String,
    pub context_digest: String,
    pub text: String,
    #[serde(default)]
    pub cited_evidence_ids: Vec<String>,
    #[serde(default)]
    pub human_reviews: Vec<HumanReview>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureKind {
    Fixture,
    ModelOutput,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedRun {
    pub run_id: String,
    pub capture_kind: CaptureKind,
    pub conditions: RunConditions,
    pub condition_digest: String,
    pub outputs: Vec<RecordedOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComparisonInput {
    pub format_version: u32,
    pub suite_id: String,
    pub cases: Vec<ContinuityCase>,
    /// Only named components may differ. Memory/tool changes must be
    /// declared rather than being mistaken for effects of a new model.
    #[serde(default)]
    pub allowed_condition_changes: BTreeSet<ConditionComponent>,
    pub baseline: RecordedRun,
    pub candidate: RecordedRun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationStatus {
    Pass,
    Fail,
    Unverified,
}

#[derive(Debug, Clone, Serialize)]
pub struct MechanicalChecks {
    pub required_text: bool,
    pub forbidden_text: bool,
    pub declared_evidence_coverage: bool,
    pub commitment_mentions: bool,
}

impl MechanicalChecks {
    fn passed(&self) -> bool {
        self.required_text
            && self.forbidden_text
            && self.declared_evidence_coverage
            && self.commitment_mentions
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HumanAxisResult {
    pub axis: HumanAxis,
    pub reviewers: usize,
    pub status: EvaluationStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct OutputEvaluation {
    pub output_digest: String,
    pub mechanical: MechanicalChecks,
    pub human: Vec<HumanAxisResult>,
    pub status: EvaluationStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaseComparison {
    pub case_id: String,
    pub context_digest: String,
    pub baseline: OutputEvaluation,
    pub candidate: OutputEvaluation,
    pub changed_output: bool,
    /// A regression is reported only when a baseline check passed and its
    /// corresponding candidate check failed. Missing reviews are not passes.
    pub regressions: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComparisonReport {
    pub format_version: u32,
    pub evaluator: &'static str,
    pub suite_id: String,
    pub dataset_digest: String,
    pub baseline_run_id: String,
    pub candidate_run_id: String,
    pub capture_kind: CaptureKind,
    pub baseline_conditions: RunConditions,
    pub candidate_conditions: RunConditions,
    pub changed_conditions: Vec<ConditionComponent>,
    pub cases: Vec<CaseComparison>,
    pub candidate_status: EvaluationStatus,
    pub comparison_status: EvaluationStatus,
    pub limits: Vec<&'static str>,
}

#[derive(Debug, thiserror::Error)]
#[error("continuity evaluation input refused: {code}")]
pub struct EvaluationError {
    pub code: &'static str,
}

type EvalResult<T> = Result<T, EvaluationError>;

fn refuse<T>(code: &'static str) -> EvalResult<T> {
    Err(EvaluationError { code })
}

fn valid_digest(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|c| c.is_ascii_hexdigit()))
}

fn nonempty_bounded(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= MAX_TEXT_BYTES
}

fn validate_terms(terms: &[String]) -> bool {
    terms.len() <= 128
        && terms.iter().all(|term| nonempty_bounded(term))
        && terms.iter().collect::<BTreeSet<_>>().len() == terms.len()
}

fn validate_run<'a>(
    run: &'a RecordedRun,
    cases: &BTreeMap<&str, &ContinuityCase>,
) -> EvalResult<BTreeMap<&'a str, &'a RecordedOutput>> {
    if !nonempty_bounded(&run.run_id) || !nonempty_bounded(&run.conditions.model) {
        return refuse("INVALID_RUN_ID_OR_MODEL");
    }
    if [
        &run.conditions.prompt_digest,
        &run.conditions.sampling_digest,
        &run.conditions.tools_digest,
        &run.conditions.memory_digest,
    ]
    .iter()
    .any(|digest| !valid_digest(digest))
    {
        return refuse("INVALID_CONDITION_COMPONENT_DIGEST");
    }
    if run.condition_digest != run.conditions.digest() {
        return refuse("CONDITION_DIGEST_MISMATCH");
    }
    if run.outputs.len() != cases.len() {
        return refuse("MISSING_OR_EXTRA_OUTPUTS");
    }
    let mut outputs = BTreeMap::new();
    for output in &run.outputs {
        let Some(case) = cases.get(output.case_id.as_str()) else {
            return refuse("UNKNOWN_OUTPUT_CASE");
        };
        if outputs.insert(output.case_id.as_str(), output).is_some() {
            return refuse("DUPLICATE_OUTPUT_CASE");
        }
        if output.input != case.input || output.context_digest != case.context_digest {
            return refuse("INPUT_OR_CONTEXT_MISMATCH");
        }
        if !nonempty_bounded(&output.text) {
            return refuse("EMPTY_OR_EXCESS_OUTPUT");
        }
        if !validate_terms(&output.cited_evidence_ids) || output.human_reviews.len() > 128 {
            return refuse("INVALID_OUTPUT_METADATA");
        }
        let mut reviews = BTreeSet::new();
        for review in &output.human_reviews {
            if !nonempty_bounded(&review.reviewer_id)
                || !nonempty_bounded(&review.rationale)
                || !case.human_axes.contains(&review.axis)
                || !reviews.insert((review.axis, review.reviewer_id.trim()))
            {
                return refuse("INVALID_OR_DUPLICATE_HUMAN_REVIEW");
            }
        }
    }
    Ok(outputs)
}

fn score(case: &ContinuityCase, output: &RecordedOutput) -> OutputEvaluation {
    let mechanical = MechanicalChecks {
        required_text: case.required_text.iter().all(|t| output.text.contains(t)),
        forbidden_text: case.forbidden_text.iter().all(|t| !output.text.contains(t)),
        declared_evidence_coverage: case
            .required_evidence_ids
            .iter()
            .all(|id| output.cited_evidence_ids.contains(id)),
        commitment_mentions: case.commitment_text.iter().all(|t| output.text.contains(t)),
    };
    let human: Vec<_> = case
        .human_axes
        .iter()
        .map(|axis| {
            let reviews: Vec<_> = output
                .human_reviews
                .iter()
                .filter(|r| r.axis == *axis)
                .collect();
            let status = if reviews.iter().any(|r| r.judgment == Judgment::Fail) {
                EvaluationStatus::Fail
            } else if reviews.len() < MIN_REVIEWERS {
                EvaluationStatus::Unverified
            } else {
                EvaluationStatus::Pass
            };
            HumanAxisResult {
                axis: *axis,
                reviewers: reviews.len(),
                status,
            }
        })
        .collect();
    let status = if !mechanical.passed() || human.iter().any(|r| r.status == EvaluationStatus::Fail)
    {
        EvaluationStatus::Fail
    } else if human.is_empty()
        || human
            .iter()
            .any(|r| r.status == EvaluationStatus::Unverified)
    {
        EvaluationStatus::Unverified
    } else {
        EvaluationStatus::Pass
    };
    OutputEvaluation {
        output_digest: content_digest(output.text.as_bytes()),
        mechanical,
        human,
        status,
    }
}

/// Evaluate recorded outputs only. Unknown/missing/duplicate cases and
/// unapproved or misdeclared conditions fail before any scores are emitted.
pub fn compare(input: &ComparisonInput) -> EvalResult<ComparisonReport> {
    if input.format_version != FORMAT_VERSION {
        return refuse("UNSUPPORTED_FORMAT_VERSION");
    }
    if !nonempty_bounded(&input.suite_id) || input.cases.is_empty() || input.cases.len() > MAX_CASES
    {
        return refuse("INVALID_SUITE_OR_CASE_COUNT");
    }
    let mut cases = BTreeMap::new();
    for case in &input.cases {
        if !nonempty_bounded(&case.case_id)
            || !nonempty_bounded(&case.input)
            || !valid_digest(&case.context_digest)
            || !validate_terms(&case.required_text)
            || !validate_terms(&case.forbidden_text)
            || !validate_terms(&case.required_evidence_ids)
            || !validate_terms(&case.commitment_text)
        {
            return refuse("INVALID_CASE_CONTRACT");
        }
        if case.required_text.is_empty()
            && case.forbidden_text.is_empty()
            && case.required_evidence_ids.is_empty()
            && case.commitment_text.is_empty()
            && case.human_axes.is_empty()
        {
            return refuse("EMPTY_CASE_CONTRACT");
        }
        if case
            .required_text
            .iter()
            .chain(&case.commitment_text)
            .any(|required| {
                case.forbidden_text
                    .iter()
                    .any(|forbidden| required.contains(forbidden))
            })
        {
            return refuse("CONTRADICTORY_CASE_CONTRACT");
        }
        if cases.insert(case.case_id.as_str(), case).is_some() {
            return refuse("DUPLICATE_CASE");
        }
    }
    if input.baseline.run_id == input.candidate.run_id {
        return refuse("IDENTICAL_RUN_IDS");
    }
    if input.baseline.capture_kind != input.candidate.capture_kind {
        return refuse("MIXED_CAPTURE_KINDS");
    }
    let baseline = validate_run(&input.baseline, &cases)?;
    let candidate = validate_run(&input.candidate, &cases)?;
    let b = &input.baseline.conditions;
    let c = &input.candidate.conditions;
    let changed_conditions: Vec<_> = [
        (ConditionComponent::Model, b.model != c.model),
        (
            ConditionComponent::Prompt,
            b.prompt_digest != c.prompt_digest,
        ),
        (
            ConditionComponent::Sampling,
            b.sampling_digest != c.sampling_digest,
        ),
        (ConditionComponent::Tools, b.tools_digest != c.tools_digest),
        (
            ConditionComponent::Memory,
            b.memory_digest != c.memory_digest,
        ),
    ]
    .into_iter()
    .filter_map(|(component, changed)| changed.then_some(component))
    .collect();
    if changed_conditions
        .iter()
        .any(|component| !input.allowed_condition_changes.contains(component))
    {
        return refuse("UNAPPROVED_CONDITION_CHANGE");
    }
    let mut rows = Vec::new();
    for case in &input.cases {
        let baseline = score(case, baseline[case.case_id.as_str()]);
        let candidate = score(case, candidate[case.case_id.as_str()]);
        let mut regressions = Vec::new();
        for (name, before, after) in [
            (
                "required_text",
                baseline.mechanical.required_text,
                candidate.mechanical.required_text,
            ),
            (
                "forbidden_text",
                baseline.mechanical.forbidden_text,
                candidate.mechanical.forbidden_text,
            ),
            (
                "declared_evidence_coverage",
                baseline.mechanical.declared_evidence_coverage,
                candidate.mechanical.declared_evidence_coverage,
            ),
            (
                "commitment_mentions",
                baseline.mechanical.commitment_mentions,
                candidate.mechanical.commitment_mentions,
            ),
        ] {
            if before && !after {
                regressions.push(name.to_owned());
            }
        }
        for (before, after) in baseline.human.iter().zip(&candidate.human) {
            if before.status == EvaluationStatus::Pass && after.status != EvaluationStatus::Pass {
                regressions.push(format!("human:{:?}", before.axis));
            }
        }
        rows.push(CaseComparison {
            case_id: case.case_id.clone(),
            context_digest: case.context_digest.clone(),
            changed_output: baseline.output_digest != candidate.output_digest,
            baseline,
            candidate,
            regressions,
        });
    }
    let candidate_status = if rows
        .iter()
        .any(|r| r.candidate.status == EvaluationStatus::Fail)
    {
        EvaluationStatus::Fail
    } else if rows
        .iter()
        .any(|r| r.candidate.status == EvaluationStatus::Unverified)
    {
        EvaluationStatus::Unverified
    } else {
        EvaluationStatus::Pass
    };
    let comparison_status = if candidate_status == EvaluationStatus::Fail {
        EvaluationStatus::Fail
    } else if rows
        .iter()
        .any(|r| r.baseline.status == EvaluationStatus::Unverified)
        || candidate_status == EvaluationStatus::Unverified
    {
        EvaluationStatus::Unverified
    } else {
        EvaluationStatus::Pass
    };
    Ok(ComparisonReport {
        format_version: FORMAT_VERSION,
        evaluator: "paired-continuity-v1",
        suite_id: input.suite_id.clone(),
        dataset_digest: json_digest(
            &serde_json::to_value(input).expect("evaluation input serializes"),
        ),
        capture_kind: input.baseline.capture_kind,
        baseline_run_id: input.baseline.run_id.clone(),
        candidate_run_id: input.candidate.run_id.clone(),
        baseline_conditions: b.clone(),
        candidate_conditions: c.clone(),
        changed_conditions,
        cases: rows,
        candidate_status,
        comparison_status,
        limits: vec![
            "exact fragment checks measure contract coverage, not natural-language entailment; negated mentions can match",
            "declared evidence IDs are capture metadata, not proof that a claim follows from the cited source",
            "commitment mentions do not establish real-world execution or completion",
            "voice and relational continuity require at least two distinct human reviewer IDs per requested axis; identity and independence need operator verification",
            "recorded condition/context digests must be captured honestly by the caller; this evaluator does not attest remote model weights or capture provenance",
            "fixture results demonstrate evaluation mechanics, not real-model continuity or personality preservation",
        ],
    })
}
