//! Outcome history, evaluation battery and history-based routing.
//!
//! Every finished task leaves a [`HistoryRecord`]; the operator can mark a
//! result accepted or rejected ([`Feedback`]). Aggregated per task kind ×
//! executor × model — a model is always measured *through its harness* —
//! they become [`StatRow`]s, which routing may use once enough samples
//! exist. The evaluation battery runs the same cases across harnesses and
//! grades answers with plain, deterministic checks.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use super::types::{AgentBilling, TaskKind};

/// One finished task, as routing and evaluation see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryRecord {
    pub task_id: String,
    /// Unix seconds.
    pub at: u64,
    pub task_kind: TaskKind,
    pub executor: String,
    /// `None` = harness default model.
    #[serde(default)]
    pub model: Option<String>,
    pub billing: AgentBilling,
    pub succeeded: bool,
    pub duration_ms: u64,
    #[serde(default)]
    pub tool_calls: u64,
    #[serde(default)]
    pub tokens: Option<u64>,
    /// Reported, else estimated; `None` = unknown.
    #[serde(default)]
    pub usd: Option<f64>,
    #[serde(default)]
    pub files_touched: usize,
    #[serde(default)]
    pub eval: Option<EvalGrade>,
    /// What history-based routing would have chosen, when it differed or
    /// ran in shadow (`executor:model`).
    #[serde(default)]
    pub shadow_choice: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Accept,
    Reject,
}

impl Verdict {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "accept" => Some(Self::Accept),
            "reject" => Some(Self::Reject),
            _ => None,
        }
    }
}

/// A person's judgement of a result. The latest one per task counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Feedback {
    pub task_id: String,
    pub at: u64,
    pub verdict: Verdict,
    #[serde(default)]
    pub note: Option<String>,
}

/// Aggregate for one task kind × executor × model.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StatRow {
    pub task_kind: TaskKind,
    pub executor: String,
    pub model: Option<String>,
    pub n: usize,
    pub succeeded: usize,
    pub accepted: usize,
    pub rejected: usize,
    pub median_ms: u64,
    /// Mean over tasks with a known cost; `None` if none is known.
    pub mean_usd: Option<f64>,
    /// Smoothed quality in `0..1`, lightly penalised for latency.
    pub score: f64,
}

fn median(mut values: Vec<u64>) -> u64 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    values[values.len() / 2]
}

/// Quality of one task: a person's verdict overrides whether the harness
/// reported success.
fn quality(record: &HistoryRecord, verdict: Option<Verdict>) -> f64 {
    match verdict {
        Some(Verdict::Accept) => 1.0,
        Some(Verdict::Reject) => 0.0,
        None if record.eval.as_ref().is_some_and(|e| !e.passed) => 0.0,
        None => f64::from(u8::from(record.succeeded)),
    }
}

pub fn stats(records: &[HistoryRecord], feedback: &[Feedback]) -> Vec<StatRow> {
    let verdicts: HashMap<&str, Verdict> = feedback
        .iter()
        .map(|f| (f.task_id.as_str(), f.verdict))
        .collect();
    let mut groups: BTreeMap<(String, String, String), Vec<&HistoryRecord>> = BTreeMap::new();
    for r in records {
        groups
            .entry((
                r.task_kind.as_str().to_owned(),
                r.executor.clone(),
                r.model.clone().unwrap_or_default(),
            ))
            .or_default()
            .push(r);
    }
    groups
        .into_values()
        .map(|rows| {
            let first = rows[0];
            let n = rows.len();
            let verdict = |r: &HistoryRecord| verdicts.get(r.task_id.as_str()).copied();
            let quality_sum: f64 = rows.iter().map(|r| quality(r, verdict(r))).sum();
            let median_ms = median(rows.iter().map(|r| r.duration_ms).collect());
            let costs: Vec<f64> = rows.iter().filter_map(|r| r.usd).collect();
            #[allow(clippy::cast_precision_loss)]
            let (score, mean_usd) = {
                let smoothed = (quality_sum + 1.0) / (n as f64 + 2.0);
                let slow = (median_ms as f64 / 600_000.0).min(1.0) * 0.05;
                (
                    smoothed - slow,
                    (!costs.is_empty()).then(|| costs.iter().sum::<f64>() / costs.len() as f64),
                )
            };
            StatRow {
                task_kind: first.task_kind,
                executor: first.executor.clone(),
                model: first.model.clone(),
                n,
                succeeded: rows.iter().filter(|r| r.succeeded).count(),
                accepted: rows
                    .iter()
                    .filter(|r| verdict(r) == Some(Verdict::Accept))
                    .count(),
                rejected: rows
                    .iter()
                    .filter(|r| verdict(r) == Some(Verdict::Reject))
                    .count(),
                median_ms,
                mean_usd,
                score,
            }
        })
        .collect()
}

/// One case of the evaluation battery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalCase {
    pub id: String,
    pub kind: TaskKind,
    pub objective: String,
    #[serde(default)]
    pub success_criteria: Vec<String>,
    /// Every string must appear in the answer (case-insensitive).
    #[serde(default)]
    pub must_contain_all: Vec<String>,
    /// At least one must appear.
    #[serde(default)]
    pub must_contain_any: Vec<String>,
    #[serde(default)]
    pub min_chars: usize,
    /// Needs write permission: runs in an isolated worktree when the
    /// workspace allows writing, is skipped otherwise, and also requires
    /// changed files to pass.
    #[serde(default)]
    pub requires_write: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Battery {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub cases: Vec<EvalCase>,
}

/// Default battery (Q1–Q7 on the Kamimusuhi repository).
pub const DEFAULT_BATTERY: &str =
    include_str!("../../../../deploy/resident/task-battery.example.json");

impl Battery {
    pub fn parse(text: &str) -> Result<Self, String> {
        let battery: Self = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let mut ids = std::collections::HashSet::new();
        for case in &battery.cases {
            if case.id.is_empty()
                || !ids.insert(case.id.as_str())
                || case.objective.trim().is_empty()
            {
                return Err(format!(
                    "battery case {:?} is invalid or duplicated",
                    case.id
                ));
            }
        }
        Ok(battery)
    }

    pub fn default_battery() -> Self {
        Self::parse(DEFAULT_BATTERY).unwrap_or_else(|_| Self {
            name: "empty".to_owned(),
            description: None,
            cases: Vec::new(),
        })
    }
}

/// Result of grading one answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvalGrade {
    pub run_id: String,
    pub case: String,
    pub passed: bool,
    /// (check, passed)
    pub checks: Vec<(String, bool)>,
}

pub fn grade(
    run_id: &str,
    case: &EvalCase,
    succeeded: bool,
    answer: &str,
    files_changed: usize,
) -> EvalGrade {
    let lower = answer.to_lowercase();
    let has = |s: &String| lower.contains(&s.to_lowercase());
    let mut checks = vec![("task succeeded".to_owned(), succeeded)];
    if case.requires_write {
        checks.push(("changed files".to_owned(), files_changed > 0));
    }
    for s in &case.must_contain_all {
        checks.push((format!("contains {s:?}"), has(s)));
    }
    if !case.must_contain_any.is_empty() {
        checks.push((
            format!("contains any of {:?}", case.must_contain_any),
            case.must_contain_any.iter().any(has),
        ));
    }
    if case.min_chars > 0 {
        checks.push((
            format!("at least {} chars", case.min_chars),
            answer.chars().count() >= case.min_chars,
        ));
    }
    EvalGrade {
        run_id: run_id.to_owned(),
        case: case.id.clone(),
        passed: checks.iter().all(|(_, ok)| *ok),
        checks,
    }
}

/// Per-target summary of one evaluation run.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvalSummary {
    pub target: String,
    pub cases: usize,
    pub passed: usize,
    pub failed_cases: Vec<String>,
    pub median_ms: u64,
    pub tokens: Option<u64>,
    pub usd: Option<f64>,
    pub tool_calls: u64,
    pub files_touched: usize,
}

pub fn eval_report(run_id: &str, records: &[HistoryRecord]) -> Vec<EvalSummary> {
    let mut by_target: BTreeMap<String, Vec<&HistoryRecord>> = BTreeMap::new();
    for r in records
        .iter()
        .filter(|r| r.eval.as_ref().is_some_and(|e| e.run_id == run_id))
    {
        let target = format!(
            "{}:{}",
            r.executor,
            r.model.as_deref().unwrap_or("(default)")
        );
        by_target.entry(target).or_default().push(r);
    }
    by_target
        .into_iter()
        .map(|(target, rows)| {
            let passed = |r: &HistoryRecord| r.eval.as_ref().is_some_and(|e| e.passed);
            let sum_opt = |f: &dyn Fn(&HistoryRecord) -> Option<f64>| {
                let known: Vec<f64> = rows.iter().filter_map(|r| f(r)).collect();
                (!known.is_empty()).then(|| known.iter().sum::<f64>())
            };
            EvalSummary {
                cases: rows.len(),
                passed: rows.iter().filter(|r| passed(r)).count(),
                failed_cases: rows
                    .iter()
                    .filter(|r| !passed(r))
                    .filter_map(|r| r.eval.as_ref().map(|e| e.case.clone()))
                    .collect(),
                median_ms: median(rows.iter().map(|r| r.duration_ms).collect()),
                #[allow(
                    clippy::cast_precision_loss,
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss
                )]
                tokens: sum_opt(&|r| r.tokens.map(|t| t as f64)).map(|t| t as u64),
                usd: sum_opt(&|r| r.usd),
                tool_calls: rows.iter().map(|r| r.tool_calls).sum(),
                files_touched: rows.iter().map(|r| r.files_touched).sum(),
                target,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str, executor: &str, ok: bool, ms: u64) -> HistoryRecord {
        HistoryRecord {
            task_id: id.to_owned(),
            at: 1,
            task_kind: TaskKind::Review,
            executor: executor.to_owned(),
            model: Some("m".to_owned()),
            billing: AgentBilling::FreeTier,
            succeeded: ok,
            duration_ms: ms,
            tool_calls: 3,
            tokens: Some(100),
            usd: None,
            files_touched: 0,
            eval: None,
            shadow_choice: None,
        }
    }

    #[test]
    fn stats_group_by_harness_and_feedback_overrides_success() {
        let records = vec![
            record("a", "devin", true, 1_000),
            record("b", "devin", true, 3_000),
            record("c", "opencode", true, 2_000),
        ];
        let feedback = vec![Feedback {
            task_id: "b".to_owned(),
            at: 2,
            verdict: Verdict::Reject,
            note: None,
        }];
        let rows = stats(&records, &feedback);
        assert_eq!(
            rows.len(),
            2,
            "same model through two harnesses is two rows"
        );
        let devin = rows.iter().find(|r| r.executor == "devin").expect("devin");
        assert_eq!((devin.n, devin.succeeded, devin.rejected), (2, 2, 1));
        assert_eq!(devin.median_ms, 3_000);
        assert_eq!(devin.mean_usd, None, "unknown cost stays unknown");
        let opencode = rows.iter().find(|r| r.executor == "opencode").expect("oc");
        assert!(opencode.score > devin.score - 0.2);
    }

    #[test]
    fn grading_is_deterministic() {
        let case = EvalCase {
            id: "Q2".to_owned(),
            kind: TaskKind::Debug,
            objective: "x".to_owned(),
            success_criteria: Vec::new(),
            must_contain_all: vec!["config.rs".to_owned()],
            must_contain_any: vec!["4000".to_owned(), "4_000".to_owned()],
            min_chars: 5,
            requires_write: false,
        };
        assert!(grade("r", &case, true, "see Config.rs: 4_000 ms", 0).passed);
        let g = grade("r", &case, true, "see route_gate.rs", 0);
        assert!(!g.passed);
        assert_eq!(g.checks.iter().filter(|(_, ok)| !ok).count(), 2);
        assert!(!grade("r", &case, false, "config.rs 4000", 0).passed);
        let write = EvalCase {
            requires_write: true,
            must_contain_all: Vec::new(),
            must_contain_any: Vec::new(),
            ..case
        };
        assert!(
            !grade("r", &write, true, "all done", 0).passed,
            "a write case must change files"
        );
        assert!(grade("r", &write, true, "all done", 2).passed);
    }

    #[test]
    fn the_default_battery_parses_and_reports() {
        let battery = Battery::default_battery();
        assert_eq!(battery.cases.len(), 7);
        assert!(battery.cases.iter().any(|c| c.requires_write));
        let mut r = record("a", "devin", true, 10);
        r.eval = Some(EvalGrade {
            run_id: "e1".to_owned(),
            case: "Q1".to_owned(),
            passed: true,
            checks: Vec::new(),
        });
        let report = eval_report("e1", &[r, record("b", "devin", true, 10)]);
        assert_eq!(report.len(), 1);
        assert_eq!((report[0].cases, report[0].passed), (1, 1));
        assert_eq!(report[0].target, "devin:m");
    }
}
