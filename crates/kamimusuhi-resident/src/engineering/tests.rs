//! Deterministic resident-harness checks. Only model decisions are mocked:
//! worktrees, commands, validation receipts and durable state use real files.

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

fn restart(fixture: &Fixture) -> Shared {
    let spool = Spool::new(fixture.dir.path().join("spool"), "test").unwrap();
    Shared::new(fixture.shared.config.clone(), spool, 2, None)
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
    saved["pending"] = json!({"sequence": 99, "action": pending, "started_at": 1});
    std::fs::write(path, serde_json::to_vec_pretty(&persisted).unwrap()).unwrap();
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
    let restarted = restart(&fixture);
    let restored = job(&restarted, &id);
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
    let (_, resumed) = handle(&restarted, &json!({"action": "resume", "id": id}));
    assert_eq!(resumed["ok"], true, "{resumed}");
    let resumed_model = ScriptedModel::new([finish()]);
    action(&restarted, &resumed_model);
    assert_eq!(job(&restarted, &id)["phase"], "ready");
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
fn loading_uncertain_effects_pauses_without_replaying_run_verify_or_finish() {
    for pending_kind in ["run", "verify", "finish"] {
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
            "verify" => Action::Verify,
            "finish" => finish(),
            _ => unreachable!(),
        };
        // Model a persisted intent whose completion receipt was lost. This
        // deliberately tests load/recovery, not operating-system crash timing.
        set_pending(&fixture, &id, pending);
        let restarted = restart(&fixture);
        let restored = job(&restarted, &id);
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
        assert!(tick_with(&restarted, &model).unwrap().is_none());
        assert_eq!(model.calls(), 0);
        assert_eq!(count(&fixture.command_count), 0);
        assert_eq!(count(&fixture.validation_count), 0);

        let (_, reply) = handle(&restarted, &json!({"action": "resume", "id": id}));
        assert_eq!(reply["ok"], true, "{reply}");
        action(&restarted, &model);
        assert_eq!(model.calls(), 1);
        assert_eq!(count(&fixture.command_count), 0);
        assert_eq!(count(&fixture.validation_count), 0);
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
    let restarted = restart(&fixture);
    let restored = job(&restarted, &id);
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
    action(&restarted, &model);
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
    let restarted = restart(&fixture);
    let (status, duplicate) = handle(&restarted, &request);
    assert!((200..300).contains(&status), "{duplicate}");
    assert_eq!(duplicate["job"]["id"], id);
    assert_eq!(duplicate["job"]["candidate"], first_candidate);
    let (_, listed) = handle(&restarted, &json!({"action": "list"}));
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
    let restarted = restart(&fixture);
    let restored = job(&restarted, &id);
    assert_eq!(restored["phase"], "paused");
    assert_eq!(restored["metrics"]["interruptions"], 1);
    assert!(restored["pending"].is_null());
    assert!(
        restored["observations"]
            .to_string()
            .contains("outcome_unknown")
    );
    let no_replay = ScriptedModel::new([]);
    assert!(tick_with(&restarted, &no_replay).unwrap().is_none());
    assert_eq!(no_replay.calls(), 0);
    assert_eq!(count(&fixture.command_count), 1);
    assert_eq!(
        std::fs::read_to_string(candidate(&restarted, &id).join("result.txt")).unwrap(),
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
    let restarted = restart(&fixture);
    let (_, reply) = handle(
        &restarted,
        &json!({"action": "create", "workspace": "fixture", "objective": "new work"}),
    );
    assert_eq!(reply["ok"], false, "{reply}");
    let model = ScriptedModel::new([]);
    assert!(tick_with(&restarted, &model).is_err());
    assert_eq!(model.calls(), 0);
    assert_eq!(std::fs::read(&path).unwrap(), corrupt);
    assert_eq!(
        std::fs::read_to_string(candidate_path.join("preserved.txt")).unwrap(),
        "unsaved result\n"
    );
    let (_, status) = handle(&restarted, &json!({"action": "list"}));
    assert!(!status["status"]["error"].is_null());
}
