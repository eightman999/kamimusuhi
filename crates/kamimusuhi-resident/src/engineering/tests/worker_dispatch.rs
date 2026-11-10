//! Parent-side decisions on a worker result. Real IPC and process death are
//! covered by engineering_worker/engineering_http, outside this injected seam.

use super::*;

struct ResultRunner {
    result: Mutex<Option<worker::Dispatch>>,
    calls: AtomicUsize,
    break_save: Option<PathBuf>,
}

impl ResultRunner {
    fn new(result: worker::Dispatch) -> Self {
        Self {
            result: Mutex::new(Some(result)),
            calls: AtomicUsize::new(0),
            break_save: None,
        }
    }
}

impl ActionRunner for ResultRunner {
    fn execute(
        &self,
        state_path: &Path,
        claimed: &Job,
        _policy: &WorkspaceConfig,
        _limits: &workspace::Limits,
        _cancel: &AtomicBool,
    ) -> worker::Dispatch {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let persisted: Value = serde_json::from_slice(&std::fs::read(state_path).unwrap()).unwrap();
        let stored = persisted["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|job| job["id"] == claimed.id)
            .unwrap();
        assert_eq!(stored["phase"], "active");
        assert_eq!(
            stored["pending"],
            serde_json::to_value(&claimed.pending).unwrap()
        );
        if let Some(preserved) = &self.break_save {
            std::fs::rename(state_path, preserved).unwrap();
            std::fs::create_dir(state_path).unwrap();
        }
        self.result.lock().unwrap().take().expect("one dispatch")
    }
}

fn run() -> Action {
    Action::Run {
        argv: vec!["sh".to_owned(), "-c".to_owned(), "exit 0".to_owned()],
    }
}

#[test]
fn unknown_worker_results_pause_each_process_action_without_claiming_completion() {
    for proposed in [
        run(),
        Action::Verify,
        Action::Finish {
            summary: "this claim has no confirmed result".to_owned(),
        },
    ] {
        let fixture = fixture();
        let id = create(&fixture.shared);
        prepare(&fixture.shared, &id);
        let original = job(&fixture.shared, &id);
        let name = action_name(&proposed);
        let model = ScriptedModel::new([proposed]);
        let runner = ResultRunner::new(worker::Dispatch::Unknown(
            "worker exited without a complete reply".to_owned(),
        ));
        let result = tick_with_runner(&fixture.shared, &model, &runner)
            .unwrap()
            .unwrap();
        assert_eq!(result.phase, Phase::Paused);
        let paused = job(&fixture.shared, &id);
        assert_eq!(paused["candidate"], original["candidate"]);
        assert_eq!(paused["metrics"]["decisions"], 1);
        assert_eq!(paused["metrics"]["actions"], 0);
        assert_eq!(paused["metrics"]["verification_runs"], 0);
        assert_eq!(paused["metrics"]["interruptions"], 1);
        assert_eq!(paused["next_action_sequence"], 2);
        assert!(paused["pending"].is_null());
        assert!(paused["validation"].is_null());
        assert_eq!(paused["last_error"], "interrupted_action_outcome_unknown");
        let observed = paused["observations"].as_array().unwrap().last().unwrap();
        assert_eq!(observed["action"], name);
        assert_eq!(observed["sequence"], 1);
        assert_eq!(observed["detail"]["sequence"], 1);
        assert_eq!(
            observed["detail"]["kind"],
            "interrupted_action_outcome_unknown"
        );
        model.assert_consumed();
        let no_replay = ScriptedModel::new([]);
        assert!(
            tick_with_runner(&fixture.shared, &no_replay, &runner)
                .unwrap()
                .is_none()
        );
        assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
        assert_eq!(no_replay.calls(), 0);
        let saved = std::fs::read(checkpoint_path(&fixture)).unwrap();
        let fixture = restart(fixture);
        assert!(fixture.shared.engineering.summary()["error"].is_null());
        assert_eq!(job(&fixture.shared, &id), paused);
        assert_eq!(std::fs::read(checkpoint_path(&fixture)).unwrap(), saved);
        assert!(tick_with(&fixture.shared, &no_replay).unwrap().is_none());
        let (_, resumed) = handle(
            &fixture.shared,
            &json!({"action":"resume","id":id,"note":"fixture execution is stopped"}),
        );
        assert_eq!(resumed["ok"], true);
        let inspect = ScriptedModel::new([Action::Read {
            path: "code.txt".to_owned(),
        }]);
        action(&fixture.shared, &inspect);
        let continued = job(&fixture.shared, &id);
        assert_eq!(continued["metrics"]["decisions"], 2);
        assert_eq!(continued["metrics"]["actions"], 1);
        assert_eq!(continued["metrics"]["interruptions"], 1);
        assert_eq!(
            continued["observations"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()["sequence"],
            2
        );
        assert_eq!(count(&fixture.command_count), 0);
        assert_eq!(count(&fixture.validation_count), 0);
    }
}

#[test]
fn confirmed_worker_errors_and_prestart_rejections_leave_a_retryable_failure() {
    for dispatch in [
        worker::Dispatch::Completed(Err("confirmed workspace operation error".to_owned())),
        worker::Dispatch::Rejected("worker refused the request before starting".to_owned()),
    ] {
        let fixture = fixture();
        let id = create(&fixture.shared);
        prepare(&fixture.shared, &id);
        let model = ScriptedModel::new([run()]);
        let runner = ResultRunner::new(dispatch);
        tick_with_runner(&fixture.shared, &model, &runner).unwrap();
        let failed = job(&fixture.shared, &id);
        assert_eq!(failed["phase"], "active");
        assert_eq!(failed["metrics"]["decisions"], 1);
        assert_eq!(failed["metrics"]["actions"], 1);
        assert_eq!(failed["metrics"]["interruptions"], 0);
        assert!(failed["pending"].is_null());
        assert!(!failed["last_error"].as_str().unwrap().is_empty());
        assert_eq!(
            failed["observations"].as_array().unwrap().last().unwrap()["ok"],
            false
        );
        let inspect = ScriptedModel::new([Action::Read {
            path: "code.txt".to_owned(),
        }]);
        action(&fixture.shared, &inspect);
        assert_eq!(job(&fixture.shared, &id)["metrics"]["actions"], 2);
        assert_eq!(count(&fixture.command_count), 0);
    }
}

#[test]
fn an_unknown_result_save_failure_preserves_the_pending_claim_for_reload() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let preserved = fixture
        .dir
        .path()
        .join("claimed-before-worker-disappeared.json");
    let mut runner = ResultRunner::new(worker::Dispatch::Unknown("reply lost".to_owned()));
    runner.break_save = Some(preserved.clone());
    let model = ScriptedModel::new([run()]);
    assert!(tick_with_runner(&fixture.shared, &model, &runner).is_err());
    assert!(!fixture.shared.engineering.summary()["error"].is_null());
    let in_memory = job(&fixture.shared, &id);
    assert_eq!(in_memory["pending"]["sequence"], 1);
    assert_eq!(in_memory["metrics"]["actions"], 0);
    let no_replay = ScriptedModel::new([]);
    assert!(tick_with_runner(&fixture.shared, &no_replay, &runner).is_err());
    assert_eq!(runner.calls.load(Ordering::SeqCst), 1);
    assert_eq!(no_replay.calls(), 0);
    let checkpoint = checkpoint_path(&fixture);
    std::fs::remove_dir(&checkpoint).unwrap();
    std::fs::rename(preserved, checkpoint).unwrap();
    let fixture = restart(fixture);
    assert!(fixture.shared.engineering.summary()["error"].is_null());
    let recovered = job(&fixture.shared, &id);
    assert_eq!(recovered["phase"], "paused");
    assert_eq!(recovered["metrics"]["decisions"], 1);
    assert_eq!(recovered["metrics"]["actions"], 0);
    assert_eq!(recovered["metrics"]["interruptions"], 1);
    assert_eq!(recovered["next_action_sequence"], 2);
    assert!(recovered["pending"].is_null());
    assert!(tick_with(&fixture.shared, &no_replay).unwrap().is_none());
    assert_eq!(count(&fixture.command_count), 0);
    assert_eq!(count(&fixture.validation_count), 0);
}
