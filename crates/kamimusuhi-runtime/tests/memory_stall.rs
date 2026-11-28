//! Memory stall signal over a real store: fake persona, fake reflector, no
//! API. Runs on the system clock (deterministic IDs only) so that records
//! written in sequence carry increasing timestamps — the signal measures
//! "since" by time.

use std::process::Command;
use std::time::Duration;

use kamimusuhi_core::continuity::ContinuityStore;
use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceSource, EvidenceStore, NewEvidence, RetentionClass,
};
use kamimusuhi_core::ids::EvidenceId;
use kamimusuhi_core::mutation::OriginClass;
use kamimusuhi_core::time::UtcTimestamp;
use kamimusuhi_runtime::c0::reflection;
use kamimusuhi_runtime::config::ReflectorBackend;
use kamimusuhi_runtime::dialogue::DialogueSession;
use kamimusuhi_runtime::memory_stall::{
    self, MemoryStallBaseline, MemoryStallReason, MemoryStallState, MemoryStallThresholds,
};
use kamimusuhi_runtime::{ResourceImplementation, Runtime, RuntimeOptions};
use kamimusuhi_testkit::fake_persona::FIXTURE_USER_SUBJECT;

fn init(dir: &std::path::Path) -> Runtime {
    let mut runtime = Runtime::init(
        dir,
        RuntimeOptions::deterministic_ids(31),
        ResourceImplementation::FakeA,
    )
    .unwrap();
    let mut config = runtime.config().clone();
    config.c0.reflector.backend = ReflectorBackend::Fake;
    // No turn-count reflection: only an explicit cycle may grow memory
    // besides the persona's own remember-request drafts.
    config.c0.reflection_interval_turns = 0;
    runtime.save_config(config).unwrap();
    let review = runtime.identity_status().unwrap();
    runtime.accept_identity(&review.review_token).unwrap();
    runtime
}

/// Strictly separate consecutive writes in time.
fn tick() {
    std::thread::sleep(Duration::from_millis(5));
}

fn thresholds(min_user_utterances: u64) -> MemoryStallThresholds {
    MemoryStallThresholds {
        min_user_utterances,
        min_quiet_secs: 0,
    }
}

#[test]
fn stall_signal_tracks_conversation_since_memory_last_grew() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());

    // An individual nobody has spoken to is never stalled.
    let fresh = memory_stall::report(&runtime, thresholds(1)).unwrap();
    assert_eq!(fresh.state, MemoryStallState::Ok);
    assert_eq!(fresh.reason, MemoryStallReason::NoUserUtterancesSinceMemory);
    assert_eq!(fresh.user_utterances_since_memory, 0);

    let mut session =
        DialogueSession::start(&mut runtime, FIXTURE_USER_SUBJECT, Default::default()).unwrap();
    tick();
    // The persona drafts a relationship fact; the kernel activates it.
    let reply = session
        .turn(&mut runtime, "私はほうじ茶が好き。覚えておいて", |_| Ok(()))
        .unwrap();
    assert_eq!(reply.c0.as_ref().unwrap().drafts_activated, 1);

    // Memory grew in the same turn: the utterance that caused it does not
    // count against it.
    let grown = memory_stall::report(&runtime, thresholds(1)).unwrap();
    assert_eq!(grown.state, MemoryStallState::Ok);
    assert_eq!(grown.baseline, MemoryStallBaseline::LastMemory);
    assert_eq!(grown.user_utterances_since_memory, 0);
    assert!(grown.last_memory_at.is_some());
    assert!(grown.state_records_total >= 1);

    // Plain conversation that nothing remembers.
    for text in ["こんにちは", "今日は晴れだね", "散歩に行った"] {
        tick();
        session.turn(&mut runtime, text, |_| Ok(())).unwrap();
    }

    let before = runtime
        .store()
        .commits(runtime.individual_id())
        .unwrap()
        .len();

    // Exactly at the utterance threshold: stalled, and no reflection ran.
    let at = memory_stall::report(&runtime, thresholds(3)).unwrap();
    assert_eq!(at.user_utterances_since_memory, 3);
    assert_eq!(at.state, MemoryStallState::Stalled);
    assert_eq!(at.reason, MemoryStallReason::StalledNoReflection);
    assert_eq!(at.reflections_since_memory, 0);
    assert_eq!(at.memory_proposals, 0);
    // One short: not yet.
    let below = memory_stall::report(&runtime, thresholds(4)).unwrap();
    assert_eq!(below.state, MemoryStallState::Ok);
    assert_eq!(below.reason, MemoryStallReason::BelowThreshold);

    // Quiet-period boundary measured from the last memory, inclusive.
    let last_memory = UtcTimestamp::from_rfc3339(at.last_memory_at.as_deref().unwrap()).unwrap();
    let hour_later = UtcTimestamp::from_unix_millis(last_memory.unix_millis() + 3_600_000);
    let quiet = |secs| MemoryStallThresholds {
        min_user_utterances: 3,
        min_quiet_secs: secs,
    };
    let exact = memory_stall::report_at(&runtime, hour_later, quiet(3600)).unwrap();
    assert_eq!(exact.quiet_secs, 3600);
    assert_eq!(exact.state, MemoryStallState::Stalled);
    let short = memory_stall::report_at(&runtime, hour_later, quiet(3601)).unwrap();
    assert_eq!(short.state, MemoryStallState::Ok);
    // With the conservative defaults a few minutes of chat is not a stall.
    let defaults = memory_stall::report(&runtime, MemoryStallThresholds::default()).unwrap();
    assert_eq!(defaults.state, MemoryStallState::Ok);

    // Reading the signal changed nothing canonical.
    assert_eq!(
        runtime
            .store()
            .commits(runtime.individual_id())
            .unwrap()
            .len(),
        before
    );
    assert_eq!(
        memory_stall::report(&runtime, thresholds(3))
            .unwrap()
            .user_utterances_since_memory,
        3
    );

    // A reflection cycle that captures the conversation grows memory again:
    // the stall clears on its own, without the signal doing anything.
    tick();
    let mut writer = None;
    let cycle = reflection::run(
        &mut runtime,
        FIXTURE_USER_SUBJECT,
        Some(session.session_id()),
        None,
        &mut writer,
    )
    .unwrap();
    assert!(cycle.canonical_drafts_activated >= 1);
    let cleared = memory_stall::report(&runtime, thresholds(1)).unwrap();
    assert_eq!(cleared.state, MemoryStallState::Ok);
    assert_eq!(cleared.user_utterances_since_memory, 0);
    assert!(cleared.state_records_total > grown.state_records_total);
    runtime.stopping();
}

/// A public-channel style utterance: stored as `Ephemeral`, the way dialogue
/// stores non-operator Discord speech.
fn append_ephemeral(runtime: &Runtime, n: u128) -> UtcTimestamp {
    let record = runtime
        .store()
        .append(NewEvidence {
            evidence_id: EvidenceId::from_u128(0x5EED_E000 + n),
            individual_id: runtime.individual_id(),
            session_id: None,
            turn_id: None,
            kind: EvidenceKind::UserUtterance,
            origin_class: OriginClass::Reported,
            payload: serde_json::json!({"text": format!("public chatter {n}")}),
            source: EvidenceSource {
                source_id: Some("discord:public-fixture".to_owned()),
                source_sequence: Some(u64::try_from(n).unwrap()),
                content_digest: None,
            },
            retention_class: RetentionClass::Ephemeral,
        })
        .unwrap();
    record.created_at
}

#[test]
fn only_standard_utterances_count_and_ephemeral_ones_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());
    let mut session =
        DialogueSession::start(&mut runtime, FIXTURE_USER_SUBJECT, Default::default()).unwrap();
    tick();
    session
        .turn(&mut runtime, "私はほうじ茶が好き。覚えておいて", |_| Ok(()))
        .unwrap();

    // Only ephemeral speech accumulates: never a stall, but visible.
    let mut newest_ephemeral = None;
    for n in 0..5 {
        tick();
        newest_ephemeral = Some(append_ephemeral(&runtime, n));
    }
    let quiet = memory_stall::report(&runtime, thresholds(1)).unwrap();
    assert_eq!(quiet.state, MemoryStallState::Ok);
    assert_eq!(quiet.reason, MemoryStallReason::NoUserUtterancesSinceMemory);
    assert_eq!(quiet.user_utterances_since_memory, 0);
    assert_eq!(quiet.ephemeral_user_utterances_since_memory, 5);
    assert_eq!(quiet.last_user_utterance_at, None);

    // One standard utterance, then more ephemeral ones after it.
    tick();
    session
        .turn(&mut runtime, "こんにちは", |_| Ok(()))
        .unwrap();
    for n in 5..8 {
        tick();
        newest_ephemeral = Some(append_ephemeral(&runtime, n));
    }
    let r = memory_stall::report(&runtime, thresholds(1)).unwrap();
    assert_eq!(r.user_utterances_since_memory, 1);
    assert_eq!(r.ephemeral_user_utterances_since_memory, 8);
    assert_eq!(r.state, MemoryStallState::Stalled);
    // Threshold 2 is not reached by 1 standard + 8 ephemeral.
    let below = memory_stall::report(&runtime, thresholds(2)).unwrap();
    assert_eq!(below.state, MemoryStallState::Ok);
    assert_eq!(below.reason, MemoryStallReason::BelowThreshold);
    // The newest utterance time follows the standard speech, not the later
    // ephemeral chatter.
    let last_standard =
        UtcTimestamp::from_rfc3339(r.last_user_utterance_at.as_deref().unwrap()).unwrap();
    assert!(last_standard < newest_ephemeral.unwrap());
    let value = serde_json::to_value(&r).unwrap();
    assert_eq!(value["ephemeral_user_utterances_since_memory"], 8);
    runtime.stopping();
}

#[test]
fn invalid_thresholds_are_refused_before_reading() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = init(dir.path());
    assert!(memory_stall::report(&runtime, thresholds(0)).is_err());
    runtime.stopping();
}

fn cli(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_kamimusuhi-runtime"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn memory_stall_cli_reports_json_and_scopes_its_flags() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_str().unwrap();
    let init = cli(&["init", "--dir", path, "--seed", "7"]);
    assert!(init.status.success(), "{init:?}");

    let out = cli(&[
        "memory-stall",
        "--dir",
        path,
        "--min-user-utterances",
        "5",
        "--min-quiet-secs",
        "60",
    ]);
    assert!(out.status.success(), "{out:?}");
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["state"], "ok");
    assert_eq!(report["reason"], "NO_USER_UTTERANCES_SINCE_MEMORY");
    assert_eq!(report["thresholds"]["min_user_utterances"], 5);
    assert_eq!(report["thresholds"]["min_quiet_secs"], 60);

    let defaults = cli(&["memory-stall", "--dir", path]);
    let report: serde_json::Value = serde_json::from_slice(&defaults.stdout).unwrap();
    assert_eq!(report["thresholds"]["min_user_utterances"], 40);
    assert_eq!(report["thresholds"]["min_quiet_secs"], 172_800);

    // The threshold flags belong to memory-stall alone.
    let misuse = cli(&["inspect", "--dir", path, "--min-user-utterances", "3"]);
    assert_eq!(misuse.status.code(), Some(2));
    let zero = cli(&["memory-stall", "--dir", path, "--min-user-utterances", "0"]);
    assert!(!zero.status.success());
}
