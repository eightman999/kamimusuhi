//! Recorded verification is bound to its commit and operator check settings.
//! All checks and worktrees are real; the model only supplies fixed actions.

use super::*;

fn request() -> Value {
    json!({
        "action":"create", "workspace":"fixture",
        "objective":"Fix code.txt and preserve the operator's workspace",
        "request_id":"verification-identity-fixture",
    })
}

fn ready(mut fixture: Fixture, two_checks: bool) -> (Fixture, String) {
    if two_checks {
        fixture
            .shared
            .config
            .engineering
            .as_mut()
            .unwrap()
            .workspaces[0]
            .checks
            .push(vec!["sh".into(), "-c".into(), "exit 0".into()]);
    }
    let (status, created) = handle(&fixture.shared, &request());
    assert_eq!(status, 201, "{created}");
    let id = created["job"]["id"].as_str().unwrap().to_owned();
    prepare(&fixture.shared, &id);
    let model = ScriptedModel::new([fix(), finish()]);
    action(&fixture.shared, &model);
    action(&fixture.shared, &model);
    model.assert_consumed();
    assert_eq!(job(&fixture.shared, &id)["phase"], "ready");
    assert_eq!(count(&fixture.validation_count), 1);
    (fixture, id)
}

fn views(fixture: &Fixture, id: &str, replay: bool) -> Vec<(&'static str, Value)> {
    let (status, shown) = handle(&fixture.shared, &json!({"action":"show","id":id}));
    assert_eq!(status, 200, "{shown}");
    let (status, listed) = handle(&fixture.shared, &json!({"action":"list"}));
    assert_eq!(status, 200, "{listed}");
    let by_id = |jobs: &Value| {
        jobs.as_array()
            .unwrap()
            .iter()
            .find(|j| j["id"] == id)
            .unwrap()
            .clone()
    };
    let status = fixture.shared.status();
    let mut result = vec![
        ("show", shown["job"].clone()),
        ("list", by_id(&listed["jobs"])),
        ("list summary", by_id(&listed["status"]["jobs"])),
        ("resident status", by_id(&status["engineering"]["jobs"])),
    ];
    if replay {
        let (status, repeated) = handle(&fixture.shared, &request());
        assert_eq!(status, 201, "{repeated}");
        assert_eq!(repeated["job"]["id"], id);
        result.push(("request replay", repeated["job"].clone()));
    }
    result
}

fn assert_views(fixture: &Fixture, id: &str, expected: &str, replay: bool) {
    let recorded = job(&fixture.shared, id);
    let count_before = count(&fixture.validation_count);
    let metrics = recorded["metrics"].clone();
    let current_views = views(fixture, id, replay);
    for (source, view) in &current_views {
        assert_eq!(view["phase"], "ready", "{source}: {view}");
        let verification = &view["verification"];
        assert_eq!(verification["scope"], "recorded_commit", "{source}: {view}");
        assert_eq!(verification["head"], recorded["validation"]["head"]);
        assert_eq!(verification["passed"], true);
        assert_eq!(
            verification["policy"]["status"], expected,
            "{source}: {view}"
        );
        assert_eq!(verification["candidate"]["status"], "not_observed");
        assert!(verification["candidate"]["observed_at"].is_null());
        assert_eq!(view["metrics"], metrics);
    }
    for pair in current_views.windows(2) {
        assert_eq!(pair[0].1["verification"], pair[1].1["verification"]);
    }
    assert_eq!(job(&fixture.shared, id)["metrics"], metrics);
    assert_eq!(count(&fixture.validation_count), count_before);
    let unused_model = ScriptedModel::new([]);
    assert!(tick_with(&fixture.shared, &unused_model).unwrap().is_none());
    assert_eq!(unused_model.calls(), 0);
}

#[test]
fn ready_receipt_keeps_policy_identity_across_all_views_and_reload() {
    let (fixture, id) = ready(fixture(), false);
    let recorded = job(&fixture.shared, &id);
    let contract = &recorded["validation"]["contract"];
    println!(
        "verification_identity matching contract_present={} actual_checks={}",
        !contract.is_null(),
        count(&fixture.validation_count)
    );
    assert_eq!(contract["schema_version"], 1);
    let digest = contract["policy_digest"].as_str().unwrap();
    assert!(digest.starts_with("sha256:") && digest.len() == 71);
    assert!(contract["completed_at"].as_u64().unwrap() > 0);
    assert_views(&fixture, &id, "matching", true);
    let saved = std::fs::read(checkpoint_path(&fixture)).unwrap();
    let fixture = restart(fixture);
    assert_eq!(job(&fixture.shared, &id), recorded);
    assert_eq!(std::fs::read(checkpoint_path(&fixture)).unwrap(), saved);
    assert_views(&fixture, &id, "matching", true);
    assert_eq!(count(&fixture.validation_count), 1);
}

#[test]
fn failed_verification_retains_its_contract_time_and_failed_view() {
    let fixture = fixture();
    let (status, created) = handle(&fixture.shared, &request());
    assert_eq!(status, 201, "{created}");
    let id = created["job"]["id"].as_str().unwrap().to_owned();
    prepare(&fixture.shared, &id);
    // The initial candidate is broken. This executes the configured check;
    // its failed receipt must have the same provenance as a successful one.
    let model = ScriptedModel::new([
        Action::Verify,
        Action::Checkpoint {
            summary: "The actual check failed".into(),
            next_step: "Repair the recorded candidate".into(),
        },
    ]);
    action(&fixture.shared, &model);
    let recorded = job(&fixture.shared, &id);
    let receipt = &recorded["validation"];
    let contract = &receipt["contract"];
    println!(
        "verification_identity failed_receipt={} contract_present={} actual_checks={}",
        receipt["passed"] == false,
        !contract.is_null(),
        count(&fixture.validation_count)
    );
    assert_eq!(recorded["phase"], "active");
    assert_eq!(receipt["passed"], false);
    assert_eq!(recorded["metrics"]["verification_runs"], 1);
    assert_eq!(recorded["metrics"]["verification_failures"], 1);
    assert_eq!(contract["schema_version"], 1);
    assert!(contract["completed_at"].as_u64().unwrap() > 0);
    let digest = contract["policy_digest"].as_str().unwrap();
    assert!(digest.starts_with("sha256:") && digest.len() == 71);

    let visible = views(&fixture, &id, true);
    let expected_verification = visible[0].1["verification"].clone();
    for (source, view) in &visible {
        assert_eq!(view["phase"], "active", "{source}: {view}");
        let verification = &view["verification"];
        assert_eq!(verification["scope"], "recorded_commit");
        assert_eq!(verification["head"], receipt["head"]);
        assert_eq!(verification["passed"], false, "{source}: {view}");
        assert_eq!(verification["completed_at"], contract["completed_at"]);
        assert_eq!(verification["policy"]["status"], "matching");
        assert_eq!(verification["policy"]["recorded_digest"], digest);
        assert_eq!(verification["policy"]["current_digest"], digest);
        assert_eq!(verification["candidate"]["status"], "not_observed");
        assert!(verification["candidate"]["observed_at"].is_null());
        assert_eq!(verification, &expected_verification);
        assert_eq!(view["metrics"], recorded["metrics"]);
    }
    assert_eq!(job(&fixture.shared, &id), recorded);
    assert_eq!(count(&fixture.validation_count), 1);
    assert_eq!(count(&fixture.command_count), 0);

    action(&fixture.shared, &model);
    model.assert_consumed();
    assert_eq!(model.calls(), 2);
    assert_eq!(
        model.contexts.lock().unwrap()[1]["verification"],
        expected_verification
    );
    assert_eq!(job(&fixture.shared, &id)["validation"], *receipt);
    assert_eq!(count(&fixture.validation_count), 1);
    assert_eq!(count(&fixture.command_count), 0);
}

#[test]
fn relevant_configuration_changes_are_distinct_from_unrelated_agent_settings() {
    let (mut fixture, id) = ready(fixture(), true);
    let original = fixture.shared.config.engineering.as_ref().unwrap().clone();
    let original_receipt = job(&fixture.shared, &id)["validation"].clone();
    for case in [
        "argv",
        "order",
        "added check",
        "timeout",
        "output",
        "workspace path",
    ] {
        let config = fixture.shared.config.engineering.as_mut().unwrap();
        *config = original.clone();
        match case {
            "argv" => config.workspaces[0].checks[1][2] = "exit 7".into(),
            "order" => config.workspaces[0].checks.reverse(),
            "added check" => config.workspaces[0].checks.push(vec!["false".into()]),
            "timeout" => config.validation_timeout_secs += 1,
            "output" => config.max_output_bytes += 1024,
            "workspace path" => config.workspaces[0].path = fixture.dir.path().join("different"),
            _ => unreachable!(),
        }
        assert_views(&fixture, &id, "changed", true);
        assert_eq!(
            job(&fixture.shared, &id)["validation"],
            original_receipt,
            "{case}"
        );
    }
    let config = fixture.shared.config.engineering.as_mut().unwrap();
    *config = original.clone();
    config.model = "different-model".into();
    config.tick_secs += 1;
    config.max_tokens += 1;
    config.operation_timeout_secs += 1;
    config.workspaces[0]
        .commands
        .push("different-program".into());
    assert_views(&fixture, &id, "matching", true);
    fixture
        .shared
        .config
        .engineering
        .as_mut()
        .unwrap()
        .workspaces[0]
        .name = "renamed".into();
    assert_views(&fixture, &id, "unavailable", false);
    println!(
        "verification_identity changed_settings=6 unrelated_group=1 unavailable=1 extra_checks=0 extra_model=0"
    );
}

#[test]
fn current_candidate_is_explicitly_unobserved_after_edits_commit_and_removal() {
    let (fixture, id) = ready(fixture(), false);
    let path = candidate(&fixture.shared, &id);
    let recorded = job(&fixture.shared, &id);
    let head = recorded["validation"]["head"].as_str().unwrap();
    std::fs::write(path.join("code.txt"), "external change\n").unwrap();
    assert_views(&fixture, &id, "matching", true);
    git(&path, &["add", "code.txt"]);
    git(
        &path,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "external candidate edit",
        ],
    );
    assert_ne!(git(&path, &["rev-parse", "HEAD"]), head);
    assert_views(&fixture, &id, "matching", true);
    let moved = path.with_extension("moved-fixture");
    std::fs::rename(&path, &moved).unwrap();
    assert_views(&fixture, &id, "matching", true);
    assert_eq!(
        job(&fixture.shared, &id)["validation"],
        recorded["validation"]
    );
    assert_eq!(count(&fixture.validation_count), 1);
    println!(
        "verification_identity external_edit=unobserved external_commit=unobserved missing_candidate=unobserved extra_checks=0"
    );
}

#[test]
fn old_receipt_is_unknown_without_rewriting_or_inventing_current_identity() {
    let (fixture, id) = ready(fixture(), false);
    let mut snapshot = checkpoint(&fixture);
    snapshot["jobs"][0]["validation"]
        .as_object_mut()
        .unwrap()
        .remove("contract");
    let saved = write_checkpoint(&fixture, &snapshot);
    let fixture = restart(fixture);
    assert_eq!(std::fs::read(checkpoint_path(&fixture)).unwrap(), saved);
    assert!(job(&fixture.shared, &id)["validation"]["contract"].is_null());
    assert_views(&fixture, &id, "unknown", true);
    let shown = views(&fixture, &id, false);
    for (_, view) in shown {
        assert!(view["verification"]["completed_at"].is_null());
        assert!(view["verification"]["policy"]["recorded_digest"].is_null());
    }
    assert_eq!(count(&fixture.validation_count), 1);
    println!("verification_identity legacy=unknown inferred_identity=0 extra_checks=0");
}

#[test]
fn malformed_verification_contract_is_quarantined_before_recovery() {
    for case in [
        "version",
        "algorithm",
        "digest length",
        "nonhex digest",
        "missing timestamp",
    ] {
        let (fixture, _) = ready(fixture(), false);
        let mut snapshot = checkpoint(&fixture);
        let receipt = &mut snapshot["jobs"][0]["validation"];
        let mut contract = receipt.get("contract").cloned().unwrap_or_else(|| {
            json!({
                "schema_version":1, "policy_digest":format!("sha256:{}", "a".repeat(64)),
                "completed_at":1,
            })
        });
        match case {
            "version" => contract["schema_version"] = json!(2),
            "algorithm" => contract["policy_digest"] = json!(format!("md5:{}", "a".repeat(64))),
            "digest length" => contract["policy_digest"] = json!("sha256:a"),
            "nonhex digest" => {
                let digest = format!("sha256:{}", "g".repeat(64));
                assert_eq!(digest.len(), 71);
                contract["policy_digest"] = json!(digest);
            }
            "missing timestamp" => {
                contract.as_object_mut().unwrap().remove("completed_at");
            }
            _ => unreachable!(),
        }
        receipt["contract"] = contract;
        assert_snapshot_quarantined(fixture, snapshot, case);
    }
}

#[test]
fn malformed_contract_in_active_or_paused_job_blocks_all_pending_recovery() {
    for phase in ["active", "paused"] {
        let (fixture, id) = ready(fixture(), false);
        let other = create(&fixture.shared);
        prepare(&fixture.shared, &other);
        set_pending(
            &fixture,
            &other,
            Action::Run {
                argv: vec![
                    "sh".into(),
                    fixture.command_script.to_string_lossy().into_owned(),
                    fixture.command_count.to_string_lossy().into_owned(),
                ],
            },
        );
        let mut snapshot = checkpoint(&fixture);
        let jobs = snapshot["jobs"].as_array_mut().unwrap();
        let saved = jobs.iter_mut().find(|job| job["id"] == id).unwrap();
        saved["phase"] = json!(phase);
        // Use a complete JSON contract even before production knows this
        // optional field. Baseline compilation must not depend on new types.
        saved["validation"]["contract"] = json!({
            "schema_version":1,
            "policy_digest":format!("sha256:{}", "g".repeat(64)),
            "completed_at":1,
        });
        // A valid interrupted action precedes the malformed receipt. The
        // entire original snapshot must survive, without partial recovery.
        jobs.rotate_left(1);
        assert_eq!(jobs[0]["id"], other);
        assert!(jobs[0]["pending"].is_object());
        assert_eq!(jobs[1]["phase"], phase);
        assert_snapshot_quarantined(
            fixture,
            snapshot,
            &format!("malformed {phase} contract after a valid pending action"),
        );
    }
}

#[test]
fn active_model_sees_recorded_scope_and_changed_check_settings() {
    let mut fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let model = ScriptedModel::new([
        fix(),
        Action::Verify,
        Action::Checkpoint {
            summary: "Keep inspecting".into(),
            next_step: "Check the new requirements".into(),
        },
    ]);
    action(&fixture.shared, &model);
    action(&fixture.shared, &model);
    assert_eq!(job(&fixture.shared, &id)["phase"], "active");
    fixture
        .shared
        .config
        .engineering
        .as_mut()
        .unwrap()
        .workspaces[0]
        .checks
        .push(vec!["false".into()]);
    action(&fixture.shared, &model);
    model.assert_consumed();
    let contexts = model.contexts.lock().unwrap();
    let verification = &contexts.last().unwrap()["verification"];
    assert_eq!(verification["scope"], "recorded_commit");
    assert_eq!(verification["policy"]["status"], "changed");
    assert_eq!(verification["candidate"]["status"], "not_observed");
    assert_eq!(count(&fixture.validation_count), 1);
}
