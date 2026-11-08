//! Operator recovery must retain failure evidence, never certify delivery.
use std::path::{Path, PathBuf};

use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceSource, EvidenceStore, NewEvidence, RetentionClass,
};
use kamimusuhi_core::ids::{EvidenceId, TurnId};
use kamimusuhi_core::mutation::OriginClass;
use kamimusuhi_runtime::dialogue::DialogueSession;
use kamimusuhi_runtime::llm_jev::{MockLanguageProvider, RuleBasedDecisionProvider};
use kamimusuhi_runtime::{ResourceImplementation, Runtime, RuntimeOptions, memory_writeback};
use serde_json::{Value, json};

const REASON: &str = "operator confirmed the synthetic failed attempt; not a successful writeback";

fn session(runtime: &mut Runtime) -> DialogueSession {
    DialogueSession::start_with_providers(
        runtime,
        "abort-fixture",
        Default::default(),
        Box::new(RuleBasedDecisionProvider),
        Some(Box::new(MockLanguageProvider)),
    )
    .unwrap()
}

fn init(path: &Path) -> Runtime {
    Runtime::init(
        path,
        RuntimeOptions::deterministic(0xAB071),
        ResourceImplementation::FakeA,
    )
    .unwrap()
}

fn pending(path: &Path) -> (PathBuf, Value, Vec<u8>) {
    let paths: Vec<_> = std::fs::read_dir(path.join("memory-writeback"))
        .unwrap()
        .map(|p| p.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    assert_eq!(paths.len(), 1);
    let bytes = std::fs::read(&paths[0]).unwrap();
    (
        paths[0].clone(),
        serde_json::from_slice(&bytes).unwrap(),
        bytes,
    )
}

fn failed(path: &Path) -> (Runtime, TurnId) {
    let mut runtime = init(path);
    let mut chat = session(&mut runtime);
    assert!(
        chat.turn(&mut runtime, "synthetic failed output", |_| {
            Err(std::io::Error::other("injected surface failure"))
        })
        .is_err()
    );
    let (_, marker, _) = pending(path);
    (
        runtime,
        marker["turn_id"].as_str().unwrap().parse().unwrap(),
    )
}

fn rows(path: &Path) -> Vec<(String, String)> {
    let c = rusqlite::Connection::open(path.join("kamimusuhi.sqlite")).unwrap();
    c.prepare("SELECT evidence_id,payload_json FROM evidence_records ORDER BY rowid")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn token(runtime: &Runtime, turn: TurnId) -> String {
    memory_writeback::review_abort(runtime, turn, REASON).unwrap()["review_token"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn abort_keeps_marker_and_original_records_and_survives_retry_and_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, turn) = failed(dir.path());
    let (path, marker, bytes) = pending(dir.path());
    let original = rows(dir.path());
    let head = runtime.head().unwrap();
    let reviewed = token(&runtime, turn);
    assert_eq!(
        rows(dir.path()),
        original,
        "review must not append evidence"
    );
    let result = memory_writeback::abort(&runtime, turn, REASON, &reviewed).unwrap();
    assert_eq!(result["state"], "aborted");
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let after = rows(dir.path());
    assert_eq!(&after[..original.len()], original.as_slice());
    assert_eq!(after.len(), original.len() + 1);
    let audit = runtime
        .store()
        .get(marker["completion_id"].as_str().unwrap().parse().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(audit.payload["event"], "memory_writeback_aborted");
    assert_eq!(audit.payload["reason"], REASON);
    assert_eq!(audit.turn_id, Some(turn));
    assert!(
        !runtime
            .store()
            .links_from(audit.evidence_id)
            .unwrap()
            .is_empty()
    );
    assert_eq!(runtime.head().unwrap(), head);
    memory_writeback::abort(&runtime, turn, REASON, &reviewed).unwrap();
    assert_eq!(rows(dir.path()), after, "retry must not add a second event");
    assert!(memory_writeback::abort(&runtime, turn, "different reason", &reviewed).is_err());
    drop(runtime);
    let reopened = Runtime::open(dir.path(), RuntimeOptions::deterministic(0xAB072)).unwrap();
    let status = memory_writeback::status(&reopened).unwrap();
    assert_eq!(status["unresolved"], 0);
    assert_eq!(status["aborted"], 1);
    assert_eq!(status["aborted_turn_ids"], json!([turn]));
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn active_shared_lease_and_actual_emit_callback_prevent_abort() {
    let dir = tempfile::tempdir().unwrap();
    let (mut runtime, turn) = failed(dir.path());
    let reviewer = Runtime::open(dir.path(), RuntimeOptions::deterministic(0xAB073)).unwrap();
    let reviewed = token(&reviewer, turn);
    let before = rows(dir.path());
    let lease = runtime.lock_identity().unwrap();
    assert!(memory_writeback::review_abort(&reviewer, turn, REASON).is_err());
    assert!(memory_writeback::abort(&reviewer, turn, REASON, &reviewed).is_err());
    assert_eq!(rows(dir.path()), before);
    drop(lease);
    let mut chat = session(&mut runtime);
    let mut checked = false;
    chat.turn(&mut runtime, "a real active turn", |_| {
        assert!(memory_writeback::abort(&reviewer, turn, REASON, &reviewed).is_err());
        checked = true;
        Ok(())
    })
    .unwrap();
    assert!(checked);
    memory_writeback::abort(&reviewer, turn, REASON, &reviewed).unwrap();
    assert_eq!(
        memory_writeback::status(&reviewer).unwrap()["unresolved"],
        0
    );
}

#[test]
fn changed_turn_evidence_invalidates_review_without_losing_failure() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, turn) = failed(dir.path());
    let (_, marker, bytes) = pending(dir.path());
    let reviewed = token(&runtime, turn);
    runtime
        .store()
        .append(NewEvidence {
            evidence_id: EvidenceId::generate(runtime.ids().as_ref()),
            individual_id: runtime.individual_id(),
            session_id: Some(marker["session_id"].as_str().unwrap().parse().unwrap()),
            turn_id: Some(turn),
            kind: EvidenceKind::SystemEvent,
            origin_class: OriginClass::Observed,
            payload: json!({"event":"additional_failure_observation"}),
            source: EvidenceSource::default(),
            retention_class: RetentionClass::Standard,
        })
        .unwrap();
    let before = rows(dir.path());
    assert!(memory_writeback::abort(&runtime, turn, REASON, &reviewed).is_err());
    assert_eq!(rows(dir.path()), before);
    assert_eq!(pending(dir.path()).2, bytes);
    assert_eq!(memory_writeback::status(&runtime).unwrap()["unresolved"], 1);
    let new_review = token(&runtime, turn);
    assert_ne!(new_review, reviewed);
    memory_writeback::abort(&runtime, turn, REASON, &new_review).unwrap();
}

#[test]
fn emitted_but_unverified_turn_cannot_be_abandoned_as_an_unemitted_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());
    let sql = rusqlite::Connection::open(dir.path().join("kamimusuhi.sqlite")).unwrap();
    sql.execute_batch("CREATE TRIGGER stop_completion BEFORE INSERT ON evidence_records WHEN json_extract(NEW.payload_json,'$.event')='memory_writeback_verified' BEGIN SELECT RAISE(FAIL,'injected completion failure'); END;").unwrap();
    let mut chat = session(&mut runtime);
    assert!(
        chat.turn(&mut runtime, "synthetic emitted turn", |_| Ok(()))
            .is_err()
    );
    let (_, marker, bytes) = pending(dir.path());
    let turn = marker["turn_id"].as_str().unwrap().parse().unwrap();
    let before = rows(dir.path());
    assert!(memory_writeback::review_abort(&runtime, turn, REASON).is_err());
    assert!(memory_writeback::abort(&runtime, turn, REASON, "anything").is_err());
    assert_eq!(rows(dir.path()), before);
    assert_eq!(pending(dir.path()).2, bytes);
}

#[test]
fn verified_completion_cannot_be_reclassified_by_restored_crash_marker() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());
    let mut chat = session(&mut runtime);
    let mut captured = None;
    let reply = chat
        .turn(&mut runtime, "synthetic completed turn", |_| {
            captured = Some(pending(dir.path()));
            Ok(())
        })
        .unwrap();
    let (path, _, bytes) = captured.unwrap();
    std::fs::write(path, bytes).unwrap();
    let before = rows(dir.path());
    assert!(memory_writeback::review_abort(&runtime, reply.turn_id, REASON).is_err());
    assert_eq!(rows(dir.path()), before);
    let status = memory_writeback::status(&runtime).unwrap();
    assert_eq!(status["unresolved"], 0);
    assert_eq!(status["aborted"], 0);
}

#[test]
fn failed_audit_link_write_rolls_back_and_does_not_resolve_marker() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, turn) = failed(dir.path());
    let reviewed = token(&runtime, turn);
    let before = rows(dir.path());
    let bytes = pending(dir.path()).2;
    let sql = rusqlite::Connection::open(dir.path().join("kamimusuhi.sqlite")).unwrap();
    sql.execute_batch("CREATE TRIGGER stop_abort_links BEFORE INSERT ON evidence_links BEGIN SELECT RAISE(FAIL,'injected abort lineage failure'); END;").unwrap();
    assert!(memory_writeback::abort(&runtime, turn, REASON, &reviewed).is_err());
    assert_eq!(rows(dir.path()), before);
    assert_eq!(pending(dir.path()).2, bytes);
    assert_eq!(memory_writeback::status(&runtime).unwrap()["unresolved"], 1);
    sql.execute_batch("DROP TRIGGER stop_abort_links").unwrap();
    memory_writeback::abort(&runtime, turn, REASON, &reviewed).unwrap();
}

#[test]
fn tampered_abort_audit_or_marker_is_not_reported_as_resolved() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, turn) = failed(dir.path());
    let reviewed = token(&runtime, turn);
    memory_writeback::abort(&runtime, turn, REASON, &reviewed).unwrap();
    let (path, marker, bytes) = pending(dir.path());
    let sql = rusqlite::Connection::open(dir.path().join("kamimusuhi.sqlite")).unwrap();
    sql.execute("UPDATE evidence_records SET payload_json=json_set(payload_json,'$.marker_sha256','sha256:tampered') WHERE evidence_id=?",[marker["completion_id"].as_str().unwrap()]).unwrap();
    assert!(memory_writeback::status(&runtime).is_err());
    std::fs::write(path, [bytes, b" ".to_vec()].concat()).unwrap();
    assert!(memory_writeback::status(&runtime).is_err());
}

#[test]
fn invalid_reason_token_and_marker_binding_never_append_a_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, turn) = failed(dir.path());
    let before = rows(dir.path());
    assert!(memory_writeback::review_abort(&runtime, turn, "  ").is_err());
    assert!(memory_writeback::review_abort(&runtime, turn, &"x".repeat(2049)).is_err());
    assert!(memory_writeback::abort(&runtime, turn, REASON, "unreviewed").is_err());
    let (path, mut marker, _) = pending(dir.path());
    marker["individual_id"] = json!("0000000000000000000000000000abcdef");
    std::fs::write(path, serde_json::to_vec(&marker).unwrap()).unwrap();
    assert!(memory_writeback::review_abort(&runtime, turn, REASON).is_err());
    assert_eq!(rows(dir.path()), before);
}
