//! Claim identities survive uncertain outcomes, migration and history pruning.

use super::*;

fn as_legacy_checkpoint(fixture: &Fixture) -> Value {
    let mut snapshot = checkpoint(fixture);
    snapshot["schema_version"] = json!(1);
    for saved in snapshot["jobs"].as_array_mut().unwrap() {
        saved
            .as_object_mut()
            .unwrap()
            .remove("next_action_sequence");
    }
    snapshot
}

fn legacy_write_intent(saved: &mut Value, path: &str, content: &str) -> u64 {
    let sequence = saved["metrics"]["actions"].as_u64().unwrap() + 1;
    saved["metrics"]["decisions"] = json!(
        saved["metrics"]["decisions"]
            .as_u64()
            .unwrap()
            .max(sequence)
    );
    saved["phase"] = json!("active");
    saved["pending"] = json!({
        "sequence":sequence,"started_at":1,
        "action":{"action":"write","path":path,"content":content,"expected":null}
    });
    sequence
}

fn latest_read_sequence(shared: &Shared, id: &str) -> u64 {
    job(shared, id)["observations"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|record| record["action"] == "read")
        .unwrap()["sequence"]
        .as_u64()
        .unwrap()
}

fn read_log_sequences(fixture: &Fixture, id: &str) -> Vec<u64> {
    let directory = fixture.shared.spool.root().join("logs/engineering/test");
    let mut paths: Vec<_> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    paths.sort();
    paths
        .into_iter()
        .flat_map(|path| {
            std::fs::read_to_string(path)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .collect::<Vec<_>>()
        })
        .filter(|record| record["job_id"] == id && record["action"] == "read")
        .map(|record| record["sequence"].as_u64().unwrap())
        .collect()
}

#[test]
fn an_interrupted_write_and_the_next_read_have_distinct_action_sequences() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let original = job(&fixture.shared, &id);
    let original_bytes = std::fs::read(fixture.workspace.join("code.txt")).unwrap();
    let path = candidate(&fixture.shared, &id).join("recovered.txt");
    let written = "the effect completed before its receipt\n";
    std::fs::write(&path, written).unwrap();
    set_pending(
        &fixture,
        &id,
        Action::Write {
            path: "recovered.txt".to_owned(),
            content: written.to_owned(),
            expected: None,
        },
    );
    let uncertain_sequence = checkpoint(&fixture)["jobs"][0]["pending"]["sequence"]
        .as_u64()
        .unwrap();
    let fixture = restart(fixture);
    let recovered = job(&fixture.shared, &id);
    assert_eq!(recovered["candidate"], original["candidate"]);
    assert_eq!(recovered["phase"], "active");
    assert!(recovered["pending"].is_null());
    let unknown = recovered["observations"]
        .as_array()
        .unwrap()
        .last()
        .unwrap();
    assert_eq!(unknown["sequence"], uncertain_sequence);
    assert_eq!(unknown["detail"]["sequence"], uncertain_sequence);
    let model = ScriptedModel::new([Action::Read {
        path: "recovered.txt".to_owned(),
    }]);
    action(&fixture.shared, &model);
    let read_sequence = latest_read_sequence(&fixture.shared, &id);
    assert!(read_sequence > uncertain_sequence);
    assert_eq!(read_sequence, 2);
    assert_eq!(std::fs::read_to_string(path).unwrap(), written);
    assert_eq!(
        std::fs::read(fixture.workspace.join("code.txt")).unwrap(),
        original_bytes
    );
    let completed = job(&fixture.shared, &id);
    assert_eq!(completed["metrics"]["decisions"], 2);
    assert_eq!(completed["metrics"]["actions"], 1);
    assert_eq!(completed["metrics"]["interruptions"], 1);
    assert_eq!(completed["next_action_sequence"], 3);
    assert_eq!(read_log_sequences(&fixture, &id), vec![read_sequence]);
    assert_eq!(model.calls(), 1);
    assert_eq!(count(&fixture.command_count), 0);
    assert_eq!(count(&fixture.validation_count), 0);
    model.assert_consumed();
}

#[test]
fn v1_migration_preserves_old_ids_and_reserves_above_every_legacy_sequence_source() {
    for (source, expected_cursor) in [
        ("decisions", 8_u64),
        ("observation", 14),
        ("unknown", 18),
        ("no_pending", 8),
    ] {
        let fixture = fixture();
        let id = create(&fixture.shared);
        prepare(&fixture.shared, &id);
        let path = candidate(&fixture.shared, &id).join("legacy-effect.txt");
        std::fs::write(&path, "preserved legacy effect\n").unwrap();
        let mut snapshot = as_legacy_checkpoint(&fixture);
        let saved = &mut snapshot["jobs"][0];
        saved["request_id"] = json!(format!("legacy-request-{source}"));
        saved["checkpoint"] = json!({
            "summary":"既存の調査と未コミット成果を保つ",
            "next_step":"候補を読み直して次の行動を決める"
        });
        saved["metrics"]["actions"] = json!(2);
        saved["metrics"]["decisions"] = json!(7);
        // v1 allowed control events to share old completed-action numbers.
        // Retain these historical identities rather than renumbering them.
        saved["observations"] = json!([
            {"sequence":2,"action":"read","ok":true,"detail":{"bytes":3}},
            {"sequence":2,"action":"model","ok":false,"detail":{"error":"old failure"}}
        ]);
        if source == "observation" {
            saved["observations"][1]["sequence"] = json!(13);
        } else if source == "unknown" {
            saved["observations"][1] = json!({
                "sequence":2,"action":"write","ok":false,
                "detail":{"kind":"interrupted_action_outcome_unknown","sequence":17}
            });
        }
        let pending_sequence = (source != "no_pending")
            .then(|| legacy_write_intent(saved, "legacy-effect.txt", "preserved legacy effect\n"));
        let before = saved.clone();
        let old_bytes = write_checkpoint(&fixture, &snapshot);
        let fixture = restart(fixture);
        assert!(
            fixture.shared.engineering.summary()["error"].is_null(),
            "{source}"
        );
        let migrated = job(&fixture.shared, &id);
        for field in [
            "id",
            "request_id",
            "objective",
            "workspace",
            "candidate",
            "checkpoint",
        ] {
            assert_eq!(migrated[field], before[field], "{source}: {field}");
        }
        assert_eq!(migrated["phase"], "active");
        assert!(migrated["pending"].is_null());
        assert_eq!(
            migrated["next_action_sequence"], expected_cursor,
            "{source}"
        );
        assert_eq!(migrated["metrics"]["actions"], 2);
        assert_eq!(migrated["metrics"]["decisions"], 7);
        let observations = migrated["observations"].as_array().unwrap();
        assert_eq!(
            &observations[..2],
            before["observations"].as_array().unwrap()
        );
        if let Some(pending_sequence) = pending_sequence {
            assert_eq!(pending_sequence, 3);
            assert_eq!(observations.len(), 3);
            assert_eq!(observations[2]["sequence"], pending_sequence);
            assert_eq!(observations[2]["detail"]["sequence"], pending_sequence);
        } else {
            assert_eq!(observations.len(), 2);
            assert_eq!(migrated["metrics"]["interruptions"], 0);
        }
        assert_eq!(checkpoint(&fixture)["schema_version"], 2);
        let migrated_bytes = std::fs::read(checkpoint_path(&fixture)).unwrap();
        assert_ne!(migrated_bytes, old_bytes);
        let fixture = restart(fixture);
        assert_eq!(
            std::fs::read(checkpoint_path(&fixture)).unwrap(),
            migrated_bytes
        );
        let model = ScriptedModel::new([Action::Read {
            path: "legacy-effect.txt".to_owned(),
        }]);
        action(&fixture.shared, &model);
        assert_eq!(latest_read_sequence(&fixture.shared, &id), expected_cursor);
        assert_eq!(read_log_sequences(&fixture, &id), vec![expected_cursor]);
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "preserved legacy effect\n"
        );
        assert_eq!(count(&fixture.command_count), 0);
        assert_eq!(count(&fixture.validation_count), 0);
        model.assert_consumed();
    }
}

#[test]
fn v2_keeps_a_high_cursor_after_history_pruning_and_control_observations() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let mut snapshot = checkpoint(&fixture);
    snapshot["jobs"][0]["observations"] = json!([]);
    snapshot["jobs"][0]["next_action_sequence"] = json!(1000);
    write_checkpoint(&fixture, &snapshot);
    let fixture = restart(fixture);
    let failure = ScriptedModel::new([]);
    action(&fixture.shared, &failure);
    assert_eq!(failure.calls(), 1);
    let failed = job(&fixture.shared, &id);
    assert_eq!(failed["next_action_sequence"], 1000);
    assert_eq!(failed["metrics"]["actions"], 0);
    assert_eq!(failed["observations"][0]["sequence"], 0);
    assert_eq!(failed["observations"][0]["action"], "model");
    // Move the saved retry due time without sleeping; this is a state-load
    // test, not a measurement of provider scheduling time.
    let mut snapshot = checkpoint(&fixture);
    snapshot["jobs"][0]["next_attempt_at"] = json!(0);
    write_checkpoint(&fixture, &snapshot);
    let fixture = restart(fixture);
    let model = ScriptedModel::new([Action::Read {
        path: "code.txt".to_owned(),
    }]);
    action(&fixture.shared, &model);
    assert_eq!(latest_read_sequence(&fixture.shared, &id), 1000);
    assert_eq!(job(&fixture.shared, &id)["next_action_sequence"], 1001);
    assert_eq!(job(&fixture.shared, &id)["metrics"]["decisions"], 1);
    let bytes = std::fs::read(checkpoint_path(&fixture)).unwrap();
    let fixture = restart(fixture);
    assert_eq!(std::fs::read(checkpoint_path(&fixture)).unwrap(), bytes);
    let model = ScriptedModel::new([Action::Read {
        path: "notes.txt".to_owned(),
    }]);
    action(&fixture.shared, &model);
    assert_eq!(latest_read_sequence(&fixture.shared, &id), 1001);
    assert_eq!(read_log_sequences(&fixture, &id), vec![1000, 1001]);
    assert_eq!(count(&fixture.command_count), 0);
    assert_eq!(count(&fixture.validation_count), 0);
}

#[test]
fn v2_rejects_missing_zero_past_and_inconsistent_pending_cursors() {
    for case in [
        "missing",
        "zero",
        "past",
        "pending_same",
        "pending_gap",
        "no_decision",
    ] {
        let fixture = fixture();
        let id = create(&fixture.shared);
        prepare(&fixture.shared, &id);
        let model = ScriptedModel::new([Action::Read {
            path: "code.txt".to_owned(),
        }]);
        action(&fixture.shared, &model);
        let mut snapshot = checkpoint(&fixture);
        let saved = &mut snapshot["jobs"][0];
        match case {
            "missing" => {
                saved
                    .as_object_mut()
                    .unwrap()
                    .remove("next_action_sequence");
            }
            "zero" => saved["next_action_sequence"] = json!(0),
            "past" => saved["next_action_sequence"] = json!(1),
            _ => {
                saved["pending"] = json!({
                    "sequence":2,"started_at":1,
                    "action":{"action":"read","path":"code.txt"}
                });
                saved["metrics"]["decisions"] = json!(2);
                saved["next_action_sequence"] = json!(3);
                match case {
                    "pending_same" => saved["next_action_sequence"] = json!(2),
                    "pending_gap" => saved["next_action_sequence"] = json!(4),
                    "no_decision" => saved["metrics"]["decisions"] = json!(1),
                    _ => unreachable!(),
                }
            }
        }
        assert_snapshot_quarantined(fixture, snapshot, case);
    }
}

#[test]
fn v1_migration_is_atomic_when_a_later_job_is_invalid_or_overflows() {
    for case in ["invalid_later_job", "overflow_later_job"] {
        let fixture = fixture();
        let first = create(&fixture.shared);
        prepare(&fixture.shared, &first);
        let second = create(&fixture.shared);
        assert_ne!(first, second);
        let mut snapshot = as_legacy_checkpoint(&fixture);
        legacy_write_intent(&mut snapshot["jobs"][0], "pending.txt", "unconfirmed\n");
        if case == "invalid_later_job" {
            snapshot["jobs"][1]["phase"] = json!("ready");
        } else {
            snapshot["jobs"][1]["metrics"]["decisions"] = json!(u64::MAX);
        }
        assert_snapshot_quarantined(fixture, snapshot, case);
    }
}

#[test]
fn an_exhausted_cursor_pauses_before_inference_without_starving_another_job() {
    let fixture = fixture();
    let exhausted = create(&fixture.shared);
    prepare(&fixture.shared, &exhausted);
    let other = create(&fixture.shared);
    let mut snapshot = checkpoint(&fixture);
    snapshot["jobs"][0]["next_action_sequence"] = json!(u64::MAX);
    snapshot["jobs"][0]["updated_at"] = json!(0);
    let original = snapshot["jobs"][0]["candidate"].clone();
    write_checkpoint(&fixture, &snapshot);
    let fixture = restart(fixture);
    assert!(fixture.shared.engineering.summary()["error"].is_null());
    let model = ScriptedModel::new([Action::Read {
        path: "code.txt".to_owned(),
    }]);
    let paused = tick_with(&fixture.shared, &model).unwrap().unwrap();
    assert_eq!(paused.id, exhausted);
    assert_eq!(paused.phase, Phase::Paused);
    assert_eq!(model.calls(), 0);
    let exhausted_job = job(&fixture.shared, &exhausted);
    assert_eq!(exhausted_job["metrics"]["actions"], 0);
    assert_eq!(exhausted_job["next_action_sequence"], u64::MAX);
    assert_eq!(exhausted_job["candidate"], original);
    assert!(exhausted_job["pending"].is_null());
    let last = exhausted_job["observations"]
        .as_array()
        .unwrap()
        .last()
        .unwrap();
    assert_eq!(last["sequence"], 0);
    assert!(last["detail"].to_string().contains("exhausted"));
    prepare(&fixture.shared, &other);
    action(&fixture.shared, &model);
    assert_eq!(model.calls(), 1);
    assert_eq!(job(&fixture.shared, &other)["metrics"]["actions"], 1);
    assert_eq!(job(&fixture.shared, &exhausted)["phase"], "paused");
    assert_eq!(count(&fixture.command_count), 0);
    assert_eq!(count(&fixture.validation_count), 0);
    model.assert_consumed();
}

#[test]
fn legacy_unknown_receipts_with_invalid_sequences_are_preserved_and_quarantined() {
    for case in ["missing", "zero", "text", "negative"] {
        let fixture = fixture();
        let id = create(&fixture.shared);
        prepare(&fixture.shared, &id);
        let mut snapshot = as_legacy_checkpoint(&fixture);
        let detail = match case {
            "missing" => json!({"kind":"interrupted_action_outcome_unknown"}),
            "zero" => json!({"kind":"interrupted_action_outcome_unknown","sequence":0}),
            "text" => json!({"kind":"interrupted_action_outcome_unknown","sequence":"1"}),
            "negative" => json!({"kind":"interrupted_action_outcome_unknown","sequence":-1}),
            _ => unreachable!(),
        };
        snapshot["jobs"][0]["observations"] = json!([
            {"sequence":0,"action":"write","ok":false,"detail":detail}
        ]);
        assert_snapshot_quarantined(fixture, snapshot, case);
    }
}
