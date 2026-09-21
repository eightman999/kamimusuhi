//! Deterministic provider-arbitration coverage for the dialogue loop.
//!
//! Every scenario runs against in-process scripted organs — no sockets, no
//! external service, no wall-clock dependence beyond scripted delays — so
//! the race, the gates, the fallbacks and cancellation replay identically
//! on every run.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use kamimusuhi_core::routing::PrivacyConstraint;
use kamimusuhi_runtime::dialogue::DialogueSession;
use kamimusuhi_runtime::llm_jev::testing::{
    ScriptedDecisionProvider, ScriptedLanguageProvider, ScriptedOutcome,
};
use kamimusuhi_runtime::llm_jev::{ConversationError, Decision, DecisionProvider};
use kamimusuhi_runtime::{ResourceImplementation, Runtime, RuntimeOptions};

fn session_with(
    decision: Box<dyn DecisionProvider>,
    primary: ScriptedLanguageProvider,
    extras: Vec<(String, ScriptedLanguageProvider)>,
) -> (tempfile::TempDir, Runtime, DialogueSession) {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = Runtime::init(
        dir.path(),
        RuntimeOptions::deterministic(7),
        ResourceImplementation::FakeA,
    )
    .unwrap();
    let mut session = DialogueSession::start_with_providers(
        &mut runtime,
        "alice",
        PrivacyConstraint::LocalOnly,
        decision,
        Some(Box::new(primary)),
    )
    .unwrap();
    for (id, provider) in extras {
        session
            .add_language_provider(&id, Arc::new(provider))
            .unwrap();
    }
    session.set_debug_trace(true);
    (dir, runtime, session)
}

/// Wait until every gauge reads zero — bounded so a real leak fails the
/// test instead of hanging it.
fn all_inactive(gauges: &[Arc<AtomicUsize>]) -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if gauges.iter().all(|gauge| gauge.load(Ordering::SeqCst) == 0) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    false
}

// A — one provider, one acceptable response: the loop closes and delivers.
#[test]
fn single_provider_accept_delivers() {
    let (_dir, mut runtime, mut session) = session_with(
        Box::new(ScriptedDecisionProvider::accept_all()),
        ScriptedLanguageProvider::fast_good("primary"),
        Vec::new(),
    );
    let reply = session
        .turn(&mut runtime, "こんにちは", |_| Ok(()))
        .unwrap();
    assert_eq!(reply.response, "scripted-good");
    let trace = reply.llm_jev.unwrap();
    assert_eq!(trace.language_provider_id, "primary");
    assert_eq!(trace.invocation_gate.decision, Decision::Speak);
    assert_eq!(trace.response_gate.decision, Decision::Accept);
    assert!(trace.decision_fallbacks.is_empty());
    assert!(trace.late_candidates.is_empty());
    assert_eq!(trace.retry_count, 0);
}

// B — fast_bad + slow_good: the fastest candidate is rejected and the
// slower good one is delivered. "Fastest wins" is about the first *usable*
// candidate, not the first bytes.
#[test]
fn fast_bad_rejected_then_slow_good_accepted() {
    let (_dir, mut runtime, mut session) = session_with(
        Box::new(ScriptedDecisionProvider::reject_containing("fixture-bad")),
        ScriptedLanguageProvider::fast_bad("primary"),
        vec![(
            "slow".to_owned(),
            ScriptedLanguageProvider::slow_good("slow"),
        )],
    );
    let reply = session
        .turn(&mut runtime, "こんにちは", |_| Ok(()))
        .unwrap();
    assert_eq!(reply.response, "scripted-good-slow");
    let trace = reply.llm_jev.unwrap();
    assert_eq!(trace.language_provider_id, "slow");
    assert_eq!(trace.generated_candidates.len(), 2);
    assert_eq!(trace.assessments.len(), 2);
    assert_eq!(trace.assessments[0].gate.decision, Decision::Reject);
    assert_eq!(trace.assessments[1].gate.decision, Decision::Accept);
}

// C — fast_good + an organ that never answers: the good candidate wins, the
// hanging organ is cancelled and its late finish is recorded, not dropped.
#[test]
fn fast_good_accepted_and_hanging_organ_is_cancelled_and_observed() {
    let hanging = ScriptedLanguageProvider::hanging("hung", Duration::from_secs(5), true);
    let active = hanging.active();
    let (_dir, mut runtime, mut session) = session_with(
        Box::new(ScriptedDecisionProvider::accept_all()),
        ScriptedLanguageProvider::fast_good("primary"),
        vec![("hung".to_owned(), hanging)],
    );
    let started = Instant::now();
    let reply = session
        .turn(&mut runtime, "こんにちは", |_| Ok(()))
        .unwrap();
    let elapsed = started.elapsed();
    assert_eq!(reply.response, "scripted-good");
    let trace = reply.llm_jev.unwrap();
    assert_eq!(trace.generated_candidates.len(), 1);
    assert_eq!(trace.generated_candidates[0].id, "primary");
    // The turn closed long before the hanging organ's own deadline.
    assert!(elapsed < Duration::from_secs(5));
    let late = trace
        .late_candidates
        .iter()
        .find(|candidate| candidate.id == "hung")
        .expect("the cancelled organ's late attempt is recorded");
    assert_eq!(late.error_code.as_deref(), Some("CANCELLED"));
    // Cancellation is not a provider fault: advisory telemetry stays clean.
    assert_eq!(trace.provider_telemetry["hung"].calls, 0);
    assert!(all_inactive(&[active]));
}

// D — twelve organs race. There is no fixed provider cap; the anonymous
// candidate pool is bounded to what the race actually delivered.
#[test]
fn a_dozen_providers_race_without_a_fixed_cap() {
    let mut gauges = Vec::new();
    let mut extras = Vec::new();
    for index in 1_u64..12 {
        let provider = ScriptedLanguageProvider::new(
            format!("organ-{index}"),
            Duration::from_millis(index),
            ScriptedOutcome::Respond("scripted-good"),
        );
        gauges.push(provider.calls());
        extras.push((format!("organ-{index}"), provider));
    }
    let (_dir, mut runtime, mut session) = session_with(
        Box::new(ScriptedDecisionProvider::accept_all()),
        ScriptedLanguageProvider::new(
            "primary",
            Duration::from_millis(1),
            ScriptedOutcome::Respond("scripted-good"),
        ),
        extras,
    );
    let reply = session
        .turn(&mut runtime, "こんにちは", |_| Ok(()))
        .unwrap();
    let trace = reply.llm_jev.unwrap();
    // All twelve organs registered and were invoked by the race.
    assert_eq!(trace.provider_candidates.len(), 12);
    assert_eq!(session.language_provider_telemetry().len(), 12);
    for gauge in &gauges {
        assert_eq!(gauge.load(Ordering::SeqCst), 1);
    }
    // The delivered response is bound to one of them, and every received
    // candidate carried an anonymous candidate-N identity to the gate.
    assert!(
        trace.language_provider_id == "primary" || trace.language_provider_id.starts_with("organ-")
    );
}

// E — the fastest organ fails or stalls; the race keeps moving and the next
// usable candidate is delivered. Two failure shapes: a fast timeout error,
// and an organ that simply never answers before its own deadline.
#[test]
fn timing_out_fastest_provider_fails_over() {
    let (_dir, mut runtime, mut session) = session_with(
        Box::new(ScriptedDecisionProvider::accept_all()),
        ScriptedLanguageProvider::new(
            "primary",
            Duration::from_millis(2),
            ScriptedOutcome::Fail(|| ConversationError::Timeout),
        ),
        vec![(
            "slow".to_owned(),
            ScriptedLanguageProvider::slow_good("slow"),
        )],
    );
    let reply = session
        .turn(&mut runtime, "こんにちは", |_| Ok(()))
        .unwrap();
    assert_eq!(reply.response, "scripted-good-slow");
    let trace = reply.llm_jev.unwrap();
    assert_eq!(trace.language_provider_id, "slow");
    let failed = trace
        .generated_candidates
        .iter()
        .find(|candidate| candidate.id == "primary")
        .unwrap();
    assert_eq!(failed.error_code.as_deref(), Some("TIMEOUT"));
    assert_eq!(trace.provider_telemetry["primary"].failures, 1);

    // A hang that cannot honour cancellation: the turn still finishes on the
    // good organ without waiting for the straggler's internal deadline.
    let (_dir, mut runtime, mut session) = session_with(
        Box::new(ScriptedDecisionProvider::accept_all()),
        ScriptedLanguageProvider::new(
            "primary",
            Duration::ZERO,
            ScriptedOutcome::Hang {
                deadline: Duration::from_millis(500),
                honour_cancellation: false,
            },
        ),
        vec![(
            "fast".to_owned(),
            ScriptedLanguageProvider::fast_good("fast"),
        )],
    );
    let started = Instant::now();
    let reply = session
        .turn(&mut runtime, "こんにちは", |_| Ok(()))
        .unwrap();
    assert_eq!(reply.response, "scripted-good");
    assert!(started.elapsed() < Duration::from_millis(500));
}

// F — Jev cannot answer at all. The turn degrades to the local rule-based
// gate, is marked `fallback`, is traced, and the conversation continues.
#[test]
fn jev_outage_degrades_to_the_local_gate_and_conversation_continues() {
    // Full outage: prepare fails, so the whole turn is local.
    let decision = ScriptedDecisionProvider::unavailable(|| ConversationError::Timeout);
    let (_dir, mut runtime, mut session) = session_with(
        Box::new(decision),
        ScriptedLanguageProvider::fast_good("primary"),
        vec![(
            "extra".to_owned(),
            ScriptedLanguageProvider::slow_good("extra"),
        )],
    );
    for turn in 0..2 {
        let reply = session
            .turn(&mut runtime, &format!("ターン{turn}"), |_| Ok(()))
            .expect("a dead Jev must not wedge the conversation");
        assert!(!reply.response.is_empty());
        let trace = reply.llm_jev.unwrap();
        assert_eq!(
            trace.decision_fallbacks,
            vec![
                "prepare_turn:TIMEOUT".to_owned(),
                "assess_responses:TIMEOUT".to_owned()
            ]
        );
        assert!(trace.invocation_gate.fallback);
        assert!(trace.response_gate.fallback);
    }
    assert_eq!(session.turn_count(), 2);

    // Partial outage: the invocation gate answers, the assessment dies.
    let decision = ScriptedDecisionProvider {
        assess_error: Some(|| ConversationError::HttpStatus(503)),
        ..ScriptedDecisionProvider::accept_all()
    };
    let (_dir, mut runtime, mut session) = session_with(
        Box::new(decision),
        ScriptedLanguageProvider::fast_good("primary"),
        Vec::new(),
    );
    let reply = session
        .turn(&mut runtime, "こんにちは", |_| Ok(()))
        .unwrap();
    let trace = reply.llm_jev.unwrap();
    assert_eq!(
        trace.decision_fallbacks,
        vec!["assess_responses:HTTP_STATUS".to_owned()]
    );
    assert!(!trace.invocation_gate.fallback);
    assert!(trace.response_gate.fallback);
}

// F' — a Jev that *answers* but breaks its contract does not degrade: the
// malformed verdict stays fail-closed, the turn errors, nothing delivers.
#[test]
fn malformed_jev_answers_stay_fail_closed() {
    let decision = ScriptedDecisionProvider {
        assess_error: Some(|| ConversationError::Malformed("bad verdict".to_owned())),
        ..ScriptedDecisionProvider::accept_all()
    };
    let (_dir, mut runtime, mut session) = session_with(
        Box::new(decision),
        ScriptedLanguageProvider::fast_good("primary"),
        Vec::new(),
    );
    assert!(
        session
            .turn(&mut runtime, "こんにちは", |_| panic!(
                "must not deliver"
            ))
            .is_err()
    );
    assert!(
        session
            .history_for_display(&runtime)
            .unwrap()
            .iter()
            .all(|message| message.role != kamimusuhi_core::persona::ConversationRole::Assistant)
    );
}

// F'' — a Jev that rejects our request (4xx), or that is misconfigured or
// missing its credential, is not an outage: the cause is ours to fix, so
// the turn stays fail-closed and never silently degrades to the local gate.
#[test]
fn jev_client_and_config_errors_stay_fail_closed() {
    for error in [
        (|| ConversationError::HttpStatus(400)) as fn() -> ConversationError,
        (|| ConversationError::Credential("missing KAMIMUSUHI_JEV_KEY".to_owned()))
            as fn() -> ConversationError,
        (|| ConversationError::InvalidConfig("unparseable base url".to_owned()))
            as fn() -> ConversationError,
    ] {
        let decision = ScriptedDecisionProvider::unavailable(error);
        let (_dir, mut runtime, mut session) = session_with(
            Box::new(decision),
            ScriptedLanguageProvider::fast_good("primary"),
            Vec::new(),
        );
        assert!(
            session
                .turn(&mut runtime, "こんにちは", |_| panic!(
                    "must not deliver"
                ))
                .is_err(),
            "a contract/config failure must not degrade to the local gate"
        );
        assert!(
            session.history_for_display(&runtime).unwrap().iter().all(
                |message| message.role != kamimusuhi_core::persona::ConversationRole::Assistant
            )
        );
    }

    // Same classification at the assessment stage, after a healthy
    // invocation gate.
    let decision = ScriptedDecisionProvider {
        assess_error: Some(|| ConversationError::HttpStatus(403)),
        ..ScriptedDecisionProvider::accept_all()
    };
    let (_dir, mut runtime, mut session) = session_with(
        Box::new(decision),
        ScriptedLanguageProvider::fast_good("primary"),
        Vec::new(),
    );
    assert!(
        session
            .turn(&mut runtime, "こんにちは", |_| panic!(
                "must not deliver"
            ))
            .is_err()
    );
}

// G — every provider fails: a controlled error, no delivery, no deadlock,
// and the session stays usable for the next turn.
#[test]
fn all_providers_fail_is_a_controlled_error_and_the_session_recovers() {
    let (_dir, mut runtime, mut session) = session_with(
        Box::new(ScriptedDecisionProvider::accept_all()),
        ScriptedLanguageProvider::erroring("primary"),
        vec![
            ("empty".to_owned(), ScriptedLanguageProvider::empty("empty")),
            (
                "timeout".to_owned(),
                ScriptedLanguageProvider::new(
                    "timeout",
                    Duration::from_millis(2),
                    ScriptedOutcome::Fail(|| ConversationError::Timeout),
                ),
            ),
        ],
    );
    assert!(
        session
            .turn(&mut runtime, "こんにちは", |_| panic!(
                "must not deliver"
            ))
            .is_err()
    );
    assert_eq!(
        session
            .last_generated_candidates()
            .iter()
            .filter(|candidate| candidate.error_code.is_some())
            .count(),
        3
    );
    // Recovery: a newly registered healthy organ is used on the next turn.
    session
        .add_language_provider(
            "recovered",
            Arc::new(ScriptedLanguageProvider::fast_good("recovered")),
        )
        .unwrap();
    let reply = session.turn(&mut runtime, "もう一度", |_| Ok(())).unwrap();
    assert_eq!(reply.response, "scripted-good");
    assert_eq!(session.turn_count(), 2);
}

// H — nothing the gate is shown can carry a real provider or model name.
#[test]
fn decision_payloads_never_contain_model_or_provider_identity() {
    let decision = ScriptedDecisionProvider::accept_all();
    let requests = Arc::clone(&decision.requests);
    let (_dir, mut runtime, mut session) = session_with(
        Box::new(decision),
        ScriptedLanguageProvider::new(
            "primary",
            Duration::from_millis(1),
            ScriptedOutcome::Respond("scripted-good"),
        ),
        vec![(
            "backup".to_owned(),
            ScriptedLanguageProvider::slow_good("backup"),
        )],
    );
    let reply = session
        .turn(&mut runtime, "こんにちは", |_| Ok(()))
        .unwrap();
    assert!(!reply.response.is_empty());
    // The gate saw candidate-N identities and telemetry only: never the
    // organs' provider label or model strings.
    let captured = requests.lock().unwrap();
    assert_eq!(captured.len(), 2, "prepare_turn + one assessment");
    for body in captured.iter() {
        assert!(!body.contains("scripted-language"), "{body}");
        assert!(!body.contains("scripted-model"), "{body}");
        assert!(!body.contains("\"primary\""), "{body}");
        assert!(!body.contains("\"backup\""), "{body}");
    }
    // The assessment request did carry anonymous candidate identities.
    let assessment: serde_json::Value = serde_json::from_str(captured.last().unwrap()).unwrap();
    let ids: Vec<&str> = assessment["selection"]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|candidate| candidate["id"].as_str().unwrap())
        .collect();
    assert!(ids.iter().all(|id| id.starts_with("candidate-")));
}

// I — accepted-early organs stop promptly: the cancellation token fires,
// in-flight calls return CANCELLED, and no thread or task leaks.
#[test]
fn cancelled_organs_do_not_leak_threads_or_calls() {
    let hung = ScriptedLanguageProvider::hanging("hung", Duration::from_secs(30), true);
    let active = hung.active();
    let calls = hung.calls();
    let (_dir, mut runtime, mut session) = session_with(
        Box::new(ScriptedDecisionProvider::accept_all()),
        ScriptedLanguageProvider::fast_good("primary"),
        vec![("hung".to_owned(), hung)],
    );
    let reply = session
        .turn(&mut runtime, "こんにちは", |_| Ok(()))
        .unwrap();
    assert_eq!(reply.response, "scripted-good");
    // The organ was invoked once, observed cancelled, and its worker exited.
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(all_inactive(&[active]));
    let trace = reply.llm_jev.unwrap();
    assert!(
        trace
            .late_candidates
            .iter()
            .any(|candidate| candidate.id == "hung"
                && candidate.error_code.as_deref() == Some("CANCELLED"))
    );
}

// J — consecutive turns keep session state coherent: sequence advances,
// turn ids are unique, the conversation-side core state threads through.
#[test]
fn consecutive_turns_keep_session_state_coherent() {
    let (_dir, mut runtime, mut session) = session_with(
        Box::new(ScriptedDecisionProvider::accept_all()),
        ScriptedLanguageProvider::fast_good("primary"),
        vec![(
            "extra".to_owned(),
            ScriptedLanguageProvider::slow_good("extra"),
        )],
    );
    let mut turn_ids = std::collections::BTreeSet::new();
    for (index, text) in [
        "こんにちは",
        "調子はどう？",
        "エラーが出ました",
        "続きを教えて",
        "ありがとう",
        "もう一度確認して",
    ]
    .iter()
    .enumerate()
    {
        let reply = session.turn(&mut runtime, text, |_| Ok(())).unwrap();
        assert!(turn_ids.insert(reply.turn_id));
        let trace = reply.llm_jev.unwrap();
        assert_eq!(trace.core_state_after.turn, index as u64 + 1);
        assert_eq!(trace.response_gate.decision, Decision::Accept);
    }
    assert_eq!(session.turn_count(), 6);
    let history = session.history_for_display(&runtime).unwrap();
    assert!(history.len() >= 4);
}
