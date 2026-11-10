//! A failure episode outlives the short observation window. Only a durable
//! accepted decision closes it; task validation remains a separate outcome.

use super::*;

const FIRST_ERROR: &str = "engineering model unavailable (HTTP 503)";
const RETRY_ERROR: &str = "engineering model unavailable (HTTP 400)";

struct FailingModel {
    error: String,
    calls: AtomicUsize,
}

impl EngineerModel for FailingModel {
    fn decide(&self, _context: &Value) -> Result<Action, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(self.error.clone())
    }
}

fn due(fixture: &Fixture, id: &str) {
    // Drive due scheduler decisions deterministically. This suite measures
    // episode retention, not the real retry interval covered by HTTP tests.
    fixture
        .shared
        .engineering
        .update(id, |job| {
            job.next_attempt_at = 0;
            Ok(())
        })
        .unwrap();
}

fn fail(fixture: &Fixture, id: &str, error: &str) -> Value {
    due(fixture, id);
    let model = FailingModel {
        error: error.to_owned(),
        calls: AtomicUsize::new(0),
    };
    assert!(tick_with(&fixture.shared, &model).unwrap().is_some());
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    job(&fixture.shared, id)
}

fn read() -> Action {
    Action::Read {
        path: "code.txt".to_owned(),
    }
}

#[test]
fn the_first_failure_survives_rollover_reload_and_a_later_recovery() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let original = job(&fixture.shared, &id);
    let first = fail(&fixture, &id, FIRST_ERROR);
    assert_eq!(first["model_recovery"]["first_error"], FIRST_ERROR);
    assert_eq!(first["model_recovery"]["latest_error"], FIRST_ERROR);
    assert_eq!(first["model_recovery"]["attempts"], 1);
    assert!(first["model_recovery"]["recovered_at"].is_null());

    // Opaque persisted timestamps make an accidental reset observable
    // without sleeping. Clock ordering is deliberately not an invariant.
    let mut snapshot = checkpoint(&fixture);
    snapshot["jobs"][0]["model_recovery"]["first_failure_at"] = json!(3);
    snapshot["jobs"][0]["model_recovery"]["last_failure_at"] = json!(2);
    write_checkpoint(&fixture, &snapshot);
    let fixture = restart(fixture);
    assert!(fixture.shared.engineering.summary()["error"].is_null());
    for _ in 0..KEEP_OBSERVATIONS + 1 {
        fail(&fixture, &id, RETRY_ERROR);
    }
    let failures = (KEEP_OBSERVATIONS + 2) as u64;
    let before = job(&fixture.shared, &id);
    let episode = before["model_recovery"].clone();
    let observations = before["observations"].as_array().unwrap();
    assert_eq!(observations.len(), KEEP_OBSERVATIONS);
    assert!(
        observations.iter().all(|record| {
            record["action"] == "model" && record["detail"]["error"] == RETRY_ERROR
        })
    );
    assert!(!before["observations"].to_string().contains("503"));
    assert_eq!(episode["first_error"], FIRST_ERROR);
    assert_eq!(episode["latest_error"], RETRY_ERROR);
    assert_eq!(episode["first_failure_at"], 3);
    assert!(episode["last_failure_at"].as_u64().is_some());
    assert_eq!(episode["attempts"], failures);
    assert!(episode["recovered_at"].is_null());
    assert_eq!(before["metrics"]["model_failures"], failures);
    assert_eq!(before["metrics"]["decisions"], 0);
    assert_eq!(before["metrics"]["actions"], 0);
    assert_eq!(before["candidate"], original["candidate"]);
    assert_eq!(before["objective"], original["objective"]);
    let bytes = std::fs::read(checkpoint_path(&fixture)).unwrap();
    let fixture = restart(fixture);
    assert_eq!(job(&fixture.shared, &id)["model_recovery"], episode);
    assert_eq!(std::fs::read(checkpoint_path(&fixture)).unwrap(), bytes);

    due(&fixture, &id);
    let model = ScriptedModel::new([read()]);
    action(&fixture.shared, &model);
    assert_eq!(model.contexts.lock().unwrap()[0]["model_recovery"], episode);
    model.assert_consumed();
    let recovered = job(&fixture.shared, &id)["model_recovery"].clone();
    assert!(recovered["recovered_at"].as_u64().is_some());
    for field in [
        "first_error",
        "latest_error",
        "first_failure_at",
        "last_failure_at",
        "attempts",
    ] {
        assert_eq!(recovered[field], episode[field]);
    }

    // Re-loading a historical episode with non-ordered wall-clock stamps
    // must neither quarantine it nor refresh its recovery on every action.
    let mut snapshot = checkpoint(&fixture);
    snapshot["jobs"][0]["model_recovery"]["last_failure_at"] = json!(2);
    snapshot["jobs"][0]["model_recovery"]["recovered_at"] = json!(1);
    let historical = snapshot["jobs"][0]["model_recovery"].clone();
    write_checkpoint(&fixture, &snapshot);
    let fixture = restart(fixture);
    assert!(fixture.shared.engineering.summary()["error"].is_null());
    let model = ScriptedModel::new([Action::Read {
        path: "missing-file.txt".to_owned(),
    }]);
    action(&fixture.shared, &model);
    let after = job(&fixture.shared, &id);
    assert_eq!(after["model_recovery"], historical);
    assert_eq!(
        after["observations"].as_array().unwrap().last().unwrap()["ok"],
        false
    );
    assert_eq!(after["metrics"]["model_failures"], failures);
    assert_eq!(after["metrics"]["actions"], 2);
    assert_eq!(
        model.contexts.lock().unwrap()[0]["model_recovery"],
        historical
    );
    model.assert_consumed();

    let next = fail(&fixture, &id, "a new independent provider outage");
    assert_eq!(
        next["model_recovery"]["first_error"],
        "a new independent provider outage"
    );
    assert_eq!(
        next["model_recovery"]["latest_error"],
        next["model_recovery"]["first_error"]
    );
    assert_eq!(next["model_recovery"]["attempts"], 1);
    assert!(next["model_recovery"]["recovered_at"].is_null());
    assert_eq!(next["metrics"]["model_failures"], failures + 1);
    assert_eq!(next["candidate"], original["candidate"]);
    assert_eq!(
        std::fs::read_to_string(fixture.workspace.join("code.txt")).unwrap(),
        "broken\n"
    );
    assert_eq!(count(&fixture.command_count), 0);
    assert_eq!(count(&fixture.validation_count), 0);
    eprintln!(
        "model episode retention: failures={failures} observations={} first_cause_preserved=true reload_preserved=true recovered_once=true next_episode_attempts=1",
        observations.len()
    );
}

#[test]
fn old_v2_without_an_episode_does_not_invent_the_first_cause() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let mut snapshot = checkpoint(&fixture);
    snapshot["jobs"][0]
        .as_object_mut()
        .unwrap()
        .remove("model_recovery");
    snapshot["jobs"][0]["metrics"]["model_failures"] = json!(4);
    snapshot["jobs"][0]["last_error"] = json!("only the latest legacy error is known");
    let bytes = write_checkpoint(&fixture, &snapshot);
    let fixture = restart(fixture);
    assert!(fixture.shared.engineering.summary()["error"].is_null());
    let restored = job(&fixture.shared, &id);
    assert!(restored["model_recovery"].is_null());
    assert_eq!(restored["metrics"]["model_failures"], 4);
    assert_eq!(
        restored["last_error"],
        "only the latest legacy error is known"
    );
    assert_eq!(std::fs::read(checkpoint_path(&fixture)).unwrap(), bytes);
    let model = ScriptedModel::new([read()]);
    action(&fixture.shared, &model);
    assert!(job(&fixture.shared, &id)["model_recovery"].is_null());
    model.assert_consumed();
    let first_known = fail(&fixture, &id, FIRST_ERROR);
    assert_eq!(first_known["model_recovery"]["first_error"], FIRST_ERROR);
    assert_eq!(first_known["model_recovery"]["attempts"], 1);
    assert_eq!(first_known["metrics"]["model_failures"], 5);
    assert_eq!(count(&fixture.command_count), 0);
    assert_eq!(count(&fixture.validation_count), 0);
}

#[test]
fn malformed_failure_episodes_are_quarantined_without_rewriting_the_original() {
    for case in [
        "zero_attempts",
        "attempts_above_total",
        "oversized_first",
        "oversized_latest",
    ] {
        let fixture = fixture();
        let id = create(&fixture.shared);
        prepare(&fixture.shared, &id);
        let mut snapshot = checkpoint(&fixture);
        snapshot["jobs"][0]["metrics"]["model_failures"] = json!(2);
        snapshot["jobs"][0]["model_recovery"] = json!({
            "first_error":FIRST_ERROR,"latest_error":RETRY_ERROR,
            "first_failure_at":3,"last_failure_at":2,"attempts":2,"recovered_at":null,
        });
        let episode = &mut snapshot["jobs"][0]["model_recovery"];
        match case {
            "zero_attempts" => episode["attempts"] = json!(0),
            "attempts_above_total" => episode["attempts"] = json!(3),
            "oversized_first" => episode["first_error"] = json!("あ".repeat(342)),
            "oversized_latest" => episode["latest_error"] = json!("x".repeat(1025)),
            _ => unreachable!(),
        }
        assert_snapshot_quarantined(fixture, snapshot, case);
    }
}

#[test]
fn empty_control_and_oversized_model_errors_remain_retryable() {
    for error in [
        String::new(),
        "\0temporary provider failure\0".to_owned(),
        "障害".repeat(400),
    ] {
        let fixture = fixture();
        let id = create(&fixture.shared);
        prepare(&fixture.shared, &id);
        let failed = fail(&fixture, &id, &error);
        assert!(fixture.shared.engineering.summary()["error"].is_null());
        assert_eq!(failed["phase"], "active");
        assert!(failed["pending"].is_null());
        assert_eq!(failed["model_recovery"]["attempts"], 1);
        assert!(failed["model_recovery"]["recovered_at"].is_null());
        for field in ["first_error", "latest_error"] {
            assert!(failed["model_recovery"][field].as_str().unwrap().len() <= 1024);
        }
        assert_eq!(
            failed["model_recovery"]["first_error"],
            failed["model_recovery"]["latest_error"]
        );
        let fixture = restart(fixture);
        assert!(fixture.shared.engineering.summary()["error"].is_null());
        assert_eq!(
            job(&fixture.shared, &id)["model_recovery"],
            failed["model_recovery"]
        );
        due(&fixture, &id);
        let model = ScriptedModel::new([read()]);
        action(&fixture.shared, &model);
        model.assert_consumed();
        assert!(
            job(&fixture.shared, &id)["model_recovery"]["recovered_at"]
                .as_u64()
                .is_some()
        );
        assert_eq!(count(&fixture.command_count), 0);
        assert_eq!(count(&fixture.validation_count), 0);
    }
}

#[test]
fn a_paused_inference_does_not_close_its_open_failure_episode() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let failed = fail(&fixture, &id, FIRST_ERROR);
    let episode = failed["model_recovery"].clone();
    assert_eq!(episode["attempts"], 1);
    due(&fixture, &id);
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
        let worker = scope.spawn(|| tick_with(&fixture.shared, &model));
        let entered = entered_rx.recv_timeout(Duration::from_secs(5));
        if entered.is_err() {
            model.release();
        }
        entered.expect("model is in progress before pause");
        let (_, paused) = handle(&fixture.shared, &json!({"action":"pause","id":id}));
        model.release();
        assert!(worker.join().unwrap().unwrap().is_some());
        assert_eq!(paused["ok"], true);
    });
    let paused = job(&fixture.shared, &id);
    assert_eq!(paused["phase"], "paused");
    assert_eq!(paused["model_recovery"], episode);
    assert_eq!(paused["metrics"]["decisions"], 0);
    assert_eq!(paused["metrics"]["actions"], 0);
    assert!(paused["pending"].is_null());
    assert_eq!(count(&fixture.command_count), 0);
    assert_eq!(model.calls.load(Ordering::SeqCst), 1);
    let fixture = restart(fixture);
    let (_, resumed) = handle(&fixture.shared, &json!({"action":"resume","id":id}));
    assert_eq!(resumed["ok"], true);
    assert_eq!(resumed["job"]["model_recovery"], episode);
    let model = ScriptedModel::new([read()]);
    action(&fixture.shared, &model);
    assert!(
        job(&fixture.shared, &id)["model_recovery"]["recovered_at"]
            .as_u64()
            .is_some()
    );
    assert_eq!(count(&fixture.command_count), 0);
    assert_eq!(count(&fixture.validation_count), 0);
}

#[test]
fn a_failed_claim_save_neither_closes_the_episode_nor_executes_the_command() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let failed = fail(&fixture, &id, FIRST_ERROR);
    let episode = failed["model_recovery"].clone();
    assert_eq!(episode["attempts"], 1);
    due(&fixture, &id);
    let path = checkpoint_path(&fixture);
    let bytes = std::fs::read(&path).unwrap();
    let preserved = fixture
        .dir
        .path()
        .join("checkpoint-with-open-model-episode.json");
    std::fs::rename(&path, &preserved).unwrap();
    std::fs::create_dir(&path).unwrap();
    let model = ScriptedModel::new([Action::Run {
        argv: vec![
            "sh".to_owned(),
            fixture.command_script.to_string_lossy().into_owned(),
            fixture.command_count.to_string_lossy().into_owned(),
        ],
    }]);
    assert!(tick_with(&fixture.shared, &model).is_err());
    assert!(!fixture.shared.engineering.summary()["error"].is_null());
    assert_eq!(job(&fixture.shared, &id)["model_recovery"], episode);
    assert_eq!(job(&fixture.shared, &id)["metrics"]["decisions"], 0);
    assert_eq!(job(&fixture.shared, &id)["metrics"]["actions"], 0);
    assert!(job(&fixture.shared, &id)["pending"].is_null());
    assert_eq!(std::fs::read(&preserved).unwrap(), bytes);
    assert_eq!(count(&fixture.command_count), 0);
    model.assert_consumed();
    std::fs::remove_dir(&path).unwrap();
    std::fs::rename(preserved, &path).unwrap();
    let fixture = restart(fixture);
    assert!(fixture.shared.engineering.summary()["error"].is_null());
    assert_eq!(job(&fixture.shared, &id)["model_recovery"], episode);
    let model = ScriptedModel::new([read()]);
    action(&fixture.shared, &model);
    assert!(
        job(&fixture.shared, &id)["model_recovery"]["recovered_at"]
            .as_u64()
            .is_some()
    );
    assert_eq!(count(&fixture.command_count), 0);
    assert_eq!(count(&fixture.validation_count), 0);
}
