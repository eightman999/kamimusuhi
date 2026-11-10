//! Context-driven contract evaluation, not a benchmark of model coding skill.
//!
//! The decision policy is stateless. Only the observer outside it records
//! calls. Git, worktrees, checks and checkpoint reloads use the real harness.
//! The testkit's resource fakes target CognitiveResource, while this boundary
//! is EngineerModel; reuse the resident fixture without a reverse dependency.

use super::*;

const OBJECTIVE: &str = "Fix code.txt and preserve the operator's workspace";
const MEMORY_REQUEST: &str = "Checkpoint before editing.";
const MEMORY_SUMMARY: &str = "code.txt を fixed に直す。元の workspace と notes.txt を保持する。";
const MEMORY_NEXT: &str = "候補の code.txt を読み、必要な置換を選び、設定済み検証を実行する。";

struct ReactiveEngineerModel;

fn inspect_source() -> Action {
    Action::ReadRange {
        path: "code.txt".to_owned(),
        start_line: 1,
        max_lines: 10,
    }
}

fn observed_detail(record: &Value) -> Result<Value, String> {
    if record["context_truncated"] == true {
        return Err("fixture policy requires a complete observation".to_owned());
    }
    let text = record["external_data"]
        .as_str()
        .ok_or("fixture observation has no external data")?;
    serde_json::from_str(text).map_err(|_| "fixture observation is not complete JSON".to_owned())
}

impl EngineerModel for ReactiveEngineerModel {
    fn decide(&self, context: &Value) -> Result<Action, String> {
        let objective = context["objective"]
            .as_str()
            .ok_or("fixture objective is missing")?;
        if !objective.starts_with(OBJECTIVE) {
            return Err("fixture policy received another objective".to_owned());
        }
        if context["configured_checks"]
            .as_array()
            .is_none_or(Vec::is_empty)
        {
            return Err("fixture policy has no independent check configuration".to_owned());
        }
        let wants_memory = objective.ends_with(MEMORY_REQUEST);
        let memory = &context["checkpoint"]["self_authored_unverified"];
        if wants_memory {
            if memory["summary"] == "" && memory["next_step"] == "" {
                return Ok(Action::Checkpoint {
                    summary: MEMORY_SUMMARY.to_owned(),
                    next_step: MEMORY_NEXT.to_owned(),
                });
            }
            if memory["summary"] != MEMORY_SUMMARY || memory["next_step"] != MEMORY_NEXT {
                return Err("fixture policy cannot recover its saved plan".to_owned());
            }
        }

        let latest = context["observations"].as_array().and_then(|v| v.last());
        if let Some(record) = latest {
            let detail = observed_detail(record)?;
            if detail["kind"] == "interrupted_action_outcome_unknown" {
                // The receipt cannot tell us whether the replacement ran.
                // A fresh read, not the claimed action, determines the branch.
                return Ok(inspect_source());
            }
            if record["action"] == "read_range" && record["ok"] == true {
                if detail["path"] != "code.txt" || !detail["next_line"].is_null() {
                    return Err("fixture source observation is incomplete".to_owned());
                }
                let content = detail["content"]
                    .as_str()
                    .ok_or("fixture source observation has no text")?;
                return match content {
                    "broken\n" => Ok(Action::Replace {
                        path: detail["path"].as_str().unwrap().to_owned(),
                        old: content.trim_end_matches('\n').to_owned(),
                        new: "fixed".to_owned(),
                    }),
                    "fixed\n" => Ok(finish()),
                    _ => Err("fixture policy has no repair for the observed source".to_owned()),
                };
            }
            if record["action"] == "replace" && record["ok"] == true {
                return Ok(finish());
            }
            if record["action"] == "checkpoint" && record["ok"] == true {
                return Ok(inspect_source());
            }
        }
        let validation = &context["validation"];
        if validation["passed"] == false
            && validation["checks"].as_array().is_some_and(|checks| {
                checks
                    .iter()
                    .any(|check| check["exit_code"].as_i64().is_some_and(|code| code != 0))
            })
        {
            return Ok(inspect_source());
        }
        if wants_memory {
            return Ok(inspect_source());
        }
        if latest.is_none_or(|record| record["action"] == "prepare") {
            // The first scenario deliberately makes the harness obtain real
            // failure evidence before the policy chooses a source repair.
            return Ok(finish());
        }
        Err("fixture policy needs source or verification evidence".to_owned())
    }
}

#[derive(Default)]
struct ObservedModel {
    contexts: Mutex<Vec<Value>>,
}

impl ObservedModel {
    fn calls(&self) -> usize {
        self.contexts.lock().unwrap().len()
    }
}

impl EngineerModel for ObservedModel {
    fn decide(&self, context: &Value) -> Result<Action, String> {
        self.contexts.lock().unwrap().push(context.clone());
        ReactiveEngineerModel.decide(context)
    }
}

struct Original {
    candidate: Value,
    head: String,
    code: Vec<u8>,
    notes: Vec<u8>,
}

fn start(fixture: &Fixture, objective: &str) -> (String, Original) {
    let (status, reply) = handle(
        &fixture.shared,
        &json!({"action":"create","workspace":"fixture","objective":objective}),
    );
    assert_eq!(status, 201, "{reply}");
    let id = reply["job"]["id"].as_str().unwrap().to_owned();
    prepare(&fixture.shared, &id);
    let original = Original {
        candidate: job(&fixture.shared, &id)["candidate"].clone(),
        head: git(&fixture.workspace, &["rev-parse", "HEAD"]),
        code: std::fs::read(fixture.workspace.join("code.txt")).unwrap(),
        notes: std::fs::read(fixture.workspace.join("notes.txt")).unwrap(),
    };
    (id, original)
}

fn drive(shared: &Shared, id: &str, model: &ObservedModel) {
    // A fixture watchdog bounds a regression. This is not a lifetime budget
    // in the agent and never selects the policy's next action.
    for _ in 0..8 {
        if job(shared, id)["phase"] == "ready" {
            return;
        }
        let advanced = tick_with(shared, model).expect("reactive fixture tick");
        assert!(advanced.is_some(), "reactive job did not advance");
    }
    assert_eq!(
        job(shared, id)["phase"],
        "ready",
        "fixture watchdog reached"
    );
}

#[derive(Default)]
struct Accounting {
    previous_model_calls: usize,
    injected_effects: usize,
    explicit_resumes: usize,
    memory_preserved: Option<bool>,
}

fn result(
    name: &str,
    fixture: &Fixture,
    id: &str,
    original: &Original,
    model: &ObservedModel,
    accounting: Accounting,
) -> Value {
    let completed = job(&fixture.shared, id);
    assert_eq!(completed["phase"], "ready");
    assert_eq!(completed["validation"]["passed"], true);
    assert_eq!(completed["validation"]["checks"][0]["exit_code"], 0);
    assert_eq!(completed["candidate"], original.candidate);
    assert_eq!(
        git(&fixture.workspace, &["rev-parse", "HEAD"]),
        original.head
    );
    assert_eq!(
        std::fs::read(fixture.workspace.join("code.txt")).unwrap(),
        original.code
    );
    assert_eq!(
        std::fs::read(fixture.workspace.join("notes.txt")).unwrap(),
        original.notes
    );
    assert_eq!(
        std::fs::read_to_string(candidate(&fixture.shared, id).join("code.txt")).unwrap(),
        "fixed\n"
    );
    assert_eq!(
        std::fs::read(candidate(&fixture.shared, id).join("notes.txt")).unwrap(),
        original.notes
    );
    assert_eq!(count(&fixture.command_count), 0);
    let replacements = completed["observations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| record["action"] == "replace" && record["ok"] == true)
        .count();
    // All cases are shorter than the observation window, so the successful
    // replacement receipts and driver-injected crash effects account for all
    // changes in this suite. They are not a general exactly-once proof.
    assert_eq!(replacements + accounting.injected_effects, 1);
    let calls = model.calls();
    let checks = count(&fixture.validation_count);
    assert!(tick_with(&fixture.shared, model).unwrap().is_none());
    assert_eq!(model.calls(), calls, "Ready must not make another decision");
    assert_eq!(count(&fixture.validation_count), checks);
    let metrics = &completed["metrics"];
    json!({
        "case_id":name,"ready":true,
        "model_calls":calls+accounting.previous_model_calls,
        "decisions":metrics["decisions"],"actions":metrics["actions"],
        "verification_runs":metrics["verification_runs"],
        "verification_failures":metrics["verification_failures"],
        "repairs":metrics["repairs"],"interruptions":metrics["interruptions"],
        "model_failures":metrics["model_failures"],"actual_checks":checks,
        "completed_replacements":replacements,"injected_effects":accounting.injected_effects,
        "duplicate_effects":0,"extra_calls_after_ready":0,"extra_checks_after_ready":0,
        "explicit_resumes":accounting.explicit_resumes,
        "working_memory_preserved":accounting.memory_preserved,
        "candidate_identity_preserved":true,"original_checkout_preserved":true
    })
}

fn assert_metrics(actual: &Value, expected: Value) {
    for (key, expected) in expected.as_object().unwrap() {
        assert_eq!(&actual[key], expected, "{}: {key}", actual["case_id"]);
    }
}

fn failed_validation_case() -> Value {
    let fixture = fixture();
    let (id, original) = start(&fixture, OBJECTIVE);
    let model = ObservedModel::default();
    drive(&fixture.shared, &id, &model);
    let contexts = model.contexts.lock().unwrap();
    assert_eq!(contexts[1]["validation"]["passed"], false);
    assert_eq!(contexts[1]["validation"]["checks"][0]["exit_code"], 1);
    assert_eq!(
        contexts[2]["observations"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["action"],
        "read_range"
    );
    drop(contexts);
    let report = result(
        "real_failure_then_repair",
        &fixture,
        &id,
        &original,
        &model,
        Accounting::default(),
    );
    assert_metrics(
        &report,
        json!({
            "model_calls":4,"decisions":4,"actions":4,"actual_checks":2,
            "verification_runs":2,"verification_failures":1,"repairs":1,
            "interruptions":0,"model_failures":0,"completed_replacements":1,"explicit_resumes":0
        }),
    );
    report
}

fn saved_memory_case() -> Value {
    let fixture = fixture();
    let objective = format!("{OBJECTIVE}. {MEMORY_REQUEST}");
    let (id, original) = start(&fixture, &objective);
    let first_model = ObservedModel::default();
    let saved = tick_with(&fixture.shared, &first_model).unwrap().unwrap();
    assert_eq!(saved.checkpoint.summary, MEMORY_SUMMARY);
    assert_eq!(saved.checkpoint.next_step, MEMORY_NEXT);
    assert_eq!(saved.metrics.actions, 1);
    let previous_model_calls = first_model.calls();
    assert_eq!(previous_model_calls, 1);
    drop(first_model);
    let fixture = restart(fixture);
    assert_eq!(job(&fixture.shared, &id)["metrics"]["actions"], 1);
    assert_eq!(count(&fixture.validation_count), 0);
    let model = ObservedModel::default();
    drive(&fixture.shared, &id, &model);
    let contexts = model.contexts.lock().unwrap();
    assert_eq!(contexts[0]["objective"], objective);
    assert_eq!(contexts[0]["candidate"], original.candidate);
    assert_eq!(
        contexts[0]["checkpoint"]["self_authored_unverified"]["summary"],
        MEMORY_SUMMARY
    );
    assert_eq!(
        contexts[0]["checkpoint"]["self_authored_unverified"]["next_step"],
        MEMORY_NEXT
    );
    drop(contexts);
    let report = result(
        "fresh_model_after_checkpoint_reload",
        &fixture,
        &id,
        &original,
        &model,
        Accounting {
            previous_model_calls,
            memory_preserved: Some(true),
            ..Accounting::default()
        },
    );
    assert_metrics(
        &report,
        json!({
            "model_calls":4,"decisions":4,"actions":4,"actual_checks":1,
            "verification_runs":1,"verification_failures":0,"repairs":0,
            "interruptions":0,"model_failures":0,"completed_replacements":1,"explicit_resumes":0
        }),
    );
    report
}

fn uncertain_replacement_case(applied: bool) -> Value {
    let fixture = fixture();
    let (id, original) = start(&fixture, OBJECTIVE);
    if applied {
        // The driver injects a crash state. The decision policy cannot read
        // this boolean or the filesystem; only a later ReadRange reveals it.
        std::fs::write(candidate(&fixture.shared, &id).join("code.txt"), "fixed\n").unwrap();
    }
    set_pending(
        &fixture,
        &id,
        Action::Replace {
            path: "code.txt".to_owned(),
            old: "broken".to_owned(),
            new: "fixed".to_owned(),
        },
    );
    let fixture = restart(fixture);
    let recovered = job(&fixture.shared, &id);
    assert_eq!(recovered["phase"], "paused");
    assert!(recovered["pending"].is_null());
    assert_eq!(recovered["candidate"], original.candidate);
    let model = ObservedModel::default();
    assert!(tick_with(&fixture.shared, &model).unwrap().is_none());
    assert_eq!(model.calls(), 0);
    assert_eq!(count(&fixture.validation_count), 0);
    assert_eq!(count(&fixture.command_count), 0);
    let (_, resumed) = handle(
        &fixture.shared,
        &json!({
            "action":"resume","id":id,"note":"Inspect the preserved candidate before choosing a new action."
        }),
    );
    assert_eq!(resumed["ok"], true);
    drive(&fixture.shared, &id, &model);
    let contexts = model.contexts.lock().unwrap();
    let read = contexts[1]["observations"]
        .as_array()
        .unwrap()
        .last()
        .unwrap();
    assert_eq!(read["action"], "read_range");
    assert_eq!(
        observed_detail(read).unwrap()["content"],
        if applied { "fixed\n" } else { "broken\n" }
    );
    drop(contexts);
    let name = if applied {
        "uncertain_replace_already_applied"
    } else {
        "uncertain_replace_not_applied"
    };
    let report = result(
        name,
        &fixture,
        &id,
        &original,
        &model,
        Accounting {
            injected_effects: usize::from(applied),
            explicit_resumes: 1,
            ..Accounting::default()
        },
    );
    assert_metrics(
        &report,
        json!({
            "model_calls":if applied {2} else {3},
            "decisions":if applied {3} else {4},
            "actions":if applied {2} else {3},
            "actual_checks":1,"verification_runs":1,"verification_failures":0,
            "repairs":0,"interruptions":1,"model_failures":0,
            "completed_replacements":if applied {0} else {1},"explicit_resumes":1
        }),
    );
    report
}

#[test]
fn reactive_engineer_contract_suite_uses_observations_to_repair_and_resume() {
    let started = std::time::Instant::now();
    let cases = vec![
        failed_validation_case(),
        saved_memory_case(),
        uncertain_replacement_case(true),
        uncertain_replacement_case(false),
    ];
    let sum = |key: &str| {
        cases
            .iter()
            .map(|case| case[key].as_u64().unwrap())
            .sum::<u64>()
    };
    let successes = cases.iter().filter(|case| case["ready"] == true).count();
    assert_eq!(successes, 4);
    assert_eq!(sum("model_calls"), 13);
    assert_eq!(sum("decisions"), 15);
    assert_eq!(sum("actions"), 13);
    assert_eq!(sum("actual_checks"), 5);
    assert_eq!(sum("repairs"), 1);
    assert_eq!(sum("duplicate_effects"), 0);
    let totals = json!({
        "cases":cases.len(),"ready":successes,"success_rate":successes as f64/cases.len() as f64,
        "model_calls":sum("model_calls"),"decisions":sum("decisions"),"actions":sum("actions"),
        "actual_checks":sum("actual_checks"),"repairs":sum("repairs"),
        "interruptions":sum("interruptions"),"duplicate_effects":sum("duplicate_effects"),
        "completed_without_explicit_resume":2,"completed_after_explicit_resume":2,
        "explicit_resumes":sum("explicit_resumes"),"token_usage":null,
        "elapsed_ms":started.elapsed().as_millis()
    });
    println!(
        "{}",
        json!({
            "format_version":1,"suite_id":"native-engineering-reactive-contracts-v1",
            "scope":"deterministic context-driven policy; no real provider or model-quality claim",
            "restart_scope":"Shared and model recreation from durable state, not process kill",
            "cases":cases,"totals":totals
        })
    );
}
