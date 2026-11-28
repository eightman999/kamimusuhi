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
//! * (b) the C0 lane's canonical narration used to be written in a second
//!   transaction after the lane moved, so a failed narration left the lane
//!   moved with no canonical record. The narration now commits in the lane's
//!   own transaction (activation, rollback, proposal intake); the tests pin
//!   that the two never disagree, on failure or on the normal path.

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

/// Shape (b). `c0::activate` once committed the lane move and only then
/// appended the canonical `c0_activation_applied` narration in a separate
/// write; a failed append left the lane moved, the retry was refused as
/// `ProposalNotPending`, and the narration was never written. Whatever the
/// first attempt does, the final state must have the lane moved with
/// exactly one narration.
#[test]
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

fn install(dir: &Path, event: &str) {
    sql(dir)
        .execute_batch(&format!(
            "CREATE TRIGGER injected_narration_stop BEFORE INSERT ON evidence_records
             WHEN json_extract(NEW.payload_json, '$.event') = '{event}'
             BEGIN SELECT RAISE(FAIL, 'injected narration failure'); END;"
        ))
        .unwrap();
}

fn remove(dir: &Path) {
    sql(dir)
        .execute_batch("DROP TRIGGER injected_narration_stop;")
        .unwrap();
}

fn events(dir: &Path, event: &str) -> i64 {
    count(
        dir,
        "SELECT count(*) FROM evidence_records WHERE json_extract(payload_json, '$.event') = ?1",
        event,
    )
}

fn pending_proposal(runtime: &Runtime) -> kamimusuhi_core::ids::C0ProposalId {
    c0::propose_param(runtime, "retrieval.self_notes_max", json!(9), None)
        .unwrap()
        .proposal_id
}

/// The failed attempt itself commits nothing: the head stays, the proposal
/// stays pending, no narration exists — so the retry is an ordinary apply.
#[test]
fn a_failed_activation_narration_leaves_the_lane_unmoved_and_the_proposal_pending() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = init(dir.path());
    let individual = runtime.individual_id();
    let head_before = runtime.store().c0_head(individual).unwrap();
    let proposal = pending_proposal(&runtime);

    install(dir.path(), "c0_activation_applied");
    assert!(c0::activate(&runtime, proposal, None, None).is_err());
    remove(dir.path());
    assert_eq!(runtime.store().c0_head(individual).unwrap(), head_before);
    assert!(
        runtime
            .store()
            .c0_activations(individual)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        runtime
            .store()
            .c0_proposal(proposal)
            .unwrap()
            .unwrap()
            .status,
        kamimusuhi_core::c0::ProposalStatus::Pending
    );
    assert_eq!(events(dir.path(), "c0_activation_applied"), 0);

    let activation = c0::activate(&runtime, proposal, None, None).unwrap();
    assert_eq!(
        runtime.store().c0_head(individual).unwrap(),
        activation.activation_seq
    );
    assert_eq!(events(dir.path(), "c0_activation_applied"), 1);
}

/// Shape (b) for `rollback`: a failed `c0_rollback` narration must not
/// leave the head moved back without its canonical record.
#[test]
fn c0_rollback_and_its_canonical_narration_agree_after_a_failed_narration() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = init(dir.path());
    let individual = runtime.individual_id();
    let proposal = pending_proposal(&runtime);
    let activation = c0::activate(&runtime, proposal, None, None).unwrap();
    let applied = activation.activation_seq;

    install(dir.path(), "c0_rollback");
    let first = c0::rollback(&runtime, None, None);
    remove(dir.path());
    assert!(first.is_err(), "{first:?}");
    assert_eq!(
        (
            runtime.store().c0_head(individual).unwrap(),
            events(dir.path(), "c0_rollback")
        ),
        (applied, 0),
        "a failed rollback narration committed part of the rollback"
    );
    let retried = c0::rollback(&runtime, None, None).unwrap();
    assert_eq!(retried.rolled_back_seq, applied);

    let rolled_back = runtime.store().c0_head(individual).unwrap() == activation.predecessor_seq;
    let narrations = count(
        dir.path(),
        "SELECT count(*) FROM evidence_records
         WHERE json_extract(payload_json, '$.event') = 'c0_rollback'
         AND json_extract(payload_json, '$.rolled_back_seq') = CAST(?1 AS INTEGER)",
        &applied.to_string(),
    );
    assert_eq!((rolled_back, narrations), (true, 1));
}

/// Proposal intake narrates in the same transaction: a failed narration
/// leaves no pending proposal the canonical record never saw, and the retry
/// leaves exactly one proposal with exactly one narration.
#[test]
fn c0_proposal_intake_and_its_canonical_narration_agree_after_a_failed_narration() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = init(dir.path());
    let individual = runtime.individual_id();

    install(dir.path(), "improvement_proposal");
    let first = c0::propose_param(&runtime, "retrieval.self_notes_max", json!(9), None);
    remove(dir.path());
    assert!(first.is_err(), "{first:?}");
    assert!(
        runtime
            .store()
            .c0_proposals(individual, None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(events(dir.path(), "improvement_proposal"), 0);

    let proposal = pending_proposal(&runtime);
    let proposals = runtime.store().c0_proposals(individual, None).unwrap();
    assert_eq!(proposals.len(), 1);
    assert_eq!(proposals[0].proposal_id, proposal);
    assert_eq!(
        count(
            dir.path(),
            "SELECT count(*) FROM evidence_records
             WHERE json_extract(payload_json, '$.event') = 'improvement_proposal'
             AND json_extract(payload_json, '$.proposal_id') = ?1",
            &proposal.to_string(),
        ),
        1
    );
}

/// Normal path: each transition writes its narration exactly once, with the
/// same kind, origin class, source and payload keys as before the change.
#[test]
fn each_c0_transition_is_narrated_exactly_once_on_the_normal_path() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = init(dir.path());
    let proposal = pending_proposal(&runtime);
    let activation = c0::activate(&runtime, proposal, None, None).unwrap();
    let outcome = c0::rollback(&runtime, None, None).unwrap();
    assert_eq!(outcome.rolled_back_seq, activation.activation_seq);

    let shape = |event: &str| -> (i64, String, String, String, String) {
        sql(dir.path())
            .query_row(
                "SELECT count(*), kind, origin_class, source_id,
                        (SELECT group_concat(key, ',') FROM
                           (SELECT key FROM json_each(payload_json) ORDER BY key))
                 FROM evidence_records WHERE json_extract(payload_json, '$.event') = ?1",
                [event],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap()
    };
    assert_eq!(
        shape("improvement_proposal"),
        (
            1,
            "system_event".to_owned(),
            "inferred".to_owned(),
            "self-improve-tool".to_owned(),
            "event,kind,proposal_id,proposed_value,target,via".to_owned()
        )
    );
    assert_eq!(
        shape("c0_activation_applied"),
        (
            1,
            "system_event".to_owned(),
            "inferred".to_owned(),
            "c0-derived-lane".to_owned(),
            "activation_id,activation_seq,event,proposal_id".to_owned()
        )
    );
    assert_eq!(
        shape("c0_rollback"),
        (
            1,
            "system_event".to_owned(),
            "observed".to_owned(),
            "c0-derived-lane".to_owned(),
            "event,proposal_id,restored_seq,rolled_back_seq".to_owned()
        )
    );
    // The narration names the activation actually written.
    let narrated: (i64, String) = sql(dir.path())
        .query_row(
            "SELECT json_extract(payload_json, '$.activation_seq'),
                    json_extract(payload_json, '$.activation_id')
             FROM evidence_records
             WHERE json_extract(payload_json, '$.event') = 'c0_activation_applied'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        narrated,
        (
            activation.activation_seq as i64,
            activation.activation_id.to_string()
        )
    );
}

/// Run `f` while another connection holds the write lock and then commits a
/// write. A narrated transition must wait for that writer (BEGIN IMMEDIATE
/// under the busy timeout) rather than read first and fail to upgrade on a
/// stale snapshot.
fn under_foreign_writer<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    let (locked, wait_locked) = std::sync::mpsc::channel();
    let path = dir.join("kamimusuhi.sqlite");
    let writer = std::thread::spawn(move || {
        let conn = rusqlite::Connection::open(path).unwrap();
        conn.execute_batch(
            "BEGIN IMMEDIATE;
             CREATE TABLE IF NOT EXISTS injected_foreign_write(n INTEGER);
             INSERT INTO injected_foreign_write VALUES (1);",
        )
        .unwrap();
        locked.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        conn.execute_batch("COMMIT;").unwrap();
    });
    wait_locked.recv().unwrap();
    let out = f();
    writer.join().unwrap();
    out
}

#[test]
fn narrated_c0_transitions_wait_for_a_concurrent_writer_and_commit_whole() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = init(dir.path());
    let individual = runtime.individual_id();

    let proposal = under_foreign_writer(dir.path(), || {
        c0::propose_param(&runtime, "retrieval.self_notes_max", json!(9), None)
    })
    .unwrap()
    .proposal_id;
    let activation =
        under_foreign_writer(dir.path(), || c0::activate(&runtime, proposal, None, None)).unwrap();
    assert_eq!(
        runtime.store().c0_head(individual).unwrap(),
        activation.activation_seq
    );
    let outcome = under_foreign_writer(dir.path(), || c0::rollback(&runtime, None, None)).unwrap();
    assert_eq!(outcome.rolled_back_seq, activation.activation_seq);
    assert_eq!(
        runtime.store().c0_head(individual).unwrap(),
        activation.predecessor_seq
    );
    for event in [
        "improvement_proposal",
        "c0_activation_applied",
        "c0_rollback",
    ] {
        assert_eq!(events(dir.path(), event), 1, "{event}");
    }
    // The foreign writes all landed too: nothing was serialised away.
    let foreign: i64 = sql(dir.path())
        .query_row("SELECT count(*) FROM injected_foreign_write", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(foreign, 3);
}
