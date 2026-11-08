//! Operator review/abort through real CLI processes, without network calls.
use std::path::Path;
use std::process::{Command, Output};

use kamimusuhi_runtime::dialogue::DialogueSession;
use kamimusuhi_runtime::llm_jev::{MockLanguageProvider, RuleBasedDecisionProvider};
use kamimusuhi_runtime::{ResourceImplementation, Runtime, RuntimeOptions, memory_writeback};
use serde_json::Value;

const BINARY: &str = env!("CARGO_BIN_EXE_kamimusuhi-runtime");
const REASON: &str = "operator acknowledges the synthetic failed output surface";

fn run(dir: &Path, command: &str, flags: &[&str]) -> Output {
    Command::new(BINARY)
        .arg(command)
        .arg("--dir")
        .arg(dir)
        .args(flags)
        .output()
        .unwrap()
}

fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "runtime failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn counts(dir: &Path) -> Vec<(String, i64)> {
    let conn = rusqlite::Connection::open(dir.join("kamimusuhi.sqlite")).unwrap();
    [
        "evidence_records",
        "evidence_links",
        "writer_epochs",
        "continuity_heads",
    ]
    .into_iter()
    .map(|table| {
        let count = conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        (table.to_owned(), count)
    })
    .collect()
}

fn incomplete_fixture() -> (tempfile::TempDir, String, Vec<u8>) {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = Runtime::init(
        dir.path(),
        RuntimeOptions::deterministic(0xAB07),
        ResourceImplementation::FakeA,
    )
    .unwrap();
    let mut session = DialogueSession::start_with_providers(
        &mut runtime,
        "synthetic-abort-cli",
        Default::default(),
        Box::new(RuleBasedDecisionProvider),
        Some(Box::new(MockLanguageProvider)),
    )
    .unwrap();
    assert!(
        session
            .turn(&mut runtime, "synthetic test", |_| {
                Err(std::io::Error::other("synthetic surface failure"))
            })
            .is_err()
    );
    let status = memory_writeback::status(&runtime).unwrap();
    assert_eq!(status["unresolved"], 1);
    let turn = status["turn_ids"][0].as_str().unwrap().to_owned();
    let marker = std::fs::read(
        dir.path()
            .join("memory-writeback")
            .join(format!("{turn}.json")),
    )
    .unwrap();
    (dir, turn, marker)
}

#[test]
fn operator_review_abort_and_retry_preserve_the_original_marker() {
    let (dir, turn, marker) = incomplete_fixture();
    let before = counts(dir.path());
    let review = success(run(
        dir.path(),
        "writeback-review",
        &["--turn-id", &turn, "--reason", REASON],
    ));
    assert_eq!(review["state"], "abort_review");
    assert_eq!(counts(dir.path()), before, "review wrote canonical records");
    let token = review["review_token"].as_str().unwrap();
    let flags = [
        "--turn-id",
        &turn,
        "--reason",
        REASON,
        "--review-token",
        token,
    ];
    let aborted = success(run(dir.path(), "writeback-abort", &flags));
    assert_eq!(aborted["state"], "aborted");
    let after = counts(dir.path());
    assert_eq!(after[0].1, before[0].1 + 1, "one failure audit event");
    assert!(
        after[1].1 > before[1].1,
        "audit has original evidence lineage"
    );
    assert_eq!(after[2..], before[2..], "no writer epoch or head mutation");
    let repeated = success(run(dir.path(), "writeback-abort", &flags));
    assert_eq!(repeated["state"], "aborted");
    assert_eq!(counts(dir.path()), after, "retry duplicated audit records");
    let memory = success(run(dir.path(), "memory", &[]));
    assert_eq!(memory["memory_writeback"]["unresolved"], 0);
    assert_eq!(memory["memory_writeback"]["aborted"], 1);
    assert_eq!(
        std::fs::read(
            dir.path()
                .join("memory-writeback")
                .join(format!("{turn}.json"))
        )
        .unwrap(),
        marker
    );
    let conn = rusqlite::Connection::open(dir.path().join("kamimusuhi.sqlite")).unwrap();
    let events: i64 = conn
        .query_row(
            "SELECT count(*) FROM evidence_records WHERE turn_id=? AND json_extract(payload_json, '$.event')='memory_writeback_aborted'",
            [&turn],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(events, 1);
    let successes: i64 = conn
        .query_row(
            "SELECT count(*) FROM evidence_records WHERE turn_id=? AND json_extract(payload_json, '$.event')='memory_writeback_verified'",
            [&turn],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(successes, 0, "failed turn was not relabelled successful");
}

#[test]
fn wrong_token_and_changed_reason_do_not_write() {
    let (dir, turn, marker) = incomplete_fixture();
    let review = success(run(
        dir.path(),
        "writeback-review",
        &["--turn-id", &turn, "--reason", REASON],
    ));
    let token = review["review_token"].as_str().unwrap();
    let before = counts(dir.path());
    for flags in [
        vec![
            "--turn-id",
            &turn,
            "--reason",
            REASON,
            "--review-token",
            "incorrect",
        ],
        vec![
            "--turn-id",
            &turn,
            "--reason",
            "changed reason",
            "--review-token",
            token,
        ],
    ] {
        let output = run(dir.path(), "writeback-abort", &flags);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(counts(dir.path()), before);
    }
    assert_eq!(
        std::fs::read(
            dir.path()
                .join("memory-writeback")
                .join(format!("{turn}.json"))
        )
        .unwrap(),
        marker
    );
}

#[test]
fn malformed_or_mixed_operator_options_are_rejected_before_runtime_open() {
    let dir = tempfile::tempdir().unwrap();
    let absent = dir.path().join("must-not-be-created");
    let turn = "00000000000000000000000000000001";
    let cases = [
        ("writeback-review", vec!["--reason", REASON]),
        ("writeback-review", vec!["--turn-id", turn]),
        (
            "writeback-review",
            vec!["--turn-id", "invalid", "--reason", REASON],
        ),
        ("writeback-review", vec!["--turn-id", turn, "--reason", " "]),
        (
            "writeback-review",
            vec![
                "--turn-id",
                turn,
                "--reason",
                REASON,
                "--review-token",
                "unused",
            ],
        ),
        (
            "writeback-abort",
            vec!["--turn-id", turn, "--reason", REASON],
        ),
        (
            "writeback-abort",
            vec!["--turn-id", turn, "--reason", REASON, "--review-token", ""],
        ),
        (
            "writeback-review",
            vec![
                "--turn-id",
                turn,
                "--reason",
                REASON,
                "--message",
                "must not run",
            ],
        ),
        (
            "writeback-review",
            vec![
                "--turn-id",
                turn,
                "--reason",
                REASON,
                "--subject",
                "must-not-run",
            ],
        ),
        (
            "writeback-review",
            vec![
                "--turn-id",
                turn,
                "--reason",
                REASON,
                "--param",
                "retrieval.history_messages",
            ],
        ),
        (
            "writeback-review",
            vec!["--turn-id", turn, "--reason", REASON, "--value", "2"],
        ),
        (
            "writeback-review",
            vec![
                "--turn-id",
                turn,
                "--reason",
                REASON,
                "--privacy",
                "unconstrained",
            ],
        ),
        ("memory", vec!["--turn-id", turn]),
    ];
    for (command, flags) in cases {
        let output = run(&absent, command, &flags);
        assert_eq!(output.status.code(), Some(2), "{command} {flags:?}");
        assert!(output.stdout.is_empty());
        assert!(!absent.exists(), "invalid CLI created a runtime directory");
    }
}
