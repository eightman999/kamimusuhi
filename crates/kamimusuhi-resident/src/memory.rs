//! `GET|POST /v1/kamimusuhi/memory`: a read-only view of the individual's
//! memory for the operator dashboard.
//!
//! The resident never opens the canonical store itself — the same rule as
//! dialogue: it runs `kamimusuhi-runtime memory` against the runtime
//! directory, which opens the store without a writer epoch (SELECTs only).
//! Answers are cached briefly so a polling dashboard cannot spawn a runtime
//! process per request.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::state::Shared;
use crate::util::run_with_timeout_env;

/// How long one spawned report may be reused. Dashboards poll every 10-30s;
/// this bounds the spawn rate to roughly once per two polls per viewer.
const CACHE_TTL: Duration = Duration::from_secs(20);
/// A wedged store open must not pin an HTTP thread.
const SPAWN_TIMEOUT: Duration = Duration::from_secs(45);

/// Last (cache key, fetched-at, body) — one slot is enough; viewers asking
/// for different subjects simply miss the cache.
static CACHE: Mutex<Option<(String, Instant, Value)>> = Mutex::new(None);

/// Returns (HTTP status, body).
pub fn memory(shared: &Shared, request: &Value) -> (u16, Value) {
    let Some(config) = &shared.config.dialogue else {
        return (
            404,
            json!({"error": "memory is served by the continuity node"}),
        );
    };
    let subject = request["subject"].as_str().map(str::to_owned);
    if let Some(subject) = subject.as_deref()
        && !crate::dialogue::valid_subject(subject)
    {
        return (
            400,
            json!({"error": "subject must be 1-64 chars of [A-Za-z0-9._-]"}),
        );
    }
    let limit = request["limit"].as_u64().unwrap_or(24).clamp(1, 200);
    if !config.dir.join("kamimusuhi.sqlite").is_file() {
        return (
            503,
            json!({"error": "individual is not initialized on this node"}),
        );
    }
    let key = format!("{}\x1f{limit}", subject.as_deref().unwrap_or(""));
    {
        let cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((cached_key, at, body)) = cache.as_ref()
            && *cached_key == key
            && at.elapsed() < CACHE_TTL
        {
            return (200, body.clone());
        }
    }
    let dir = config.dir.to_string_lossy().into_owned();
    let binary = config.runtime_binary.to_string_lossy().into_owned();
    let limit_arg = limit.to_string();
    let mut args = vec![
        "memory",
        "--dir",
        dir.as_str(),
        "--limit",
        limit_arg.as_str(),
    ];
    if let Some(subject) = subject.as_deref() {
        args.push("--subject");
        args.push(subject);
    }
    let Some(output) = run_with_timeout_env(&binary, &args, &[], SPAWN_TIMEOUT) else {
        return (
            504,
            json!({"error": "memory report timed out or could not start"}),
        );
    };
    let report = output
        .stdout
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<Value>(line).ok());
    match report {
        Some(mut report) if output.success => {
            report["cached_for_secs"] = json!(CACHE_TTL.as_secs());
            let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
            *cache = Some((key, Instant::now(), report.clone()));
            (200, report)
        }
        _ => {
            let tail: Vec<&str> = output.stderr.lines().rev().take(5).collect();
            let tail: Vec<&str> = tail.into_iter().rev().collect();
            (
                502,
                json!({"error": "memory report failed", "stderr_tail": tail}),
            )
        }
    }
}
