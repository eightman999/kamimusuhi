//! Deterministic resident-harness checks. Model decisions and the execution
//! location are injected; worktrees, commands, checks and state use real files.
//! Separate integrations exercise the real worker and resident processes.

use super::*;
use crate::config::Config;
use crate::spool::Spool;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, mpsc};
use std::time::Duration;

mod context_budget;
mod cursor;
mod evaluation;
mod model_recovery;
mod validation_memory;
mod verification_identity;
mod worker_dispatch;

/// In-process runner that still performs the full worker admission —
/// request build, job lock, ledger check, claim match and durable
/// reservation — skipping only the IPC transport and spawn.
struct InlineRunner;

impl ActionRunner for InlineRunner {
    fn execute(
        &self,
        state_path: &Path,
        claimed: &Job,
        policy: &WorkspaceConfig,
        limits: &workspace::Limits,
        cancel: &AtomicBool,
    ) -> worker::Dispatch {
        let request = match worker::Request::new(state_path, claimed, policy, limits) {
            Ok(request) => request,
            Err(error) => return worker::Dispatch::Rejected(error),
        };
        worker::execute_inline(&request, cancel)
    }
}

fn tick_with(shared: &Shared, model: &dyn EngineerModel) -> Result<Option<Job>, String> {
    super::tick_with_runner(shared, model, &InlineRunner)
}

struct ScriptedModel {
    actions: Mutex<VecDeque<Action>>,
    contexts: Mutex<Vec<Value>>,
}

impl ScriptedModel {
    fn new(actions: impl IntoIterator<Item = Action>) -> Self {
        Self {
            actions: Mutex::new(actions.into_iter().collect()),
            contexts: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> usize {
        self.contexts.lock().unwrap().len()
    }

    fn assert_consumed(&self) {
        assert!(self.actions.lock().unwrap().is_empty());
    }
}

impl EngineerModel for ScriptedModel {
    fn decide(&self, context: &Value) -> Result<Action, String> {
        self.contexts.lock().unwrap().push(context.clone());
        self.actions
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| "fixture model received an unexpected extra decision".to_owned())
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    shared: Shared,
    workspace: PathBuf,
    validation_count: PathBuf,
    command_count: PathBuf,
    command_script: PathBuf,
}

fn git(path: &Path, args: &[&str]) -> String {
    let result = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("run fixture git");
    assert!(
        result.status.success(),
        "fixture git {:?}: {}",
        args,
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    git(&workspace, &["init", "-q"]);
    std::fs::write(workspace.join("code.txt"), "broken\n").unwrap();
    std::fs::write(workspace.join("notes.txt"), "original notes\n").unwrap();
    git(&workspace, &["add", "."]);
    git(
        &workspace,
        &[
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@local",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "initial",
        ],
    );

    // Counters live outside either worktree: inspecting the outcome cannot
    // itself dirty the candidate or turn validation into a code modification.
    let validation_count = dir.path().join("validation-count");
    let validator = dir.path().join("validate.sh");
    std::fs::write(
        &validator,
        "#!/bin/sh\nset -eu\nprintf 'check\\n' >> \"$1\"\ntest \"$(cat code.txt)\" = fixed\n",
    )
    .unwrap();
    let command_count = dir.path().join("command-count");
    let command_script = dir.path().join("command.sh");
    std::fs::write(
        &command_script,
        "#!/bin/sh\nset -eu\nprintf 'run\\n' >> \"$1\"\n",
    )
    .unwrap();

    let config: Config = serde_json::from_value(json!({
        "node": {"id": "test", "role": "cognition", "listen": "127.0.0.1:0"},
        "paths": {"root": dir.path()},
        "engineering": {
            "model": "auto",
            "operation_timeout_secs": 5,
            "validation_timeout_secs": 5,
            "workspaces": [{
                "name": "fixture",
                "path": workspace,
                "checks": [["sh", validator, validation_count]],
                "commands": ["sh"]
            }]
        }
    }))
    .unwrap();
    config.validate().unwrap();
    let spool = Spool::new(dir.path().join("spool"), "test").unwrap();
    let shared = Shared::new(config, spool, 1, None);
    assert!(
        shared.task_plane.is_none(),
        "native loop needs no CLI executor"
    );
    Fixture {
        dir,
        shared,
        workspace,
        validation_count,
        command_count,
        command_script,
    }
}

fn job(shared: &Shared, id: &str) -> Value {
    serde_json::to_value(shared.engineering.get(id).expect("job exists")).unwrap()
}

fn create(shared: &Shared) -> String {
    let (status, reply) = handle(
        shared,
        &json!({
            "action": "create", "workspace": "fixture",
            "objective": "Fix code.txt and preserve the operator's workspace"
        }),
    );
    assert_eq!(status, 201, "{reply}");
    assert_eq!(reply["ok"], true, "{reply}");
    reply["job"]["id"].as_str().unwrap().to_owned()
}

fn prepare(shared: &Shared, id: &str) {
    assert_eq!(job(shared, id)["phase"], "preparing");
    let model = ScriptedModel::new([]);
    let result = tick_with(shared, &model).expect("prepare tick");
    assert!(result.is_some());
    assert_eq!(job(shared, id)["phase"], "active");
    assert_eq!(model.calls(), 0, "preparation is not a model decision");
}

fn action(shared: &Shared, model: &ScriptedModel) {
    assert!(tick_with(shared, model).expect("action tick").is_some());
}

fn candidate(shared: &Shared, id: &str) -> PathBuf {
    PathBuf::from(job(shared, id)["candidate"]["path"].as_str().unwrap())
}

fn count(path: &Path) -> usize {
    match std::fs::read_to_string(path) {
        Ok(contents) => contents.lines().count(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => panic!("read fixture counter: {error}"),
    }
}

fn restart(fixture: Fixture) -> Fixture {
    let Fixture {
        dir,
        shared,
        workspace,
        validation_count,
        command_count,
        command_script,
    } = fixture;
    let config = shared.config.clone();
    let boot_epoch = shared.boot_epoch + 1;
    // A restarted resident must acquire its own lease only after the old
    // instance has dropped every store and file handle for this root.
    drop(shared);
    let spool = Spool::new(dir.path().join("spool"), "test").unwrap();
    let shared = Shared::new(config, spool, boot_epoch, None);
    Fixture {
        dir,
        shared,
        workspace,
        validation_count,
        command_count,
        command_script,
    }
}

fn set_pending(fixture: &Fixture, id: &str, pending: Action) {
    let path = fixture
        .shared
        .config
        .paths
        .current_state()
        .join("engineering.json");
    let mut persisted: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let saved = persisted["jobs"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| entry["id"] == id)
        .unwrap();
    saved["phase"] = json!("active");
    let sequence = saved["next_action_sequence"]
        .as_u64()
        .unwrap_or_else(|| saved["metrics"]["actions"].as_u64().unwrap() + 1);
    if saved.get("next_action_sequence").is_some() {
        saved["next_action_sequence"] = json!(sequence.checked_add(1).unwrap());
        saved["metrics"]["decisions"] = json!(saved["metrics"]["decisions"].as_u64().unwrap() + 1);
    } else {
        saved["metrics"]["decisions"] = json!(
            saved["metrics"]["decisions"]
                .as_u64()
                .unwrap()
                .max(sequence)
        );
    }
    saved["pending"] = json!({"sequence": sequence, "action": pending, "started_at": 1});
    std::fs::write(path, serde_json::to_vec_pretty(&persisted).unwrap()).unwrap();
}

fn checkpoint_path(fixture: &Fixture) -> PathBuf {
    fixture
        .shared
        .config
        .paths
        .current_state()
        .join("engineering.json")
}

fn checkpoint(fixture: &Fixture) -> Value {
    serde_json::from_slice(&std::fs::read(checkpoint_path(fixture)).unwrap()).unwrap()
}

fn write_checkpoint(fixture: &Fixture, snapshot: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(snapshot).unwrap();
    bytes.push(b'\n');
    std::fs::write(checkpoint_path(fixture), &bytes).unwrap();
    bytes
}

fn ready_fixture() -> (Fixture, String) {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let model = ScriptedModel::new([fix(), finish()]);
    action(&fixture.shared, &model);
    action(&fixture.shared, &model);
    model.assert_consumed();
    assert_eq!(job(&fixture.shared, &id)["phase"], "ready");
    (fixture, id)
}

fn assert_snapshot_quarantined(fixture: Fixture, snapshot: Value, case: &str) {
    let jobs = fixture.shared.engineering.list();
    let original_id = jobs[0].id.clone();
    let original_head = git(&fixture.workspace, &["rev-parse", "HEAD"]);
    let candidates: Vec<_> = jobs
        .iter()
        .filter(|job| job.candidate.path.exists())
        .map(|job| {
            let path = job.candidate.path.clone();
            let head = git(&path, &["rev-parse", "HEAD"]);
            let content = std::fs::read(path.join("code.txt")).unwrap();
            std::fs::write(path.join("preserved-untracked.txt"), "keep this draft\n").unwrap();
            (path, head, content)
        })
        .collect();
    let validations = count(&fixture.validation_count);
    let commands = count(&fixture.command_count);
    let path = checkpoint_path(&fixture);
    let original_bytes = write_checkpoint(&fixture, &snapshot);
    let fixture = restart(fixture);

    // This assertion precedes every API call. Recovery must not normalize or
    // partly repair a snapshot before all of its jobs have been validated.
    assert!(
        std::fs::read(&path).unwrap() == original_bytes,
        "{case}: load rewrote the rejected snapshot"
    );
    assert!(
        !fixture.shared.engineering.summary()["error"].is_null(),
        "{case}: inconsistent snapshot was accepted"
    );
    let model = ScriptedModel::new([]);
    assert!(
        tick_with(&fixture.shared, &model).is_err(),
        "{case}: tick was accepted"
    );
    assert_eq!(model.calls(), 0, "{case}: quarantine called the model");
    for request in [
        json!({"action":"create","workspace":"fixture","objective":"must be rejected"}),
        json!({"action":"pause","id":original_id}),
        json!({"action":"resume","id":original_id}),
    ] {
        let (_, response) = handle(&fixture.shared, &request);
        assert_eq!(response["ok"], false, "{case}: {response}");
    }
    assert!(
        std::fs::read(&path).unwrap() == original_bytes,
        "{case}: API rewrote the snapshot"
    );
    assert_eq!(count(&fixture.validation_count), validations, "{case}");
    assert_eq!(count(&fixture.command_count), commands, "{case}");
    assert_eq!(
        git(&fixture.workspace, &["rev-parse", "HEAD"]),
        original_head,
        "{case}"
    );
    for (path, head, content) in candidates {
        assert_eq!(git(&path, &["rev-parse", "HEAD"]), head, "{case}");
        assert_eq!(
            std::fs::read(path.join("code.txt")).unwrap(),
            content,
            "{case}"
        );
        assert_eq!(
            std::fs::read_to_string(path.join("preserved-untracked.txt")).unwrap(),
            "keep this draft\n",
            "{case}"
        );
    }
}

fn fix() -> Action {
    Action::Write {
        path: "code.txt".to_owned(),
        content: "fixed\n".to_owned(),
        expected: Some("broken\n".to_owned()),
    }
}

fn finish() -> Action {
    Action::Finish {
        summary: "The change is complete and every check passes".to_owned(),
    }
}

#[test]
fn native_loop_repairs_failed_validation_and_finishes_only_after_a_real_pass() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let base_head = git(&fixture.workspace, &["rev-parse", "HEAD"]);
    let model = ScriptedModel::new([finish(), fix(), finish()]);

    action(&fixture.shared, &model);
    let failed = job(&fixture.shared, &id);
    assert_eq!(
        failed["phase"], "active",
        "a claimed success is not evidence"
    );
    assert_eq!(failed["validation"]["passed"], false);
    assert_eq!(failed["validation"]["checks"][0]["exit_code"], 1);
    assert_eq!(failed["metrics"]["verification_runs"], 1);
    assert_eq!(failed["metrics"]["verification_failures"], 1);
    assert_eq!(count(&fixture.validation_count), 1);

    action(&fixture.shared, &model);
    action(&fixture.shared, &model);
    let completed = job(&fixture.shared, &id);
    assert_eq!(completed["phase"], "ready");
    assert_eq!(completed["validation"]["passed"], true);
    assert_eq!(completed["validation"]["checks"][0]["exit_code"], 0);
    assert_eq!(completed["metrics"]["verification_runs"], 2);
    assert!(completed["metrics"]["repairs"].as_u64().unwrap() >= 1);
    assert_eq!(count(&fixture.validation_count), 2);
    assert_eq!(
        completed["validation"]["head"],
        git(&candidate(&fixture.shared, &id), &["rev-parse", "HEAD"])
    );
    assert_eq!(git(&fixture.workspace, &["rev-parse", "HEAD"]), base_head);
    assert_eq!(
        std::fs::read_to_string(fixture.workspace.join("code.txt")).unwrap(),
        "broken\n",
        "ready does not merge the candidate"
    );
    model.assert_consumed();
    assert_eq!(model.calls(), 3);
    assert!(
        model.contexts.lock().unwrap()[1]
            .to_string()
            .contains("exit_code")
    );
}

#[test]
fn a_rejected_write_does_not_erase_the_pending_repair_metric() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let model = ScriptedModel::new([
        finish(),
        Action::Write {
            path: "code.txt".to_owned(),
            content: "fixed\n".to_owned(),
            expected: Some("stale content\n".to_owned()),
        },
        fix(),
        finish(),
    ]);
    action(&fixture.shared, &model);
    action(&fixture.shared, &model);
    let rejected = job(&fixture.shared, &id);
    assert_eq!(rejected["phase"], "active");
    assert_eq!(
        rejected["observations"].as_array().unwrap().last().unwrap()["ok"],
        false
    );
    assert_eq!(rejected["metrics"]["repairs"], 0);
    assert_eq!(
        std::fs::read_to_string(candidate(&fixture.shared, &id).join("code.txt")).unwrap(),
        "broken\n"
    );
    action(&fixture.shared, &model);
    assert_eq!(
        job(&fixture.shared, &id)["metrics"]["repairs"],
        1,
        "a rejected CAS must not erase the unresolved verification failure"
    );
    action(&fixture.shared, &model);
    assert_eq!(job(&fixture.shared, &id)["phase"], "ready");
    assert_eq!(count(&fixture.validation_count), 2);
    model.assert_consumed();
}

#[test]
fn finish_revalidates_changes_instead_of_reusing_a_pass_for_an_old_head() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let model = ScriptedModel::new([
        fix(),
        Action::Verify,
        Action::Write {
            path: "code.txt".to_owned(),
            content: "regression\n".to_owned(),
            expected: Some("fixed\n".to_owned()),
        },
        finish(),
    ]);
    action(&fixture.shared, &model);
    action(&fixture.shared, &model);
    let verified = job(&fixture.shared, &id);
    assert_eq!(
        verified["phase"], "active",
        "Verify alone does not finish a job"
    );
    assert_eq!(verified["validation"]["passed"], true);
    let verified_head = verified["validation"]["head"].clone();

    action(&fixture.shared, &model);
    action(&fixture.shared, &model);
    let rejected = job(&fixture.shared, &id);
    assert_eq!(rejected["phase"], "active");
    assert_eq!(rejected["validation"]["passed"], false);
    assert_ne!(rejected["validation"]["head"], verified_head);
    assert_eq!(rejected["validation"]["checks"][0]["exit_code"], 1);
    assert_eq!(count(&fixture.validation_count), 2);
    model.assert_consumed();
}

#[test]
fn pause_reload_and_resume_keep_the_same_candidate_and_uncommitted_artifacts() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let original = job(&fixture.shared, &id);
    let path = candidate(&fixture.shared, &id);
    let original_head = git(&path, &["rev-parse", "HEAD"]);

    // This is the operator's independent work, created after isolation.
    std::fs::write(fixture.workspace.join("notes.txt"), "operator draft\n").unwrap();
    std::fs::write(
        fixture.workspace.join("operator-untracked.txt"),
        "keep me\n",
    )
    .unwrap();
    let model = ScriptedModel::new([
        Action::Checkpoint {
            summary: "The implementation still needs verification".to_owned(),
            next_step: "Verify the candidate without discarding the draft".to_owned(),
        },
        fix(),
        Action::Write {
            path: "artifact.txt".to_owned(),
            content: "uncommitted result\n".to_owned(),
            expected: None,
        },
    ]);
    for _ in 0..3 {
        action(&fixture.shared, &model);
    }
    assert!(!git(&path, &["status", "--porcelain"]).is_empty());
    let (_, paused) = handle(&fixture.shared, &json!({"action": "pause", "id": id}));
    assert_eq!(paused["ok"], true, "{paused}");

    // This is a durable-state reload test, not a claim of killing a resident
    // process. No model or command is running while Shared is recreated.
    let fixture = restart(fixture);
    let restored = job(&fixture.shared, &id);
    assert_eq!(restored["phase"], "paused");
    assert_eq!(restored["candidate"], original["candidate"]);
    assert_eq!(git(&path, &["rev-parse", "HEAD"]), original_head);
    assert_eq!(
        std::fs::read_to_string(path.join("code.txt")).unwrap(),
        "fixed\n"
    );
    assert_eq!(
        std::fs::read_to_string(path.join("artifact.txt")).unwrap(),
        "uncommitted result\n"
    );
    assert_eq!(
        restored["checkpoint"]["next_step"],
        "Verify the candidate without discarding the draft"
    );
    let (_, resumed) = handle(&fixture.shared, &json!({"action": "resume", "id": id}));
    assert_eq!(resumed["ok"], true, "{resumed}");
    let resumed_model = ScriptedModel::new([finish()]);
    action(&fixture.shared, &resumed_model);
    assert_eq!(job(&fixture.shared, &id)["phase"], "ready");
    assert_eq!(count(&fixture.validation_count), 1);
    assert_eq!(
        std::fs::read_to_string(fixture.workspace.join("notes.txt")).unwrap(),
        "operator draft\n"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.workspace.join("operator-untracked.txt")).unwrap(),
        "keep me\n"
    );
    assert_eq!(
        std::fs::read_to_string(path.join("artifact.txt")).unwrap(),
        "uncommitted result\n"
    );
    model.assert_consumed();
    resumed_model.assert_consumed();
}

#[test]
fn loading_uncertain_effects_pauses_without_replaying_run_replace_verify_or_finish() {
    for pending_kind in ["run", "replace", "verify", "finish"] {
        let fixture = fixture();
        let id = create(&fixture.shared);
        prepare(&fixture.shared, &id);
        let original_candidate = job(&fixture.shared, &id)["candidate"].clone();
        let pending = match pending_kind {
            "run" => Action::Run {
                argv: vec![
                    "sh".to_owned(),
                    fixture.command_script.to_string_lossy().into_owned(),
                    fixture.command_count.to_string_lossy().into_owned(),
                ],
            },
            "replace" => {
                std::fs::write(
                    candidate(&fixture.shared, &id).join("replaced.txt"),
                    "prefixx",
                )
                .unwrap();
                Action::Replace {
                    path: "replaced.txt".to_owned(),
                    old: "x".to_owned(),
                    new: "prefixx".to_owned(),
                }
            }
            "verify" => Action::Verify,
            "finish" => finish(),
            _ => unreachable!(),
        };
        // Model a persisted intent whose completion receipt was lost. This
        // deliberately tests load/recovery, not operating-system crash timing.
        set_pending(&fixture, &id, pending);
        let fixture = restart(fixture);
        let restored = job(&fixture.shared, &id);
        assert_eq!(restored["phase"], "paused", "{pending_kind}: {restored}");
        assert_eq!(restored["candidate"], original_candidate);
        assert!(restored["pending"].is_null());
        assert!(
            restored["observations"]
                .to_string()
                .contains("outcome_unknown")
        );
        let model = ScriptedModel::new([Action::Checkpoint {
            summary: "An interrupted effect requires a fresh decision".to_owned(),
            next_step: "Inspect the candidate before choosing another operation".to_owned(),
        }]);
        assert!(tick_with(&fixture.shared, &model).unwrap().is_none());
        assert_eq!(model.calls(), 0);
        assert_eq!(count(&fixture.command_count), 0);
        assert_eq!(count(&fixture.validation_count), 0);

        if pending_kind == "replace" {
            assert_eq!(
                std::fs::read_to_string(candidate(&fixture.shared, &id).join("replaced.txt"))
                    .unwrap(),
                "prefixx",
                "recovery must not apply a non-idempotent replacement again"
            );
        }

        let (_, reply) = handle(&fixture.shared, &json!({"action": "resume", "id": id}));
        assert_eq!(reply["ok"], true, "{reply}");
        action(&fixture.shared, &model);
        assert_eq!(model.calls(), 1);
        assert_eq!(count(&fixture.command_count), 0);
        assert_eq!(count(&fixture.validation_count), 0);
        if pending_kind == "replace" {
            assert_eq!(
                std::fs::read_to_string(candidate(&fixture.shared, &id).join("replaced.txt"))
                    .unwrap(),
                "prefixx",
                "resume asks for a new decision rather than replaying the old action"
            );
        }
        model.assert_consumed();
    }
}

#[test]
fn loading_an_interrupted_write_preserves_bytes_and_asks_for_a_fresh_decision() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let path = candidate(&fixture.shared, &id);
    std::fs::write(
        path.join("recovered.txt"),
        "effect completed before receipt\n",
    )
    .unwrap();
    set_pending(
        &fixture,
        &id,
        Action::Write {
            path: "recovered.txt".to_owned(),
            content: "effect completed before receipt\n".to_owned(),
            expected: None,
        },
    );
    let fixture = restart(fixture);
    let restored = job(&fixture.shared, &id);
    assert_eq!(restored["phase"], "active");
    assert!(restored["pending"].is_null());
    assert!(
        restored["observations"]
            .to_string()
            .contains("outcome_unknown")
    );
    let model = ScriptedModel::new([Action::Read {
        path: "recovered.txt".to_owned(),
    }]);
    action(&fixture.shared, &model);
    assert_eq!(model.calls(), 1);
    assert_eq!(
        std::fs::read_to_string(path.join("recovered.txt")).unwrap(),
        "effect completed before receipt\n"
    );
    assert_eq!(count(&fixture.command_count), 0);
    assert_eq!(count(&fixture.validation_count), 0);
    model.assert_consumed();
}

struct BlockingModel {
    entered: Mutex<Option<mpsc::Sender<()>>>,
    released: (Mutex<bool>, Condvar),
    calls: AtomicUsize,
    argv: Vec<String>,
}

impl BlockingModel {
    fn release(&self) {
        *self.released.0.lock().unwrap() = true;
        self.released.1.notify_all();
    }
}

impl EngineerModel for BlockingModel {
    fn decide(&self, _context: &Value) -> Result<Action, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(sender) = self.entered.lock().unwrap().take() {
            sender.send(()).unwrap();
        }
        let released = self.released.0.lock().unwrap();
        drop(
            self.released
                .1
                .wait_timeout_while(released, Duration::from_secs(10), |released| !*released)
                .unwrap(),
        );
        Ok(Action::Run {
            argv: self.argv.clone(),
        })
    }
}

#[test]
fn concurrent_ticks_claim_one_decision_and_start_the_command_once() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let (entered_tx, entered_rx) = mpsc::channel();
    let model = BlockingModel {
        entered: Mutex::new(Some(entered_tx)),
        released: (Mutex::new(false), Condvar::new()),
        calls: AtomicUsize::new(0),
        argv: vec![
            "sh".to_owned(),
            fixture.command_script.to_string_lossy().into_owned(),
            fixture.command_count.to_string_lossy().into_owned(),
        ],
    };
    std::thread::scope(|scope| {
        let first = scope.spawn(|| tick_with(&fixture.shared, &model));
        let entered = entered_rx.recv_timeout(Duration::from_secs(5));
        if entered.is_err() {
            model.release();
        }
        entered.expect("first tick enters the model");
        let (returned_tx, returned_rx) = mpsc::channel();
        let shared = &fixture.shared;
        let model_ref = &model;
        scope.spawn(move || {
            let result = tick_with(shared, model_ref);
            let _ = returned_tx.send(result);
        });
        let second = returned_rx.recv_timeout(Duration::from_secs(5));
        // Always release the first worker before asserting, even when the
        // concurrent tick incorrectly blocks, so a regression cannot hang CI.
        model.release();
        let first = first.join().unwrap().expect("first tick succeeds");
        assert!(first.is_some());
        assert!(
            second
                .expect("concurrent tick skips an acquired lease")
                .unwrap()
                .is_none()
        );
    });
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    assert_eq!(count(&fixture.command_count), 1);
    assert_eq!(job(&fixture.shared, &id)["metrics"]["actions"], 1);
}

#[test]
fn pausing_during_a_model_decision_prevents_its_proposed_command() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let (entered_tx, entered_rx) = mpsc::channel();
    let argv = vec![
        "sh".to_owned(),
        fixture.command_script.to_string_lossy().into_owned(),
        fixture.command_count.to_string_lossy().into_owned(),
    ];
    let model = BlockingModel {
        entered: Mutex::new(Some(entered_tx)),
        released: (Mutex::new(false), Condvar::new()),
        calls: AtomicUsize::new(0),
        argv: argv.clone(),
    };
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| tick_with(&fixture.shared, &model));
        let entered = entered_rx.recv_timeout(Duration::from_secs(5));
        if entered.is_err() {
            model.release();
        }
        entered.expect("model inference is in progress before pause");
        let (_, paused) = handle(&fixture.shared, &json!({"action": "pause", "id": id}));
        model.release();
        let result = worker.join().unwrap().expect("paused tick settles");
        assert_eq!(paused["ok"], true, "{paused}");
        assert!(result.is_some());
    });
    let paused = job(&fixture.shared, &id);
    assert_eq!(paused["phase"], "paused");
    assert!(paused["pending"].is_null());
    assert_eq!(paused["metrics"]["actions"], 0);
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    assert_eq!(count(&fixture.command_count), 0);

    let (_, resumed) = handle(&fixture.shared, &json!({"action": "resume", "id": id}));
    assert_eq!(resumed["ok"], true, "{resumed}");
    let fresh_decision = ScriptedModel::new([Action::Run { argv }]);
    action(&fixture.shared, &fresh_decision);
    assert_eq!(count(&fixture.command_count), 1);
    assert_eq!(job(&fixture.shared, &id)["metrics"]["actions"], 1);
    fresh_decision.assert_consumed();
}

#[test]
fn repeated_create_request_has_one_durable_job_and_one_candidate() {
    let fixture = fixture();
    let request = json!({
        "action": "create", "workspace": "fixture", "objective": "Fix code.txt",
        "request_id": "fixture-create-once"
    });
    let (_, first) = handle(&fixture.shared, &request);
    assert_eq!(first["ok"], true, "{first}");
    let id = first["job"]["id"].as_str().unwrap();
    prepare(&fixture.shared, id);
    let first_candidate = job(&fixture.shared, id)["candidate"].clone();
    let fixture = restart(fixture);
    let (status, duplicate) = handle(&fixture.shared, &request);
    assert!((200..300).contains(&status), "{duplicate}");
    assert_eq!(duplicate["job"]["id"], id);
    assert_eq!(duplicate["job"]["candidate"], first_candidate);
    let (_, listed) = handle(&fixture.shared, &json!({"action": "list"}));
    assert_eq!(listed["jobs"].as_array().unwrap().len(), 1);
}

#[test]
fn an_unwritable_action_checkpoint_prevents_the_command_from_starting() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let path = fixture
        .shared
        .config
        .paths
        .current_state()
        .join("engineering.json");
    let before = std::fs::read(&path).unwrap();
    let preserved = fixture
        .dir
        .path()
        .join("checkpoint-before-disk-failure.json");
    std::fs::rename(&path, &preserved).unwrap();
    // Replacing the target with a directory fails even when CI runs as root;
    // filesystem permission bits would not reliably simulate a failed write.
    std::fs::create_dir(&path).unwrap();
    let model = ScriptedModel::new([Action::Run {
        argv: vec![
            "sh".to_owned(),
            fixture.command_script.to_string_lossy().into_owned(),
            fixture.command_count.to_string_lossy().into_owned(),
        ],
    }]);
    assert!(tick_with(&fixture.shared, &model).is_err());
    assert_eq!(count(&fixture.command_count), 0);
    assert_eq!(job(&fixture.shared, &id)["metrics"]["actions"], 0);
    assert!(job(&fixture.shared, &id)["pending"].is_null());
    assert_eq!(std::fs::read(preserved).unwrap(), before);
    model.assert_consumed();
}

#[test]
fn a_lost_command_receipt_is_quarantined_and_not_replayed_after_reload() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let checkpoint = fixture
        .shared
        .config
        .paths
        .current_state()
        .join("engineering.json");
    let last_durable = fixture
        .dir
        .path()
        .join("checkpoint-before-lost-receipt.json");
    let script = fixture.dir.path().join("lose-command-receipt.sh");
    std::fs::write(
        &script,
        "#!/bin/sh\nset -eu\nprintf 'run\\n' >> \"$1\"\nprintf 'command result\\n' > result.txt\nmv \"$2\" \"$3\"\nmkdir \"$2\"\n",
    )
    .unwrap();
    let argv = vec![
        "sh".to_owned(),
        script.to_string_lossy().into_owned(),
        fixture.command_count.to_string_lossy().into_owned(),
        checkpoint.to_string_lossy().into_owned(),
        last_durable.to_string_lossy().into_owned(),
    ];
    let model = ScriptedModel::new([Action::Run { argv: argv.clone() }]);
    assert!(tick_with(&fixture.shared, &model).is_err());
    model.assert_consumed();
    assert_eq!(count(&fixture.command_count), 1);
    let uncertain = job(&fixture.shared, &id);
    assert_eq!(uncertain["metrics"]["actions"], 0);
    assert_eq!(uncertain["pending"]["action"]["action"], "run");
    let durable: Value = serde_json::from_slice(&std::fs::read(&last_durable).unwrap()).unwrap();
    assert_eq!(durable["jobs"][0]["pending"]["action"]["action"], "run");

    let retry = ScriptedModel::new([Action::Run { argv }]);
    assert!(tick_with(&fixture.shared, &retry).is_err());
    assert_eq!(
        count(&fixture.command_count),
        1,
        "quarantine prevents another effect"
    );

    // Restore the last durable intent to model storage recovery. The command
    // really ran; only its outcome receipt could not be saved.
    std::fs::remove_dir(&checkpoint).unwrap();
    std::fs::rename(&last_durable, &checkpoint).unwrap();
    let fixture = restart(fixture);
    let restored = job(&fixture.shared, &id);
    assert_eq!(restored["phase"], "paused");
    assert_eq!(restored["metrics"]["interruptions"], 1);
    assert!(restored["pending"].is_null());
    assert!(
        restored["observations"]
            .to_string()
            .contains("outcome_unknown")
    );
    let no_replay = ScriptedModel::new([]);
    assert!(tick_with(&fixture.shared, &no_replay).unwrap().is_none());
    assert_eq!(no_replay.calls(), 0);
    assert_eq!(count(&fixture.command_count), 1);
    assert_eq!(
        std::fs::read_to_string(candidate(&fixture.shared, &id).join("result.txt")).unwrap(),
        "command result\n"
    );
}

#[test]
fn a_corrupt_checkpoint_is_quarantined_and_never_replaced_with_an_empty_store() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let candidate_path = candidate(&fixture.shared, &id);
    std::fs::write(candidate_path.join("preserved.txt"), "unsaved result\n").unwrap();
    let path = fixture
        .shared
        .config
        .paths
        .current_state()
        .join("engineering.json");
    let corrupt = b"{\"schema_version\":1,\"jobs\":[";
    std::fs::write(&path, corrupt).unwrap();
    let fixture = restart(fixture);
    let (_, reply) = handle(
        &fixture.shared,
        &json!({"action": "create", "workspace": "fixture", "objective": "new work"}),
    );
    assert_eq!(reply["ok"], false, "{reply}");
    let model = ScriptedModel::new([]);
    assert!(tick_with(&fixture.shared, &model).is_err());
    assert_eq!(model.calls(), 0);
    assert_eq!(std::fs::read(&path).unwrap(), corrupt);
    assert_eq!(
        std::fs::read_to_string(candidate_path.join("preserved.txt")).unwrap(),
        "unsaved result\n"
    );
    let (_, status) = handle(&fixture.shared, &json!({"action": "list"}));
    assert!(!status["status"]["error"].is_null());
}

#[test]
fn semantic_checkpoint_rejects_ready_without_consistent_verification_evidence() {
    for case in [
        "missing_validation",
        "failed_validation",
        "empty_checks",
        "failed_check",
        "nonzero_exit",
        "invalid_head",
        "no_verification",
        "pending_repair",
    ] {
        let (fixture, _) = ready_fixture();
        let mut snapshot = checkpoint(&fixture);
        let saved = &mut snapshot["jobs"][0];
        match case {
            "missing_validation" => saved["validation"] = Value::Null,
            "failed_validation" => saved["validation"]["passed"] = json!(false),
            "empty_checks" => saved["validation"]["checks"] = json!([]),
            "failed_check" => saved["validation"]["checks"][0]["passed"] = json!(false),
            "nonzero_exit" => saved["validation"]["checks"][0]["exit_code"] = json!(1),
            "invalid_head" => saved["validation"]["head"] = json!("claimed-success"),
            "no_verification" => saved["metrics"]["verification_runs"] = json!(0),
            "pending_repair" => saved["repair_pending"] = json!(true),
            _ => unreachable!(),
        }
        assert_snapshot_quarantined(fixture, snapshot, case);
    }
}

#[test]
fn semantic_checkpoint_rejects_duplicate_job_ids_and_request_ids() {
    for case in ["duplicate_job_id", "duplicate_request_id"] {
        let fixture = fixture();
        let first = create(&fixture.shared);
        prepare(&fixture.shared, &first);
        let second = create(&fixture.shared);
        assert_ne!(first, second);
        let mut snapshot = checkpoint(&fixture);
        if case == "duplicate_job_id" {
            snapshot["jobs"][1] = snapshot["jobs"][0].clone();
        } else {
            for saved in snapshot["jobs"].as_array_mut().unwrap() {
                saved["request_id"] = json!("duplicated-request");
            }
        }
        assert_snapshot_quarantined(fixture, snapshot, case);
    }
}

#[test]
fn semantic_checkpoint_rejects_inconsistent_pending_sequences_and_metrics() {
    for case in [
        "past_sequence",
        "future_sequence",
        "overflowing_sequence",
        "missing_decision",
        "pending_preparing",
        "pending_ready",
        "actions_without_decisions",
        "checks_without_actions",
        "failures_without_checks",
    ] {
        let (fixture, id) = if case == "pending_ready" {
            ready_fixture()
        } else {
            let fixture = fixture();
            let id = create(&fixture.shared);
            prepare(&fixture.shared, &id);
            (fixture, id)
        };
        set_pending(
            &fixture,
            &id,
            Action::Read {
                path: "code.txt".to_owned(),
            },
        );
        let mut snapshot = checkpoint(&fixture);
        let saved = &mut snapshot["jobs"][0];
        match case {
            "past_sequence" => saved["pending"]["sequence"] = json!(0),
            "future_sequence" => {
                saved["pending"]["sequence"] = json!(2);
                saved["metrics"]["decisions"] = json!(2);
            }
            "overflowing_sequence" => {
                saved["metrics"]["actions"] = json!(u64::MAX);
                saved["metrics"]["decisions"] = json!(u64::MAX);
                saved["pending"]["sequence"] = json!(0);
            }
            "missing_decision" => saved["metrics"]["decisions"] = json!(0),
            "pending_preparing" => saved["phase"] = json!("preparing"),
            "pending_ready" => saved["phase"] = json!("ready"),
            "actions_without_decisions" => {
                saved["pending"] = Value::Null;
                saved["metrics"]["actions"] = json!(2);
            }
            "checks_without_actions" => {
                saved["pending"] = Value::Null;
                saved["metrics"]["verification_runs"] = json!(1);
            }
            "failures_without_checks" => {
                saved["pending"] = Value::Null;
                saved["metrics"]["verification_failures"] = json!(1);
            }
            _ => unreachable!(),
        }
        assert_snapshot_quarantined(fixture, snapshot, case);
    }
}

#[test]
fn semantic_checkpoint_rejects_mismatched_candidate_identity_and_root() {
    for case in [
        "job_id",
        "branch",
        "leaf",
        "workspace",
        "root",
        "relative",
        "parent_reference",
        "base_commit",
    ] {
        let fixture = fixture();
        let id = create(&fixture.shared);
        prepare(&fixture.shared, &id);
        let mut snapshot = checkpoint(&fixture);
        let saved = &mut snapshot["jobs"][0];
        let candidate_path = PathBuf::from(saved["candidate"]["path"].as_str().unwrap());
        match case {
            "job_id" => saved["id"] = json!("eng-different-id"),
            "branch" => {
                saved["candidate"]["branch"] = json!("kamimusuhi/engineering/eng-different-id")
            }
            "leaf" => {
                saved["candidate"]["path"] =
                    json!(candidate_path.with_file_name("eng-different-id"))
            }
            "workspace" => saved["workspace"] = json!("different-workspace"),
            "root" => {
                saved["candidate"]["path"] = json!(
                    fixture
                        .dir
                        .path()
                        .join("other-root")
                        .join("fixture")
                        .join(&id)
                )
            }
            "relative" => saved["candidate"]["path"] = json!(Path::new("fixture").join(&id)),
            "parent_reference" => {
                saved["candidate"]["path"] = json!(
                    candidate_path
                        .parent()
                        .unwrap()
                        .join("..")
                        .join("fixture")
                        .join(&id)
                )
            }
            "base_commit" => saved["candidate"]["base_commit"] = json!("not-a-commit"),
            _ => unreachable!(),
        }
        assert_snapshot_quarantined(fixture, snapshot, case);
    }
}

#[test]
fn semantic_checkpoint_validates_every_job_before_recovering_any_pending_action() {
    let fixture = fixture();
    let first = create(&fixture.shared);
    prepare(&fixture.shared, &first);
    let second = create(&fixture.shared);
    assert_ne!(first, second);
    set_pending(
        &fixture,
        &first,
        Action::Run {
            argv: vec![
                "sh".to_owned(),
                fixture.command_script.to_string_lossy().into_owned(),
                fixture.command_count.to_string_lossy().into_owned(),
            ],
        },
    );
    let mut snapshot = checkpoint(&fixture);
    snapshot["jobs"][1]["phase"] = json!("ready");
    assert_snapshot_quarantined(fixture, snapshot, "valid pending followed by invalid Ready");
}

#[test]
fn semantic_checkpoint_accepts_ready_and_multiple_absent_request_ids() {
    let (fixture, ready) = ready_fixture();
    let preparing = create(&fixture.shared);
    assert_ne!(ready, preparing);
    let mut snapshot = checkpoint(&fixture);
    for saved in snapshot["jobs"].as_array_mut().unwrap() {
        // Both fields have defaults for checkpoints written before their
        // introduction; missing optional metadata is not a duplicate key.
        let saved = saved.as_object_mut().unwrap();
        saved.remove("request_id");
        saved.remove("repair_pending");
    }
    let bytes = write_checkpoint(&fixture, &snapshot);
    let fixture = restart(fixture);
    assert!(fixture.shared.engineering.summary()["error"].is_null());
    assert_eq!(fixture.shared.engineering.list().len(), 2);
    let restored = job(&fixture.shared, &ready);
    assert_eq!(restored["phase"], "ready");
    assert_eq!(restored["validation"]["passed"], true);
    assert_eq!(restored["repair_pending"], false);
    assert!(restored["request_id"].is_null());
    assert_eq!(job(&fixture.shared, &preparing)["phase"], "preparing");
    assert_eq!(std::fs::read(checkpoint_path(&fixture)).unwrap(), bytes);
    assert_eq!(count(&fixture.validation_count), 1);
    assert_eq!(count(&fixture.command_count), 0);
}

#[test]
fn semantic_checkpoint_accepts_valid_active_and_paused_pending_actions() {
    for phase in ["active", "paused"] {
        let fixture = fixture();
        let id = create(&fixture.shared);
        prepare(&fixture.shared, &id);
        let original_candidate = job(&fixture.shared, &id)["candidate"].clone();
        set_pending(
            &fixture,
            &id,
            Action::Read {
                path: "code.txt".to_owned(),
            },
        );
        let mut snapshot = checkpoint(&fixture);
        snapshot["jobs"][0]["phase"] = json!(phase);
        write_checkpoint(&fixture, &snapshot);
        let fixture = restart(fixture);
        assert!(
            fixture.shared.engineering.summary()["error"].is_null(),
            "{phase}"
        );
        let restored = job(&fixture.shared, &id);
        assert_eq!(restored["phase"], phase);
        assert_eq!(restored["candidate"], original_candidate);
        assert_eq!(restored["metrics"]["interruptions"], 1);
        assert!(restored["pending"].is_null());
        assert!(
            restored["observations"]
                .to_string()
                .contains("outcome_unknown")
        );
        let model = ScriptedModel::new(if phase == "active" {
            vec![Action::Read {
                path: "code.txt".to_owned(),
            }]
        } else {
            vec![]
        });
        let result = tick_with(&fixture.shared, &model).unwrap();
        assert_eq!(result.is_some(), phase == "active");
        assert_eq!(model.calls(), usize::from(phase == "active"));
        model.assert_consumed();
        assert_eq!(count(&fixture.validation_count), 0);
        assert_eq!(count(&fixture.command_count), 0);
    }
}

#[test]
fn long_finish_and_resume_notes_fit_the_persisted_working_memory_limits() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let summary = "修正した内容と実検証の結果。".repeat(200);
    assert!(summary.len() > 4096);
    let model = ScriptedModel::new([fix(), Action::Finish { summary }]);
    action(&fixture.shared, &model);
    let finished = tick_with(&fixture.shared, &model);
    // The independent command must actually run before the receipt is
    // committed; a long UTF-8 note must not strand its successful outcome.
    assert_eq!(count(&fixture.validation_count), 1);
    assert!(
        finished.is_ok(),
        "successful verification receipt was not saved: {finished:?}"
    );
    let completed = job(&fixture.shared, &id);
    assert_eq!(completed["phase"], "ready");
    assert_eq!(completed["validation"]["checks"][0]["exit_code"], 0);
    assert_eq!(completed["validation"]["passed"], true);
    assert_eq!(completed["metrics"]["actions"], 2);
    let saved_summary = completed["checkpoint"]["summary"].as_str().unwrap();
    assert!(saved_summary.starts_with("修正した内容"));
    assert!(
        saved_summary.len() <= 4096,
        "summary exceeded its encoded byte budget"
    );
    assert_eq!(checkpoint(&fixture)["jobs"][0]["phase"], "ready");
    model.assert_consumed();

    let fixture = restart(fixture);
    assert_eq!(job(&fixture.shared, &id)["phase"], "ready");
    let note = "次に確認する観測と再開手順。".repeat(100);
    assert!(note.len() > 2048);
    let (_, resumed) = handle(
        &fixture.shared,
        &json!({"action":"resume","id":id,"note":note}),
    );
    assert_eq!(resumed["ok"], true, "{resumed}");
    let saved_note = resumed["job"]["checkpoint"]["next_step"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(saved_note.starts_with("次に確認する"));
    assert!(
        saved_note.len() <= 2048,
        "resume note exceeded its encoded byte budget"
    );
    let fixture = restart(fixture);
    let restored = job(&fixture.shared, &id);
    assert_eq!(restored["phase"], "active");
    assert_eq!(restored["checkpoint"]["next_step"], saved_note);
    assert_eq!(
        count(&fixture.validation_count),
        1,
        "reload does not rerun verification"
    );
}

#[test]
fn native_loop_repairs_a_large_rust_file_from_a_complete_range_observation() {
    assert!(
        kamimusuhi_runtime::agent_exec::process::resolve_binary("rustc").is_some(),
        "the real compiler must be available on the test PATH"
    );
    let mut fixture = fixture();
    let mut original: String = (0..6000)
        .map(|line| format!("// 文脈 {line:04}: source material that must remain unchanged\n"))
        .collect();
    let target_line = original.lines().count() + 1;
    let target = "pub fn answer() -> u32 { unknown_repair_symbol }\n";
    original.push_str(target);
    for line in 0..200 {
        original.push_str(&format!(
            "// 後続 {line:04}: retain this complete line when paging\n"
        ));
    }
    assert!(original.len() > 128 * 1024);
    assert_eq!(original.matches("unknown_repair_symbol").count(), 1);
    std::fs::write(fixture.workspace.join("large.rs"), &original).unwrap();
    git(&fixture.workspace, &["add", "large.rs"]);
    git(
        &fixture.workspace,
        &[
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@local",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "add large Rust fixture",
        ],
    );
    let base_head = git(&fixture.workspace, &["rev-parse", "HEAD"]);
    let metadata = fixture.dir.path().join("large.rmeta");
    let policy = fixture.shared.config.engineering.as_mut().unwrap();
    policy.validation_timeout_secs = 30;
    policy.workspaces[0].checks = vec![vec![
        "rustc".to_owned(),
        "--crate-type=lib".to_owned(),
        "--emit=metadata".to_owned(),
        "-o".to_owned(),
        metadata.to_string_lossy().into_owned(),
        "large.rs".to_owned(),
    ]];
    fixture.shared.config.validate().unwrap();
    let (_, created) = handle(
        &fixture.shared,
        &json!({
            "action":"create", "workspace":"fixture",
            "objective":"Fix the undefined unknown_repair_symbol in large.rs so the Rust library type-checks, preserving all other source text."
        }),
    );
    assert_eq!(created["ok"], true, "{created}");
    let id = created["job"]["id"].as_str().unwrap().to_owned();
    prepare(&fixture.shared, &id);
    let identity = job(&fixture.shared, &id)["candidate"].clone();
    let candidate_path = candidate(&fixture.shared, &id);

    // Compare both read paths against the same file, operation budget and
    // candidate. The old whole-file Read cannot expose the repair location.
    let baseline = ScriptedModel::new([Action::Read {
        path: "large.rs".to_owned(),
    }]);
    action(&fixture.shared, &baseline);
    let baseline_job = job(&fixture.shared, &id);
    let whole = &baseline_job["observations"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()["detail"];
    assert_eq!(whole["truncated"], true);
    assert_eq!(whole["bytes"], original.len());
    assert!(
        !whole["content"]
            .as_str()
            .unwrap()
            .contains("unknown_repair_symbol")
    );
    baseline.assert_consumed();

    let model = ScriptedModel::new([
        finish(),
        Action::ReadRange {
            path: "large.rs".to_owned(),
            start_line: target_line,
            max_lines: 200,
        },
        Action::Replace {
            path: "large.rs".to_owned(),
            old: "unknown_repair_symbol".to_owned(),
            new: "42".to_owned(),
        },
        finish(),
    ]);
    action(&fixture.shared, &model);
    let failed = job(&fixture.shared, &id);
    assert_eq!(failed["phase"], "active");
    assert_eq!(failed["validation"]["passed"], false);
    assert_eq!(failed["validation"]["checks"][0]["program"], "rustc");
    assert_eq!(failed["validation"]["checks"][0]["exit_code"], 1);
    assert!(
        failed["validation"]["checks"][0]["stderr"]
            .as_str()
            .unwrap()
            .contains("E0425")
    );
    action(&fixture.shared, &model);
    action(&fixture.shared, &model);

    // This is the exact context supplied to the Replace decision, not merely
    // a workspace-unit return value. Its latest observation remains valid
    // JSON, including complete source lines and the continuation cursor.
    let replace_context = model.contexts.lock().unwrap()[2].clone();
    let latest = replace_context["observations"]
        .as_array()
        .unwrap()
        .last()
        .unwrap();
    assert_eq!(latest["action"], "read_range");
    assert_eq!(latest["context_truncated"], false);
    let range: Value = serde_json::from_str(latest["external_data"].as_str().unwrap())
        .expect("read_range data must survive the native context boundary as complete JSON");
    assert_eq!(range["path"], "large.rs");
    assert_eq!(range["start_line"], target_line);
    assert_eq!(range["total_lines"], original.lines().count());
    assert!(range["content"].as_str().unwrap().starts_with(target));
    assert!(range["content"].as_str().unwrap().ends_with('\n'));
    let next_line = range
        .get("next_line")
        .and_then(Value::as_u64)
        .expect("continuation cursor");
    assert_eq!(range["end_line"].as_u64().unwrap() + 1, next_line);
    assert!(next_line > target_line as u64);
    assert!(next_line <= original.lines().count() as u64);
    assert_eq!(range["truncated"], true);
    assert_eq!(job(&fixture.shared, &id)["metrics"]["repairs"], 1);

    action(&fixture.shared, &model);
    let completed = job(&fixture.shared, &id);
    assert_eq!(completed["phase"], "ready");
    assert_eq!(completed["validation"]["passed"], true);
    assert_eq!(completed["validation"]["checks"][0]["exit_code"], 0);
    assert_eq!(completed["metrics"]["verification_runs"], 2);
    assert_eq!(completed["metrics"]["verification_failures"], 1);
    assert_eq!(completed["metrics"]["repairs"], 1);
    assert_eq!(completed["candidate"], identity);
    assert_eq!(
        completed["validation"]["head"],
        git(&candidate_path, &["rev-parse", "HEAD"])
    );
    assert_eq!(git(&fixture.workspace, &["rev-parse", "HEAD"]), base_head);
    assert_eq!(
        std::fs::read_to_string(fixture.workspace.join("large.rs")).unwrap(),
        original
    );
    assert_eq!(
        std::fs::read_to_string(candidate_path.join("large.rs")).unwrap(),
        original.replacen("unknown_repair_symbol", "42", 1)
    );
    assert_eq!(
        git(
            &candidate_path,
            &["diff", "--name-only", &base_head, "HEAD"]
        ),
        "large.rs"
    );
    assert!(
        metadata.is_file(),
        "the compiler writes metadata outside the candidate"
    );
    assert!(git(&candidate_path, &["status", "--porcelain"]).is_empty());
    model.assert_consumed();
    assert_eq!(model.calls(), 4);
    eprintln!(
        "native large Rust fixture: {} bytes, whole read truncated, range JSON {} bytes, next_line {}, rustc 1 -> 0, repairs 1, validations 2",
        original.len(),
        serde_json::to_vec(&range).unwrap().len(),
        next_line,
    );
}

/// A worker-dispatched claim introduces the genesis ledger and the
/// checkpoint marker together; a lost ledger then stops the job instead of
/// silently recreating an empty one.
#[test]
fn a_worker_claim_introduces_the_ledger_and_a_lost_ledger_stops_the_job() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let model = ScriptedModel::new([fix(), Action::Verify]);
    action(&fixture.shared, &model);
    assert_eq!(
        job(&fixture.shared, &id)["execution_ledger_version"],
        Value::Null
    );

    action(&fixture.shared, &model);
    let after_verify = job(&fixture.shared, &id);
    assert_eq!(after_verify["execution_ledger_version"], 1);
    assert_eq!(after_verify["validation"]["passed"], true);
    assert_eq!(count(&fixture.validation_count), 1);
    let executions = checkpoint_path(&fixture)
        .parent()
        .unwrap()
        .join("engineering.json.executions");
    let ledger_path = executions.join(format!("{id}.json"));
    let ledger: Value = serde_json::from_slice(&std::fs::read(&ledger_path).unwrap()).unwrap();
    assert_eq!(ledger["schema_version"], 1);
    assert_eq!(ledger["job_id"], id);
    assert_eq!(ledger["high_water"], 2);
    assert!(
        ledger["request_digest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    assert!(executions.join(format!("{id}.lock")).is_file());

    // The ledger disappears between ticks: the next worker-targeted claim
    // must stop, never repair the loss into an empty reservation.
    std::fs::remove_file(&ledger_path).unwrap();
    let model = ScriptedModel::new([Action::Run {
        argv: vec![
            "sh".into(),
            fixture.command_script.to_string_lossy().into_owned(),
            fixture.command_count.to_string_lossy().into_owned(),
        ],
    }]);
    action(&fixture.shared, &model);
    let stopped = job(&fixture.shared, &id);
    assert_eq!(stopped["phase"], "paused");
    assert!(stopped["pending"].is_null());
    assert_eq!(
        stopped["last_error"],
        "engineering execution ledger is missing"
    );
    assert_eq!(
        stopped["metrics"]["actions"],
        after_verify["metrics"]["actions"]
    );
    assert_eq!(count(&fixture.command_count), 0, "no side effect started");
    assert!(!ledger_path.exists(), "no empty ledger was recreated");

    // A corrupt ledger stops it identically.
    std::fs::write(&ledger_path, b"not a ledger").unwrap();
    let model = ScriptedModel::new([Action::Run {
        argv: vec![
            "sh".into(),
            fixture.command_script.to_string_lossy().into_owned(),
            fixture.command_count.to_string_lossy().into_owned(),
        ],
    }]);
    assert!(tick_with(&fixture.shared, &model).is_ok());
    let stopped = job(&fixture.shared, &id);
    assert_eq!(stopped["phase"], "paused");
    assert_eq!(std::fs::read(&ledger_path).unwrap(), b"not a ledger");
}

/// A positive reservation without the contract marker — an old checkpoint
/// that somehow gained a ledger — stops the claim instead of adopting it.
#[test]
fn a_positive_ledger_without_a_marker_stops_the_claim() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let persisted = job(&fixture.shared, &id);
    let candidate: Candidate = serde_json::from_value(persisted["candidate"].clone()).unwrap();
    let namespace = super::execution::namespace(&checkpoint_path(&fixture)).unwrap();
    std::fs::create_dir(&namespace.dir).unwrap();
    super::execution::publish(
        &namespace,
        &id,
        &super::execution::Ledger {
            schema_version: 1,
            job_id: id.clone(),
            candidate_digest: super::execution::candidate_digest(&candidate),
            high_water: 1,
            request_digest: Some(format!("sha256:{}", "a".repeat(64))),
        },
    )
    .unwrap();

    let model = ScriptedModel::new([Action::Run {
        argv: vec![
            "sh".into(),
            fixture.command_script.to_string_lossy().into_owned(),
            fixture.command_count.to_string_lossy().into_owned(),
        ],
    }]);
    action(&fixture.shared, &model);
    let stopped = job(&fixture.shared, &id);
    assert_eq!(stopped["phase"], "paused");
    assert!(stopped["pending"].is_null());
    assert_eq!(
        stopped["last_error"],
        "engineering execution ledger exists without its checkpoint marker"
    );
    assert_eq!(count(&fixture.command_count), 0);
    // The ledger content is preserved untouched for the operator.
    assert_eq!(stopped["execution_ledger_version"], Value::Null);
}

/// While another owner holds the job's execution lock, claim creation is
/// deferred to a later tick without any durable state change — and the job
/// resumes claiming once the holder is gone.
#[test]
fn a_busy_job_lock_defers_the_claim_without_a_durable_change() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let namespace = super::execution::namespace(&checkpoint_path(&fixture)).unwrap();
    let held = super::execution::try_lock(&namespace, &id, super::lease::Mode::Create).unwrap();

    let run = || Action::Run {
        argv: vec![
            "sh".into(),
            fixture.command_script.to_string_lossy().into_owned(),
            fixture.command_count.to_string_lossy().into_owned(),
        ],
    };
    // Inference runs before the lock check, so the deferred tick consumes
    // one scripted decision; nothing durable changes.
    let model = ScriptedModel::new([run(), run()]);
    let result = tick_with(&fixture.shared, &model).expect("deferred tick");
    assert!(
        result.is_none_or(|job| job.id != id || job.pending.is_none()),
        "a busy job lock never spends a durable claim"
    );
    let unchanged = job(&fixture.shared, &id);
    assert_eq!(unchanged["phase"], "active");
    assert!(unchanged["pending"].is_null());
    assert_eq!(unchanged["execution_ledger_version"], Value::Null);
    assert_eq!(unchanged["metrics"]["decisions"], 0);
    assert_eq!(count(&fixture.command_count), 0);
    assert!(!namespace.dir.join(format!("{id}.json")).exists());

    drop(held);
    action(&fixture.shared, &model);
    let claimed = job(&fixture.shared, &id);
    assert_eq!(claimed["metrics"]["actions"], 1);
    assert_eq!(claimed["execution_ledger_version"], 1);
    assert_eq!(count(&fixture.command_count), 1);
}

/// `resume` participates in the same job lock as workers and claims: a
/// held lock reports bounded contention without touching durable state.
#[test]
fn resume_reports_a_busy_job_lock_and_leaves_state_untouched() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let (_, paused) = handle(&fixture.shared, &json!({"action": "pause", "id": id}));
    assert_eq!(paused["job"]["phase"], "paused");

    let namespace = super::execution::namespace(&checkpoint_path(&fixture)).unwrap();
    let held = super::execution::try_lock(&namespace, &id, super::lease::Mode::Create).unwrap();
    let (status, refused) = handle(&fixture.shared, &json!({"action": "resume", "id": id}));
    assert_eq!(status, 400);
    assert_eq!(refused["ok"], false);
    assert!(
        refused["error"].as_str().unwrap().contains("executing"),
        "{refused}"
    );
    assert_eq!(job(&fixture.shared, &id)["phase"], "paused");

    drop(held);
    let (status, resumed) = handle(&fixture.shared, &json!({"action": "resume", "id": id}));
    assert_eq!(status, 200);
    assert_eq!(resumed["job"]["phase"], "active");
}

/// Two jobs keep disjoint ledgers and locks under one checkpoint; claiming
/// both advances each independently.
#[test]
fn independent_jobs_hold_disjoint_ledgers_and_locks() {
    let fixture = fixture();
    let first = create(&fixture.shared);
    prepare(&fixture.shared, &first);
    // Pause the first job so its eligibility cannot consume the second
    // job's preparation tick.
    handle(&fixture.shared, &json!({"action": "pause", "id": first}));
    let second = create(&fixture.shared);
    prepare(&fixture.shared, &second);
    handle(&fixture.shared, &json!({"action": "resume", "id": first}));
    let command = || Action::Run {
        argv: vec![
            "sh".into(),
            fixture.command_script.to_string_lossy().into_owned(),
            fixture.command_count.to_string_lossy().into_owned(),
        ],
    };
    let model = ScriptedModel::new([command(), command()]);
    action(&fixture.shared, &model);
    action(&fixture.shared, &model);
    model.assert_consumed();
    assert_eq!(count(&fixture.command_count), 2);
    let executions = checkpoint_path(&fixture)
        .parent()
        .unwrap()
        .join("engineering.json.executions");
    for id in [&first, &second] {
        let ledger: Value =
            serde_json::from_slice(&std::fs::read(executions.join(format!("{id}.json"))).unwrap())
                .unwrap();
        assert_eq!(ledger["high_water"], 1);
        assert_eq!(job(&fixture.shared, id)["metrics"]["actions"], 1);
        assert_eq!(job(&fixture.shared, id)["execution_ledger_version"], 1);
    }
}

fn inspect(shared: &Shared, id: &str) -> Value {
    let model = ScriptedModel::new([Action::Inspect]);
    action(shared, &model);
    model.assert_consumed();
    let observations = job(shared, id)["observations"].as_array().unwrap().clone();
    let detail = observations.last().unwrap()["detail"].clone();
    assert_eq!(detail["kind"], "inspect", "{detail}");
    detail
}

fn verified_fixture() -> (Fixture, String) {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    // A passing Verify records validation evidence but keeps the job
    // Active, which is where an explicit re-inspection belongs.
    let model = ScriptedModel::new([fix(), Action::Verify]);
    action(&fixture.shared, &model);
    action(&fixture.shared, &model);
    model.assert_consumed();
    assert_eq!(job(&fixture.shared, &id)["phase"], "active");
    assert_eq!(job(&fixture.shared, &id)["validation"]["passed"], true);
    (fixture, id)
}

#[test]
fn inspect_compares_the_recorded_verification_with_the_live_candidate() {
    let (fixture, id) = verified_fixture();
    let recorded = job(&fixture.shared, &id)["validation"]["head"]
        .as_str()
        .unwrap()
        .to_owned();
    let detail = inspect(&fixture.shared, &id);
    assert_eq!(detail["candidate"]["state"], "ok");
    assert_eq!(detail["candidate"]["branch_matches"], true);
    assert_eq!(detail["head"]["state"], "match");
    assert_eq!(detail["head"]["current"], recorded);
    assert_eq!(detail["verification"]["recorded"], true);
    assert_eq!(detail["verification"]["passed"], true);
    assert_eq!(detail["uncommitted"]["count"], 0);

    // An uncommitted edit changes the observation without touching the
    // recorded commit; the verification still matches its bound HEAD.
    let root = candidate(&fixture.shared, &id);
    std::fs::write(root.join("notes.txt"), "edited but uncommitted\n").unwrap();
    let detail = inspect(&fixture.shared, &id);
    assert_eq!(detail["head"]["state"], "match");
    assert_eq!(detail["uncommitted"]["count"], 1);
    assert_eq!(detail["uncommitted"]["sample"], json!(["notes.txt"]));

    // A new candidate commit moves HEAD off the recorded verification.
    git(&root, &["add", "notes.txt"]);
    git(
        &root,
        &[
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@local",
            "commit",
            "-qm",
            "candidate edit",
        ],
    );
    let detail = inspect(&fixture.shared, &id);
    assert_eq!(detail["head"]["state"], "changed");
    assert_ne!(detail["head"]["current"], recorded);
}

#[test]
fn inspect_reports_a_missing_candidate_as_data() {
    let (fixture, id) = verified_fixture();
    std::fs::remove_dir_all(candidate(&fixture.shared, &id)).unwrap();
    let detail = inspect(&fixture.shared, &id);
    assert_eq!(detail["candidate"]["exists"], false);
    assert_eq!(detail["candidate"]["state"], "missing");
    assert_eq!(detail["verification"]["recorded"], true);
    assert!(detail.get("head").is_none(), "{detail}");
    assert_eq!(job(&fixture.shared, &id)["phase"], "active");
}

/// Two independent checks measure two distinct failure causes on separate
/// external counters, across several files and a mid-repair restart.
fn fixture_two_checks() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    git(&workspace, &["init", "-q"]);
    std::fs::write(workspace.join("code.txt"), "broken\n").unwrap();
    std::fs::write(workspace.join("notes.txt"), "original notes\n").unwrap();
    git(&workspace, &["add", "."]);
    git(
        &workspace,
        &[
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@local",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "initial",
        ],
    );

    let validation_count = dir.path().join("validation-count");
    let validator = dir.path().join("validate.sh");
    std::fs::write(
        &validator,
        "#!/bin/sh\nset -eu\nprintf 'check\\n' >> \"$1\"\ntest \"$(cat code.txt)\" = fixed\n",
    )
    .unwrap();
    let notes_count = dir.path().join("notes-count");
    let notes_validator = dir.path().join("validate-notes.sh");
    std::fs::write(
        &notes_validator,
        "#!/bin/sh\nset -eu\nprintf 'notes\\n' >> \"$1\"\ntest \"$(cat notes.txt)\" = done\n",
    )
    .unwrap();
    let command_count = dir.path().join("command-count");
    let command_script = dir.path().join("command.sh");
    std::fs::write(
        &command_script,
        "#!/bin/sh\nset -eu\nprintf 'run\\n' >> \"$1\"\n",
    )
    .unwrap();

    let config: Config = serde_json::from_value(json!({
        "node": {"id": "test", "role": "cognition", "listen": "127.0.0.1:0"},
        "paths": {"root": dir.path()},
        "engineering": {
            "model": "auto",
            "operation_timeout_secs": 5,
            "validation_timeout_secs": 5,
            "workspaces": [{
                "name": "fixture",
                "path": workspace,
                "checks": [
                    ["sh", validator, validation_count],
                    ["sh", notes_validator, notes_count]
                ],
                "commands": ["sh"]
            }]
        }
    }))
    .unwrap();
    config.validate().unwrap();
    let spool = Spool::new(dir.path().join("spool"), "test").unwrap();
    let shared = Shared::new(config, spool, 1, None);
    Fixture {
        dir,
        shared,
        workspace,
        validation_count,
        command_count,
        command_script,
    }
}

#[test]
fn repair_spans_files_and_failure_causes_across_a_mid_repair_interruption() {
    let fixture = fixture_two_checks();
    let notes_count = fixture.dir.path().join("notes-count");
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);

    // First failure has two independent causes on separate counters.
    let model = ScriptedModel::new([
        finish(),
        Action::Write {
            path: "code.txt".into(),
            content: "fixed\n".into(),
            expected: Some("broken\n".into()),
        },
        Action::Verify,
    ]);
    action(&fixture.shared, &model);
    let first = job(&fixture.shared, &id);
    assert_eq!(first["phase"], "active");
    assert_eq!(first["validation"]["passed"], false);
    // Validation is fail-fast: the second check is not measured until the
    // first cause is repaired.
    assert_eq!(first["validation"]["checks"].as_array().unwrap().len(), 1);
    assert_eq!(first["validation"]["checks"][0]["exit_code"], 1);
    assert_eq!(count(&fixture.validation_count), 1);
    assert_eq!(count(&notes_count), 0);

    // Repairing one file clears only its own check; the second cause is
    // measured for the first time on the next verification.
    action(&fixture.shared, &model);
    action(&fixture.shared, &model);
    let partial = job(&fixture.shared, &id);
    assert_eq!(partial["metrics"]["repairs"], 1);
    assert_eq!(partial["validation"]["checks"][0]["exit_code"], 0);
    assert_eq!(partial["validation"]["checks"][1]["exit_code"], 1);
    assert_eq!(partial["validation"]["passed"], false);
    assert_eq!(partial["metrics"]["verification_failures"], 2);
    assert_eq!(count(&fixture.validation_count), 2);
    assert_eq!(count(&notes_count), 1);
    model.assert_consumed();

    // An interruption mid-repair preserves the pending failure without
    // replaying anything or inferring a pass.
    set_pending(&fixture, &id, Action::Verify);
    let fixture = restart(fixture);
    let restored = job(&fixture.shared, &id);
    assert_eq!(restored["phase"], "paused");
    assert_eq!(restored["metrics"]["interruptions"], 1);
    assert_eq!(restored["metrics"]["repairs"], 1);
    assert_eq!(restored["validation"], Value::Null);
    assert_eq!(count(&fixture.validation_count), 2);
    assert_eq!(count(&notes_count), 1);
    let (status, reply) = handle(&fixture.shared, &json!({"action": "resume", "id": id}));
    assert_eq!(status, 200, "{reply}");
    assert_eq!(job(&fixture.shared, &id)["phase"], "active");

    // The remaining cause is repaired on the preserved candidate, and only
    // a real pass completes the job.
    let model = ScriptedModel::new([
        Action::Write {
            path: "notes.txt".into(),
            content: "done\n".into(),
            expected: Some("original notes\n".into()),
        },
        finish(),
    ]);
    action(&fixture.shared, &model);
    action(&fixture.shared, &model);
    model.assert_consumed();
    let completed = job(&fixture.shared, &id);
    assert_eq!(completed["phase"], "ready");
    assert_eq!(completed["metrics"]["repairs"], 2);
    assert_eq!(completed["metrics"]["verification_runs"], 3);
    assert_eq!(completed["metrics"]["verification_failures"], 2);
    assert_eq!(completed["metrics"]["interruptions"], 1);
    assert_eq!(completed["validation"]["passed"], true);
    assert_eq!(count(&fixture.validation_count), 3);
    assert_eq!(count(&notes_count), 2);
    assert_eq!(
        std::fs::read_to_string(candidate(&fixture.shared, &id).join("notes.txt")).unwrap(),
        "done\n"
    );
}

/// A runner whose dispatch is refused before any check starts: the claim is
/// a verification attempt, but no command ran, so the run/failure counters,
/// recorded evidence and repair episode are left untouched.
struct RejectingRunner;

impl super::ActionRunner for RejectingRunner {
    fn execute(
        &self,
        _state_path: &Path,
        _claimed: &Job,
        _policy: &WorkspaceConfig,
        _limits: &workspace::Limits,
        _cancel: &AtomicBool,
    ) -> worker::Dispatch {
        worker::Dispatch::Rejected("worker refused the claim before starting".into())
    }
}

#[test]
fn a_rejected_verification_is_an_attempt_not_a_run() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);

    let model = ScriptedModel::new([Action::Verify]);
    assert!(
        super::tick_with_runner(&fixture.shared, &model, &RejectingRunner)
            .expect("rejected tick")
            .is_some()
    );
    let rejected = job(&fixture.shared, &id);
    assert_eq!(rejected["phase"], "active");
    assert_eq!(rejected["metrics"]["verification_attempts"], 1);
    assert_eq!(rejected["metrics"]["verification_runs"], 0);
    assert_eq!(rejected["metrics"]["verification_failures"], 0);
    assert_eq!(rejected["validation"], Value::Null);
    assert_eq!(rejected["repair_pending"], false);
    assert_eq!(count(&fixture.validation_count), 0);

    // A real dispatch still verifies normally afterwards.
    let model = ScriptedModel::new([fix(), Action::Verify]);
    action(&fixture.shared, &model);
    action(&fixture.shared, &model);
    let verified = job(&fixture.shared, &id);
    assert_eq!(verified["metrics"]["verification_attempts"], 2);
    assert_eq!(verified["metrics"]["verification_runs"], 1);
    assert_eq!(verified["metrics"]["verification_failures"], 0);
    assert_eq!(verified["validation"]["passed"], true);
    assert_eq!(count(&fixture.validation_count), 1);
}

#[test]
fn semantic_checkpoint_rejects_runs_beyond_recorded_attempts() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let model = ScriptedModel::new([fix(), Action::Verify]);
    action(&fixture.shared, &model);
    action(&fixture.shared, &model);
    assert_eq!(
        job(&fixture.shared, &id)["metrics"]["verification_attempts"],
        1
    );

    let mut persisted = checkpoint(&fixture);
    let saved = persisted["jobs"].as_array_mut().unwrap()[0]
        .as_object_mut()
        .unwrap();
    saved["metrics"]
        .as_object_mut()
        .unwrap()
        .remove("verification_attempts");
    // A legacy store predating the counter stays loadable.
    write_checkpoint(&fixture, &persisted);
    let fixture = restart(fixture);
    assert_eq!(job(&fixture.shared, &id)["metrics"]["verification_runs"], 1);

    // Rewriting attempts below the recorded runs is inconsistent.
    let mut persisted = checkpoint(&fixture);
    let saved = persisted["jobs"].as_array_mut().unwrap()[0]
        .as_object_mut()
        .unwrap();
    saved["metrics"]["verification_attempts"] = json!(0);
    saved["metrics"]["verification_runs"] = json!(2);
    write_checkpoint(&fixture, &persisted);
    let fixture = restart(fixture);
    assert!(
        fixture.shared.engineering.get(&id).is_none(),
        "runs without attempts must be quarantined"
    );
}

/// Replanning capability (§7.5): the model receives failed-check evidence in
/// its decision context, and switching methods on that observation is what
/// completes the job. Repeating the identical failing method never does.
#[test]
fn observed_failures_support_a_method_switch_that_finishes_the_job() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let wrong_run = || Action::Run {
        argv: vec![
            "sh".to_owned(),
            "-c".to_owned(),
            "printf 'half-fixed\\n' > code.txt".to_owned(),
        ],
    };
    let model = ScriptedModel::new([
        wrong_run(),
        Action::Verify,
        wrong_run(),
        Action::Verify,
        Action::Replace {
            path: "code.txt".to_owned(),
            old: "half-fixed\n".to_owned(),
            new: "fixed\n".to_owned(),
        },
        Action::Verify,
        finish(),
    ]);

    action(&fixture.shared, &model); // run: writes half-fixed
    action(&fixture.shared, &model); // verify 1: fails
    assert_eq!(job(&fixture.shared, &id)["validation"]["passed"], false);
    action(&fixture.shared, &model); // same method repeated
    action(&fixture.shared, &model); // verify 2: fails again
    assert_eq!(job(&fixture.shared, &id)["phase"], "active");
    assert_eq!(count(&fixture.validation_count), 2);

    // The replanning basis is measured: the model's decision contexts carry
    // the failed-check evidence, not just an ok flag.
    let contexts = model.contexts.lock().unwrap();
    assert_eq!(contexts.len(), 4);
    let check_text =
        contexts[3]["validation"].to_string() + &contexts[3]["observations"].to_string();
    assert!(
        check_text.contains("exit_code") || check_text.contains("\"ok\":false"),
        "failed verification is observable to the model: {check_text}"
    );
    drop(contexts);

    action(&fixture.shared, &model); // replace: different method
    action(&fixture.shared, &model); // verify 3: passes
    assert_eq!(job(&fixture.shared, &id)["validation"]["passed"], true);
    action(&fixture.shared, &model); // finish: passes
    assert_eq!(job(&fixture.shared, &id)["phase"], "ready");
    // finish re-runs the configured checks; verify×3 + finish = 4 measured runs.
    assert_eq!(count(&fixture.validation_count), 4);
    assert_eq!(
        std::fs::read_to_string(candidate(&fixture.shared, &id).join("code.txt")).unwrap(),
        "fixed\n"
    );
    model.assert_consumed();
}

/// A claim whose projection cannot persist must produce zero side effects,
/// not an execution — and the job recovers once the namespace is usable.
#[test]
fn an_unpublishable_claim_executes_nothing_and_recovers() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let executions = checkpoint_path(&fixture).with_file_name("engineering.json.executions");
    std::fs::create_dir_all(&executions).unwrap();
    // A directory at the projection's target makes the atomic rename fail.
    let blocked = executions.join(format!("{id}.claim.json"));
    std::fs::create_dir(&blocked).unwrap();
    let model = ScriptedModel::new([
        Action::Run {
            argv: vec![
                "sh".to_owned(),
                "-c".to_owned(),
                format!(
                    "printf x >> '{}'; printf 'x\\n'",
                    fixture
                        .dir
                        .path()
                        .join("workspace/never-executed.txt")
                        .display()
                ),
            ],
        },
        Action::Run {
            argv: vec!["sh".to_owned(), "-c".to_owned(), "printf ok\\n".to_owned()],
        },
    ]);
    // First claim: publish fails inside the lock → Err receipt, pending
    // cleared, zero side effects, job stays retryable.
    action(&fixture.shared, &model);
    let current = job(&fixture.shared, &id);
    assert_eq!(current["phase"], "active");
    assert!(current["pending"].is_null(), "{current}");
    assert_eq!(current["metrics"]["actions"], 1);
    assert!(
        current["last_error"]
            .as_str()
            .unwrap()
            .contains("execution record target is corrupt"),
        "{current}"
    );
    assert!(
        !candidate(&fixture.shared, &id)
            .join("never-executed.txt")
            .exists(),
        "no workspace side effect may occur without a durable claim"
    );
    assert!(blocked.is_dir(), "publish must not clobber the blocker");

    // Repair the namespace; the next claim publishes and executes once.
    std::fs::remove_dir(&blocked).unwrap();
    action(&fixture.shared, &model);
    let current = job(&fixture.shared, &id);
    assert!(current["pending"].is_null(), "{current}");
    assert_eq!(current["metrics"]["actions"], 2);
    assert!(current["last_error"].is_null(), "{current}");
    assert!(
        executions.join(format!("{id}.claim.json")).is_file(),
        "the projection is durable before execution"
    );
    model.assert_consumed();
}
