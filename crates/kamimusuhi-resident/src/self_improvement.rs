//! Code self-improvement reuses the task plane; model reports never count
//! as validation. The resident runs operator-configured commands itself.
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use crate::{state::Shared, tasks::TaskStatus};
use kamimusuhi_runtime::agent_exec::process::{self, ProcessSpec};
use serde_json::{Value, json};

pub fn definitions() -> Vec<Value> {
    vec![json!({"type":"function","function":{
        "name":"self_improve",
        "description":"Improve your code in an isolated task worktree. start takes objective and returns a task id; status polls it; apply runs the operator-configured tests AND evaluation, then applies only a successful, unchanged candidate to its unchanged base. Failed checks preserve the branch. Requires self_improvement policy. Applies source code; does not deploy/restart services.",
        "parameters":{"type":"object","properties":{
            "action":{"type":"string","enum":["start","status","apply"]},
            "objective":{"type":"string"}, "id":{"type":"string"},
            "executor":{"type":"string"}, "model":{"type":"string"}},"required":["action"]}
    }})]
}

fn fail(error: impl Into<String>) -> (u16, Value) {
    (200, json!({"ok":false,"error":error.into()}))
}

pub fn call(shared: &Shared, args: &Value, turn: Option<&str>) -> (u16, Value) {
    let Some(policy) = &shared.config.self_improvement else {
        return fail("self_improvement policy is not configured");
    };
    match args["action"].as_str() {
        Some("start") => {
            let request = json!({"objective": args["objective"], "kind":"coding",
                "workspace":policy.workspace,"permissions":"workspace_write",
                "executor":args["executor"],"model":args["model"],
                "success_criteria":["Implement the objective, add relevant regression coverage and explain evaluation. Do not deploy or change credentials."]});
            let (status, reply) =
                crate::task_orchestrator::tool(shared, "task_delegate", &request, turn);
            if reply["ok"] == true
                && let Some(id) = reply["result"]["task_id"].as_str()
            {
                shared.tasks.update(
                    &shared.spool,
                    id,
                    None,
                    None,
                    Some(json!({"self_improvement":true})),
                );
            }
            (status, reply)
        }
        Some("status" | "apply") => {
            let Some(task) = args["id"].as_str().and_then(|id| shared.tasks.get(id)) else {
                return fail("unknown task id");
            };
            if task.kind != "delegated"
                || task.detail["self_improvement"] != true
                || task.detail["workspace"] != policy.workspace
            {
                return fail("task is not a self-improvement candidate for this workspace");
            }
            if args["action"] == "status" {
                return crate::task_orchestrator::tool(shared, "task_status", args, turn);
            }
            let Ok(_guard) = shared.workspace_mutation.try_lock() else {
                return fail("workspace validation/application in progress");
            };
            if task.status != TaskStatus::Done {
                return fail("only successfully completed tasks may be evaluated/applied");
            }
            let Some(plane) = shared.task_plane.as_ref() else {
                return fail("no task plane");
            };
            let (_, workspace) = match plane.workspace(Some(&policy.workspace)) {
                Ok(w) => w,
                Err(e) => return fail(e),
            };
            let Some(path) = task.detail["worktree"]
                .as_str()
                .map(std::path::PathBuf::from)
            else {
                return fail("candidate has no worktree");
            };
            let Some(branch) = task.detail["branch"].as_str() else {
                return fail("missing branch");
            };
            let Some(base) = task.detail["base_commit"].as_str() else {
                return fail("missing base");
            };
            if !path.is_dir() {
                return fail("candidate was already applied or discarded");
            }
            // Continuing changes the provenance of a candidate. Never use an
            // older successful task to apply a later failed/cancelled run.
            if shared.tasks.list(true).iter().any(|t| {
                t["kind"] == "delegated"
                    && t["id"] != task.id
                    && t["detail"]["worktree"] == json!(path)
            }) {
                return fail("candidate has a continuation; start a new self-improvement task");
            }
            let head = match git(&path, &["rev-parse", "HEAD"]) {
                Ok(h) => h,
                Err(e) => return fail(e),
            };
            if task.detail["candidate_head"].as_str() != Some(head.as_str()) {
                return fail(
                    "candidate differs from the completed task revision; start a new self-improvement task",
                );
            }
            let unchanged = || -> Result<(), String> {
                if git(&workspace, &["rev-parse", "HEAD"])? != base {
                    return Err(
                        "workspace HEAD advanced; regenerate candidate on current base".into(),
                    );
                }
                if git(&path, &["rev-parse", "HEAD"])? != head
                    || git(&path, &["rev-parse", branch])? != head
                    || git(&path, &["symbolic-ref", "--short", "HEAD"])? != branch
                {
                    return Err("candidate revision or branch changed during validation".into());
                }
                for tree in [&workspace, &path] {
                    if !git(tree, &["status", "--porcelain"])?.is_empty() {
                        return Err("workspace/candidate has uncommitted or untracked files".into());
                    }
                }
                Ok(())
            };
            if let Err(e) = unchanged() {
                return fail(e);
            }
            let deadline = Instant::now() + Duration::from_secs(policy.timeout_secs);
            let mut checks = Vec::new();
            for (phase, commands) in [("test", &policy.tests), ("evaluation", &policy.evaluation)] {
                for command in commands {
                    let Some((program, argv)) = command.split_first() else {
                        return fail("empty validation command");
                    };
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    let outcome = process::run(
                        &ProcessSpec {
                            program,
                            args: argv,
                            cwd: Some(&path),
                            env_passthrough: &[],
                            extra_env: &[],
                            timeout: remaining,
                        },
                        &AtomicBool::new(false),
                        |_| {},
                    );
                    // Test stdout may contain private content. Persist only status.
                    let passed = outcome.as_ref().is_ok_and(process::ProcessResult::success);
                    checks.push(json!({"phase":phase,"argv":command,"passed":passed,
                        "exit_code":outcome.as_ref().ok().and_then(|o|o.exit_code),
                        "duration_ms":outcome.as_ref().ok().map(|o|o.duration_ms)}));
                    if !passed {
                        record(shared, &task.id, &head, &checks, false);
                        return (
                            200,
                            json!({"ok":false,"error":"validation failed; candidate preserved",
                            "result":{"checks":checks,"applied":false}}),
                        );
                    }
                }
            }
            if let Err(e) = unchanged() {
                record(shared, &task.id, &head, &checks, false);
                return fail(e);
            }
            let (status, result) =
                crate::task_orchestrator::worktree_action_locked(shared, args, "operator", true);
            record(shared, &task.id, &head, &checks, status == 200);
            if status == 200 {
                (
                    200,
                    json!({"ok":true,"result":{"applied":true,"checks":checks,"application":result}}),
                )
            } else {
                fail(result["error"].as_str().unwrap_or("application failed"))
            }
        }
        _ => fail("action must be start, status or apply"),
    }
}

fn record(shared: &Shared, id: &str, head: &str, checks: &[Value], applied: bool) {
    let detail =
        json!({"self_improvement_validation":{"head":head,"checks":checks,"applied":applied}});
    shared
        .tasks
        .update(&shared.spool, id, None, None, Some(detail.clone()));
    let _ = shared.spool.append(
        "logs/self-improvement",
        json!({"task_id":id,"validation":detail}),
    );
}

fn git(path: &Path, args: &[&str]) -> Result<String, String> {
    let argv: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
    let outcome = process::run(
        &ProcessSpec {
            program: "git",
            args: &argv,
            cwd: Some(path),
            env_passthrough: &[],
            extra_env: &[],
            timeout: Duration::from_secs(30),
        },
        &AtomicBool::new(false),
        |_| {},
    )?;
    if outcome.success() {
        Ok(outcome.stdout.trim().to_owned())
    } else {
        Err("git inspection failed".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, spool::Spool, tasks::NewTask};

    fn fixture(evaluation: Vec<String>) -> (Shared, tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir(&ws).unwrap();
        git(&ws, &["init", "-q"]).unwrap();
        std::fs::write(ws.join("code.txt"), "before\n").unwrap();
        git(&ws, &["add", "."]).unwrap();
        git(
            &ws,
            &[
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@local",
                "commit",
                "-qm",
                "initial",
            ],
        )
        .unwrap();
        let config: Config = serde_json::from_value(json!({
            "node":{"id":"test","role":"cognition","listen":"127.0.0.1:0"},
            "paths":{"root":dir.path()},
            "task_plane":{"workspaces":[{"name":"self","path":ws,"allow_write":true}],"executors":[]},
            "self_improvement":{"workspace":"self","tests":[["git","diff","--check","HEAD~1","HEAD"]],
                "evaluation":[evaluation],"timeout_secs":10}
        })).unwrap();
        config.validate().unwrap();
        let spool = Spool::new(dir.path().join("spool"), "test").unwrap();
        let shared = Shared::new(config, spool, 1, None);
        let id = shared.tasks.create(
            &shared.spool,
            NewTask {
                title: "candidate",
                kind: "delegated",
                node: "test",
                owner: "mio",
                status: TaskStatus::Done,
                depends_on: vec![],
                detail: json!({}),
            },
        );
        let wt = crate::task_worktree::create(&ws, &dir.path().join("worktrees"), &id).unwrap();
        std::fs::write(wt.path.join("code.txt"), "after\n").unwrap();
        let captured = crate::task_worktree::capture(&wt, "fix: improve fixture").unwrap();
        shared.tasks.update(
            &shared.spool,
            &id,
            None,
            None,
            Some(json!({"self_improvement":true,
            "workspace":"self","branch":wt.branch,"base_commit":wt.base,"worktree":wt.path,"candidate_head":captured.head})),
        );
        (shared, dir, id)
    }

    #[test]
    fn self_improvement_passes_tests_and_evaluation_before_apply() {
        let (shared, dir, id) = fixture(vec!["git".into(), "diff".into(), "--check".into()]);
        let (_, reply) = call(&shared, &json!({"action":"apply","id":id}), None);
        assert_eq!(reply["ok"], true, "{reply}");
        assert_eq!(reply["result"]["applied"], true);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("ws/code.txt")).unwrap(),
            "after\n"
        );
        let (_, again) = call(&shared, &json!({"action":"apply","id":id}), None);
        assert_eq!(again["ok"], false);
    }

    #[test]
    fn self_improvement_failed_evaluation_preserves_candidate() {
        let (shared, dir, id) = fixture(vec![
            "git".into(),
            "diff".into(),
            "--exit-code".into(),
            "HEAD~1".into(),
            "HEAD".into(),
        ]);
        let (_, reply) = call(&shared, &json!({"action":"apply","id":id}), None);
        assert_eq!(reply["ok"], false, "{reply}");
        assert_eq!(reply["result"]["checks"][0]["passed"], true);
        assert_eq!(reply["result"]["checks"][1]["passed"], false);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("ws/code.txt")).unwrap(),
            "before\n"
        );
        let task = shared.tasks.get(&id).unwrap();
        assert!(Path::new(task.detail["worktree"].as_str().unwrap()).is_dir());
    }

    #[test]
    fn self_improvement_rejects_dirty_candidate_and_advanced_base() {
        let (shared, dir, id) = fixture(vec!["git".into(), "diff".into(), "--check".into()]);
        let task = shared.tasks.get(&id).unwrap();
        let path = Path::new(task.detail["worktree"].as_str().unwrap());
        std::fs::write(path.join("untracked"), "x").unwrap();
        let (_, reply) = call(&shared, &json!({"action":"apply","id":id}), None);
        assert_eq!(reply["ok"], false);
        assert!(reply["error"].as_str().unwrap().contains("untracked"));
        std::fs::remove_file(path.join("untracked")).unwrap();
        git(
            &dir.path().join("ws"),
            &[
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@local",
                "commit",
                "--allow-empty",
                "-qm",
                "advance",
            ],
        )
        .unwrap();
        let (_, reply) = call(&shared, &json!({"action":"apply","id":id}), None);
        assert!(reply["error"].as_str().unwrap().contains("advanced"));
    }

    #[test]
    fn self_improvement_rejects_validation_that_changes_candidate() {
        let (shared, dir, id) = fixture(vec!["touch".into(), "unexpected".into()]);
        let (_, reply) = call(&shared, &json!({"action":"apply","id":id}), None);
        assert_eq!(reply["ok"], false);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("ws/code.txt")).unwrap(),
            "before\n"
        );
    }
    #[test]
    fn self_improvement_rejects_old_task_after_failed_continuation() {
        let (shared, dir, id) = fixture(vec!["git".into(), "diff".into(), "--check".into()]);
        let task = shared.tasks.get(&id).unwrap();
        shared.tasks.create(
            &shared.spool,
            NewTask {
                title: "failed continuation",
                kind: "delegated",
                node: "test",
                owner: "mio",
                status: TaskStatus::Failed,
                depends_on: vec![],
                detail: json!({"worktree":task.detail["worktree"]}),
            },
        );
        let (_, reply) = call(&shared, &json!({"action":"apply","id":id}), None);
        assert_eq!(reply["ok"], false);
        assert!(reply["error"].as_str().unwrap().contains("continuation"));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("ws/code.txt")).unwrap(),
            "before\n"
        );
    }
}
