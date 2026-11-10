//! Native context contracts: the observation payload has its own encoded
//! budget, and working memory survives independently of the history window.

use super::*;

fn assert_context_identity(context: &Value, saved: &Value, policy: &WorkspaceConfig) {
    assert_eq!(context["objective"], saved["objective"]);
    assert_eq!(context["workspace"], saved["workspace"]);
    assert_eq!(context["candidate"], saved["candidate"]);
    assert_eq!(
        context["checkpoint"]["self_authored_unverified"],
        saved["checkpoint"]
    );
    assert_eq!(context["configured_checks"], json!(policy.checks));
    assert_eq!(context["allowed_programs"], json!(policy.commands));
}

#[test]
fn native_context_keeps_complete_range_pages_within_the_encoded_observation_budget() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let original_head = git(&fixture.workspace, &["rev-parse", "HEAD"]);
    let policy = fixture
        .shared
        .config
        .engineering
        .as_ref()
        .unwrap()
        .workspaces[0]
        .clone();
    // Each ten-line page fits the range action's 6 KiB detail budget, but
    // escaping that JSON as external_data makes its outer encoding larger.
    // Distinct line labels detect misplaced content as well as broken JSON.
    let source: String = (1..=60)
        .map(|line| format!("line{line:02}: {}\r\n", "\\\"".repeat(145)))
        .collect();
    let source_lines: Vec<_> = source.split_inclusive('\n').collect();
    assert_eq!(source_lines.len(), 60);
    std::fs::write(candidate(&fixture.shared, &id).join("pages.txt"), &source).unwrap();

    let mut actions = vec![Action::Checkpoint {
        summary: "観測の行末と継続位置を保ち、元の作業ツリーを変更しない。".to_owned(),
        next_step: "pages.txtの範囲を調べ、必要な箇所だけを修正する。".to_owned(),
    }];
    for start_line in [1, 11, 21, 31, 41, 51] {
        actions.push(Action::ReadRange {
            path: "pages.txt".to_owned(),
            start_line,
            max_lines: 10,
        });
    }
    // This decision receives the six completed range observations.
    actions.push(Action::Read {
        path: "notes.txt".to_owned(),
    });
    let model = ScriptedModel::new(actions);
    action(&fixture.shared, &model);
    let remembered = job(&fixture.shared, &id);
    for _ in 0..7 {
        action(&fixture.shared, &model);
    }
    model.assert_consumed();
    assert_eq!(model.calls(), 8);
    let context = model.contexts.lock().unwrap().last().unwrap().clone();
    let observations = context["observations"].as_array().unwrap();
    let encoded_bytes = serde_json::to_vec(&context["observations"]).unwrap().len();
    let ranges: Vec<_> = observations
        .iter()
        .filter(|record| record["action"] == "read_range")
        .collect();
    let parsed: Vec<Option<Value>> = ranges
        .iter()
        .map(|record| {
            record["external_data"]
                .as_str()
                .and_then(|text| serde_json::from_str(text).ok())
        })
        .collect();
    let complete_ranges = parsed.iter().filter(|range| range.is_some()).count();
    let latest_kept = parsed
        .iter()
        .flatten()
        .any(|range| range["start_line"] == 51);
    // Emit both before/after measurements before the first failing assert.
    // This measures only the observations array, not the complete request.
    println!(
        "native context payload: source_bytes={} observations_encoded_bytes={} range_pages={} complete_range_pages={} latest_range_kept={}",
        source.len(),
        encoded_bytes,
        ranges.len(),
        complete_ranges,
        latest_kept
    );
    assert!(
        encoded_bytes <= 32_768,
        "encoded observation array exceeds 32 KiB: {encoded_bytes} bytes"
    );
    assert!(
        latest_kept,
        "the latest complete page must remain available"
    );
    assert_eq!(complete_ranges, ranges.len());
    assert_eq!(
        context["observations_omitted"].as_u64().unwrap()
            + u64::try_from(observations.len()).unwrap(),
        8,
        "prepare, checkpoint and six ranges are the stored observations before this decision"
    );
    let retained_starts: Vec<u64> = parsed
        .iter()
        .flatten()
        .map(|range| range["start_line"].as_u64().unwrap())
        .collect();
    assert!(retained_starts.windows(2).all(|pair| pair[0] < pair[1]));
    for (record, range) in ranges.iter().zip(parsed) {
        assert_eq!(record["ok"], true);
        assert_eq!(record["context_truncated"], false);
        let range = range.expect("retained range must remain complete JSON");
        assert_eq!(range["path"], "pages.txt");
        assert_eq!(range["total_lines"], 60);
        let start = usize::try_from(range["start_line"].as_u64().unwrap()).unwrap();
        let end = usize::try_from(range["end_line"].as_u64().unwrap()).unwrap();
        assert!([1, 11, 21, 31, 41, 51].contains(&start));
        assert_eq!(end, start + 9);
        assert_eq!(range["content"], source_lines[start - 1..end].concat());
        let next_line = (end < source_lines.len()).then_some(end + 1);
        assert_eq!(range["next_line"], json!(next_line));
        assert_eq!(range["truncated"], next_line.is_some());
    }
    assert_context_identity(&context, &remembered, &policy);
    assert_eq!(job(&fixture.shared, &id)["metrics"]["actions"], 8);
    assert_eq!(count(&fixture.validation_count), 0);
    assert_eq!(count(&fixture.command_count), 0);
    assert_eq!(
        git(&fixture.workspace, &["rev-parse", "HEAD"]),
        original_head
    );
    assert!(!fixture.workspace.join("pages.txt").exists());
    assert_eq!(
        std::fs::read_to_string(candidate(&fixture.shared, &id).join("pages.txt")).unwrap(),
        source
    );
}

#[test]
fn working_memory_survives_fourteen_observations_and_reload() {
    let fixture = fixture();
    let id = create(&fixture.shared);
    prepare(&fixture.shared, &id);
    let policy = fixture
        .shared
        .config
        .engineering
        .as_ref()
        .unwrap()
        .workspaces[0]
        .clone();
    let reads = 14;
    let mut actions = vec![Action::Checkpoint {
        summary: "元の作業ツリーを保持する。検証は実行結果で判断する。".to_owned(),
        next_step: "保存済みの候補でcode.txtを調べ、検証失敗の原因を修正する。".to_owned(),
    }];
    actions.extend((0..reads).map(|_| Action::Read {
        path: "notes.txt".to_owned(),
    }));
    let model = ScriptedModel::new(actions);
    action(&fixture.shared, &model);
    let remembered = job(&fixture.shared, &id);
    for _ in 0..reads {
        action(&fixture.shared, &model);
    }
    model.assert_consumed();
    assert_eq!(model.calls(), reads + 1);
    let before = job(&fixture.shared, &id);
    let observations = before["observations"].as_array().unwrap();
    assert_eq!(observations.len(), KEEP_OBSERVATIONS);
    assert!(
        observations.iter().all(|record| record["action"] == "read"),
        "prepare and checkpoint observations must have rolled out"
    );
    assert_eq!(before["metrics"]["actions"], reads + 1);
    assert_eq!(before["checkpoint"], remembered["checkpoint"]);
    assert_eq!(
        checkpoint(&fixture)["jobs"][0]["checkpoint"],
        remembered["checkpoint"],
        "working memory is independent of the durable observation window"
    );
    let checkpoint_bytes = std::fs::read(checkpoint_path(&fixture)).unwrap();
    assert_eq!(count(&fixture.validation_count), 0);
    assert_eq!(count(&fixture.command_count), 0);

    // This is a durable-state reload, not a daemon SIGKILL/restart test.
    let fixture = restart(fixture);
    let restored = job(&fixture.shared, &id);
    assert_eq!(restored, before);
    assert_eq!(
        std::fs::read(checkpoint_path(&fixture)).unwrap(),
        checkpoint_bytes
    );
    assert_eq!(count(&fixture.validation_count), 0);
    assert_eq!(count(&fixture.command_count), 0);
    let resumed_model = ScriptedModel::new([Action::Read {
        path: "code.txt".to_owned(),
    }]);
    action(&fixture.shared, &resumed_model);
    resumed_model.assert_consumed();
    assert_eq!(resumed_model.calls(), 1);
    let context = resumed_model.contexts.lock().unwrap()[0].clone();
    assert_context_identity(&context, &remembered, &policy);
    assert_eq!(context["metrics"]["actions"], reads + 1);
    assert!(
        context["observations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|record| record["action"] != "checkpoint"),
        "memory is present without its original checkpoint observation"
    );
    assert_eq!(job(&fixture.shared, &id)["metrics"]["actions"], reads + 2);
    assert_eq!(count(&fixture.validation_count), 0);
    assert_eq!(count(&fixture.command_count), 0);
    println!(
        "native memory retention: new_observations={} retained_observations={} summary_preserved=true next_step_preserved=true reload_extra_actions=0 resumed_decisions=1",
        reads,
        observations.len()
    );
}
