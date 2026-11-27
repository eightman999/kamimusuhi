//! FIO write-path failure shapes against a live runtime (no paid model).
//!
//! See `docs/verification/write-fault-shapes-2026-11-27.md`. Faults are injected with
//! SQLite triggers on the real store file, as `continuity_scenarios.rs`
//! does, and every assertion reads the final state back from disk.
//!
//! * (a) a resend of the same text after a failed writeback is a new,
//!   recorded turn — the failed attempt reserved nothing that dedupes it;
//! * (c) when the primary write (the emitted `AgentUtterance`) fails, the
//!   derived writes that describe the turn — the `memory_writeback_verified`
//!   completion and the per-turn C0 evaluation — are not written either;
//! * (b) the C0 lane's canonical narration is written in a second
//!   transaction after the lane moved. The `#[ignore]`d test reproduces the
//!   resulting disagreement; it documents a defect, not current behaviour.

use std::path::Path;

use kamimusuhi_core::ids::TurnId;
use kamimusuhi_runtime::dialogue::DialogueSession;
use kamimusuhi_runtime::llm_jev::{MockLanguageProvider, RuleBasedDecisionProvider};
use kamimusuhi_runtime::{ResourceImplementation, Runtime, RuntimeOptions, c0, memory_writeback};
use serde_json::json;

const RESENT: &str = "同じ言葉をもう一度送る";

fn init(path: &Path) -> Runtime {
    Runtime::init(
        path,
        RuntimeOptions::deterministic(0xFA17),
        ResourceImplementation::FakeA,
    )
    .unwrap()
}

fn session(runtime: &mut Runtime) -> DialogueSession {
    DialogueSession::start_with_providers(
        runtime,
        "write-fault-fixture",
        Default::default(),
        Box::new(RuleBasedDecisionProvider),
        Some(Box::new(MockLanguageProvider)),
    )
    .unwrap()
}

fn sql(dir: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(dir.join("kamimusuhi.sqlite")).unwrap()
}

fn count(dir: &Path, query: &str, arg: &str) -> i64 {
    sql(dir).query_row(query, [arg], |r| r.get(0)).unwrap()
}

const STOP_AGENT_UTTERANCE: &str = "CREATE TRIGGER injected_agent_utterance_stop
     BEFORE INSERT ON evidence_records WHEN NEW.kind = 'agent_utterance'
     BEGIN SELECT RAISE(FAIL, 'injected primary write failure'); END;";

/// Turn IDs whose writeback marker has no terminal event.
fn unresolved(runtime: &Runtime) -> Vec<String> {
    memory_writeback::status(runtime).unwrap()["turn_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect()
}

/// Run one turn whose emitted utterance cannot be stored; return its turn.
fn failed_turn(dir: &Path, runtime: &mut Runtime) -> TurnId {
    sql(dir).execute_batch(STOP_AGENT_UTTERANCE).unwrap();
    let mut chat = session(runtime);
    let mut delivered = false;
    assert!(
        chat.turn(runtime, RESENT, |_| {
            delivered = true;
            Ok(())
        })
        .is_err()
    );
    assert!(
        delivered,
        "the surface accepted the reply before persistence"
    );
    sql(dir)
        .execute_batch("DROP TRIGGER injected_agent_utterance_stop;")
        .unwrap();
    let turns = unresolved(runtime);
    assert_eq!(turns.len(), 1, "{turns:?}");
    turns[0].parse().unwrap()
}

/// Shape (a): FIO keys "already handled" on the content hash before the
/// stages run, so the same text resent after a failure is dropped. Here
/// the writeback marker records *pending*, the completion is written last,
/// and nothing keys on content — the resend is a new turn that lands.
#[test]
fn a_resent_text_after_a_failed_writeback_is_recorded_not_deduplicated() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());
    let failed = failed_turn(dir.path(), &mut runtime);
    drop(runtime);

    let mut runtime = Runtime::open(dir.path(), RuntimeOptions::deterministic(0xFA18)).unwrap();
    let mut chat = session(&mut runtime);
    let reply = chat.turn(&mut runtime, RESENT, |_| Ok(())).unwrap();
    let writeback = reply.c0.unwrap().memory_writeback;
    assert_eq!(writeback["event"], "memory_writeback_verified");

    // Final state on disk: both inputs exist under distinct turns, the
    // resend's reply is stored and verified, and the failed turn is still
    // reported unresolved rather than silently absorbed by the success.
    let inputs = count(
        dir.path(),
        "SELECT count(DISTINCT turn_id) FROM evidence_records
         WHERE kind = 'user_utterance' AND json_extract(payload_json, '$.text') = ?1",
        RESENT,
    );
    assert_eq!(inputs, 2, "the resend was not recorded as its own turn");
    let verified_input: String = sql(dir.path())
        .query_row(
            "SELECT json_extract(payload_json, '$.input_evidence_id') FROM evidence_records
             WHERE json_extract(payload_json, '$.event') = 'memory_writeback_verified'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count(
            dir.path(),
            "SELECT count(*) FROM evidence_records
             WHERE evidence_id = ?1 AND kind = 'user_utterance'",
            &verified_input,
        ),
        1
    );
    assert_ne!(
        count(
            dir.path(),
            "SELECT count(*) FROM evidence_records WHERE evidence_id = ?1 AND turn_id IS NOT NULL",
            &verified_input,
        ),
        0
    );
    assert_eq!(
        count(
            dir.path(),
            "SELECT count(*) FROM evidence_records WHERE kind = ?1",
            "agent_utterance"
        ),
        1
    );
    assert_eq!(unresolved(&runtime), vec![failed.to_string()]);
}

/// Shape (c): FIO writes the derived copy whether or not the primary save
/// succeeded. Here the completion event and the turn evaluation follow the
/// emitted utterance's append with `?`, so a failed primary leaves neither.
#[test]
fn a_failed_primary_write_leaves_no_derived_completion_or_turn_evaluation() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());
    let failed = failed_turn(dir.path(), &mut runtime).to_string();

    let for_turn = |query: &str| count(dir.path(), query, &failed);
    assert_eq!(
        for_turn(
            "SELECT count(*) FROM evidence_records WHERE turn_id = ?1 AND kind = 'agent_utterance'"
        ),
        0
    );
    assert_eq!(
        for_turn(
            "SELECT count(*) FROM evidence_records WHERE turn_id = ?1
             AND json_extract(payload_json, '$.event') = 'memory_writeback_verified'"
        ),
        0,
        "a completion exists for a turn whose reply was never stored"
    );
    assert_eq!(
        for_turn("SELECT count(*) FROM c0_evaluations WHERE turn_id = ?1"),
        0,
        "a turn evaluation exists for a turn whose reply was never stored"
    );

    // Positive control: the same derived rows do appear for a turn whose
    // primary write landed, so the zeros above are not vacuous.
    let mut chat = session(&mut runtime);
    chat.turn(&mut runtime, "こんにちは", |_| Ok(())).unwrap();
    let ok: String = sql(dir.path())
        .query_row(
            "SELECT turn_id FROM evidence_records WHERE kind = 'agent_utterance'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let for_ok = |query: &str| count(dir.path(), query, &ok);
    assert_eq!(
        for_ok(
            "SELECT count(*) FROM evidence_records WHERE turn_id = ?1
             AND json_extract(payload_json, '$.event') = 'memory_writeback_verified'"
        ),
        1
    );
    assert_eq!(
        for_ok("SELECT count(*) FROM c0_evaluations WHERE turn_id = ?1"),
        1
    );
}

/// Shape (b), reproduced. `c0::activate` commits the lane move
/// (`c0_apply_activation`) and only then appends the canonical
/// `c0_activation_applied` narration in a separate write. When that append
/// fails the caller gets an error although the lane already moved, and the
/// retry is refused as `ProposalNotPending`, so the narration is never
/// written: the lane and the canonical record disagree permanently.
#[test]
#[ignore = "defect: C0 lane moves before its canonical narration; a failed narration is never reconciled (see docs/verification/write-fault-shapes-2026-11-27.md)"]
fn c0_lane_and_its_canonical_narration_agree_after_a_failed_narration() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = init(dir.path());
    let individual = runtime.individual_id();
    let head_before = runtime.store().c0_head(individual).unwrap();
    let proposal = c0::propose_param(&runtime, "retrieval.self_notes_max", json!(9), None)
        .unwrap()
        .proposal_id;
    sql(dir.path())
        .execute_batch(
            "CREATE TRIGGER injected_narration_stop BEFORE INSERT ON evidence_records
             WHEN json_extract(NEW.payload_json, '$.event') = 'c0_activation_applied'
             BEGIN SELECT RAISE(FAIL, 'injected narration failure'); END;",
        )
        .unwrap();
    let first = c0::activate(&runtime, proposal, None, None);
    sql(dir.path())
        .execute_batch("DROP TRIGGER injected_narration_stop;")
        .unwrap();
    // Retry after the fault clears, as an operator or the reflection cycle would.
    let retried = first
        .is_err()
        .then(|| c0::activate(&runtime, proposal, None, None));

    let moved = runtime.store().c0_head(individual).unwrap() != head_before;
    let narrations = count(
        dir.path(),
        "SELECT count(*) FROM evidence_records
         WHERE json_extract(payload_json, '$.event') = 'c0_activation_applied'
         AND json_extract(payload_json, '$.proposal_id') = ?1",
        &proposal.to_string(),
    );
    assert_eq!(
        (moved, narrations),
        (true, 1),
        "lane moved={moved} but narrations={narrations}; first={first:?} retry={retried:?}"
    );
}
