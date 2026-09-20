//! Seeded long-run soak for the dialogue loop.
//!
//! A fixed-seed mix of latency variation, timeouts, rejections, retries,
//! provider failures and Jev outages drives the real turn path — evidence
//! writes, trace, gates, race and delivery — for 1,000 turns (10,000 with
//! `KAMIMUSUHI_SOAK_TURNS=10000`). The loop must never crash, deadlock,
//! starve an organ, leak threads or grow memory without bound.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use kamimusuhi_core::routing::PrivacyConstraint;
use kamimusuhi_core::trace::TraceEventKind;
use kamimusuhi_runtime::dialogue::DialogueSession;
use kamimusuhi_runtime::llm_jev::testing::{
    OutcomeWeights, SeededDecisionProvider, SeededLanguageProvider, VerdictWeights,
};
use kamimusuhi_runtime::trace::JsonlTraceSink;
use kamimusuhi_runtime::{ResourceImplementation, Runtime, RuntimeOptions, read_trace};

const DEFAULT_TURNS: usize = 1_000;

fn rss_kib() -> Option<u64> {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    String::from_utf8(output.stdout).ok()?.trim().parse().ok()
}

fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let index = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

#[test]
fn seeded_dialogue_soak() {
    let turns: usize = std::env::var("KAMIMUSUHI_SOAK_TURNS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_TURNS);

    let dir = tempfile::tempdir().unwrap();
    // Rotation disabled: with `Some(0)` every event stays in one file, so the
    // completeness check below is exact for any turn count — including the
    // 10,000-turn configuration, which would otherwise rotate past the single
    // retained generation and legitimately drop early events.
    let mut runtime = Runtime::init(
        dir.path(),
        RuntimeOptions {
            trace_max_bytes: Some(0),
            ..RuntimeOptions::deterministic(77)
        },
        ResourceImplementation::FakeA,
    )
    .unwrap();

    let organs = [
        SeededLanguageProvider::new(
            "organ-a",
            0xA11CE,
            (1, 12),
            OutcomeWeights {
                error_pct: 4,
                empty_pct: 2,
                oversized_pct: 1,
                hang_pct: 2,
                hang_deadline: Duration::from_millis(400),
            },
        ),
        SeededLanguageProvider::new(
            "organ-b",
            0xB0B,
            (3, 30),
            OutcomeWeights {
                error_pct: 8,
                empty_pct: 1,
                oversized_pct: 0,
                hang_pct: 3,
                hang_deadline: Duration::from_millis(400),
            },
        ),
        SeededLanguageProvider::new(
            "organ-c",
            0xC0FFEE,
            (1, 20),
            OutcomeWeights {
                error_pct: 6,
                empty_pct: 2,
                oversized_pct: 1,
                hang_pct: 0,
                hang_deadline: Duration::from_millis(400),
            },
        ),
    ];
    let gauges: Vec<_> = organs.iter().map(|organ| organ.active()).collect();
    let call_gauges: Vec<_> = organs.iter().map(|organ| organ.calls()).collect();
    let mut extras = organs.into_iter();
    let primary = extras.next().unwrap();

    let mut session = DialogueSession::start_with_providers(
        &mut runtime,
        "alice",
        PrivacyConstraint::LocalOnly,
        Box::new(SeededDecisionProvider::new(
            0xDEC1DE,
            5,
            VerdictWeights {
                retry_pct: 8,
                reject_pct: 7,
                outage_pct: 5,
            },
        )),
        Some(Box::new(primary)),
    )
    .unwrap();
    for (index, organ) in extras.enumerate() {
        session
            .add_language_provider(&format!("organ-{index}"), Arc::new(organ))
            .unwrap();
    }
    session.set_debug_trace(true);

    let rss_before = rss_kib();
    let started = Instant::now();
    let mut latencies = Vec::with_capacity(turns);
    let mut replies = 0_usize;
    let mut controlled_errors = BTreeMap::<String, usize>::new();
    let mut retries = 0_u64;
    let mut degraded_turns = 0_usize;
    let mut late_attempts = 0_usize;
    let mut emitted = 0_usize;

    for index in 0..turns {
        let text = match index % 4 {
            0 => format!("こんにちは {index}"),
            1 => format!("エラー確認 {index}"),
            2 => format!("調子はどう？ {index}"),
            _ => format!("続き {index}"),
        };
        let turn_started = Instant::now();
        match session.turn(&mut runtime, &text, |_| {
            emitted += 1;
            Ok(())
        }) {
            Ok(reply) => {
                replies += 1;
                assert!(
                    !reply.response.is_empty(),
                    "turn {index}: empty delivered response"
                );
                if let Some(trace) = reply.llm_jev {
                    retries += u64::from(trace.retry_count);
                    if !trace.decision_fallbacks.is_empty() {
                        degraded_turns += 1;
                    }
                    late_attempts += trace.late_candidates.len();
                }
            }
            Err(error) => {
                // Controlled failures only: a gate verdict, an unavailable
                // provider pool or a refused contract — never a panic.
                *controlled_errors
                    .entry(
                        format!("{error:?}")
                            .split('(')
                            .next()
                            .unwrap_or("?")
                            .to_owned(),
                    )
                    .or_default() += 1;
            }
        }
        latencies.push(u64::try_from(turn_started.elapsed().as_millis()).unwrap_or(u64::MAX));
    }
    let elapsed = started.elapsed();
    let rss_after = rss_kib();

    // Every organ thread must have exited: cancellation and internal
    // deadlines bound them all, so a nonzero gauge means a real leak.
    let leak_deadline = Instant::now() + Duration::from_secs(5);
    while gauges.iter().any(|gauge| gauge.load(Ordering::SeqCst) != 0)
        && Instant::now() < leak_deadline
    {
        std::thread::sleep(Duration::from_millis(5));
    }
    for (index, gauge) in gauges.iter().enumerate() {
        assert_eq!(
            gauge.load(Ordering::SeqCst),
            0,
            "organ {index} still has in-flight calls"
        );
    }
    // No organ was starved: every registered provider was invoked roughly
    // every speaking turn.
    let calls: Vec<usize> = call_gauges
        .iter()
        .map(|gauge| gauge.load(Ordering::SeqCst))
        .collect();
    for (index, count) in calls.iter().enumerate() {
        assert!(
            *count >= replies / 2,
            "organ {index} was starved: {count} calls for {replies} replies"
        );
    }

    latencies.sort_unstable();
    let summary = serde_json::json!({
        "turns": turns,
        "replies": replies,
        "controlled_errors": controlled_errors,
        "emitted": emitted,
        "retries": retries,
        "degraded_turns": degraded_turns,
        "late_attempts": late_attempts,
        "elapsed_ms": elapsed.as_millis(),
        "turn_latency_ms": {
            "p50": percentile(&latencies, 0.50),
            "p95": percentile(&latencies, 0.95),
            "p99": percentile(&latencies, 0.99),
        },
        "rss_kib": { "before": rss_before, "after": rss_after },
        "provider_calls": calls,
    });
    println!("{summary}");

    // Trace completeness: one turn.started event per attempted turn. Rotation
    // is disabled for this run, so the live file holds every event and the
    // rotated path must not exist.
    let trace_path = dir.path().join(JsonlTraceSink::FILE_NAME);
    assert!(
        !trace_path.with_extension("jsonl.1").exists(),
        "rotation disabled: no previous generation may appear"
    );
    let mut turn_starts = 0_usize;
    if let Ok(events) = read_trace(&trace_path) {
        turn_starts += events
            .iter()
            .filter(|event| event.event_kind == TraceEventKind::TurnStarted)
            .count();
    }
    assert_eq!(
        turn_starts, turns,
        "every attempted turn must open a trace event"
    );

    // The conversation actually converged: a seeded 5% WAIT plus retry and
    // reject verdicts must not eat most of the run.
    assert!(
        replies * 2 >= turns,
        "only {replies}/{turns} turns produced a reply"
    );
    assert_eq!(emitted, replies, "emission fires once per reply");
    if let (Some(before), Some(after)) = (rss_before, rss_after) {
        assert!(
            after < before + 512 * 1024,
            "RSS grew unboundedly: {before} -> {after} KiB"
        );
    }
    assert!(
        elapsed < Duration::from_secs(600),
        "soak overran its bound: {elapsed:?}"
    );
}
