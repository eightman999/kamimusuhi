//! Poll explicitly configured subjects for bounded idle cognition.
//!
//! The runtime owns the idle policy, durable cursor, backoff, budget and
//! individual lock. The resident supplies its reviewed routing declaration
//! and observes the result. It never invents a user turn, sends a message, or
//! executes a model-selected tool as a consequence of this scheduler.

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::state::Shared;
use crate::util::{iso8601, run_with_timeout_env, unix_now};

const MAX_REPORT_BYTES: usize = 256 * 1024;

fn set(shared: &Shared, mut report: Value) -> Value {
    let mut current = shared.cognition.write().unwrap_or_else(|p| p.into_inner());
    for key in ["last_result", "last_run"] {
        if report.get(key).is_none()
            && let Some(previous) = current.get(key)
        {
            report[key] = previous.clone();
        }
    }
    *current = report.clone();
    report
}

fn outcome(shared: &Shared, subject: &str, state: &str, reason: &str, started: Instant) -> Value {
    publish(shared, report(subject, state, reason, started))
}

fn report(subject: &str, state: &str, reason: &str, started: Instant) -> Value {
    json!({
        "state": state,
        "reason": reason,
        "subject": subject,
        "last_run": iso8601(unix_now()),
        "latency_ms": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

fn publish(shared: &Shared, mut report: Value) -> Value {
    let _ = shared.spool.append("logs/cognition", report.clone());
    report["last_result"] = report.clone();
    set(shared, report)
}

/// Start one sequential poller. An absent configuration performs no work.
pub fn spawn(shared: &Arc<Shared>) {
    let Some(config) = shared.config.cognition.clone() else {
        set(shared, json!({"state": "disabled"}));
        return;
    };
    set(
        shared,
        json!({"state": "idle", "reason": "COGITATION_AWAITING_POLL"}),
    );
    let worker = Arc::clone(shared);
    let spawned = thread::Builder::new()
        .name("idle-cognition".to_owned())
        .spawn(move || {
            let mut next_subject = 0;
            loop {
                poll_cycle(&worker, &config, &mut next_subject);
                // This is a lightweight poll. The runtime decides when an API
                // call is due; no extra cadence or budget is kept in the daemon.
                thread::sleep(Duration::from_secs(config.poll_interval_secs));
            }
        });
    if spawned.is_err() {
        set(
            shared,
            json!({"state":"failed", "reason":"COGITATION_THREAD_START_FAILED"}),
        );
    }
}

fn poll_cycle(shared: &Shared, config: &crate::config::CognitionConfig, next_subject: &mut usize) {
    for offset in 0..config.subjects.len() {
        let index = (*next_subject + offset) % config.subjects.len();
        let result = tick(
            shared,
            &config.subjects[index],
            Duration::from_secs(config.timeout_secs),
        );
        if result["state"] == "completed" || result["state"] == "failed" {
            // Global runtime budgets can admit only one subject. Advance only
            // after an actual attempt, so fixed cooldown polling cannot keep
            // the first subject at the head forever.
            *next_subject = (index + 1) % config.subjects.len();
            break;
        }
        if result["state"] == "busy" {
            break;
        }
    }
}

/// One scheduler observation, kept separate so fixtures can exercise the real
/// child protocol without starting an immortal poller.
pub(crate) fn tick(shared: &Shared, subject: &str, timeout: Duration) -> Value {
    let started = Instant::now();
    let Some(cognition) = &shared.config.cognition else {
        return set(shared, json!({"state": "disabled"}));
    };
    if !cognition.subjects.iter().any(|entry| entry == subject)
        || !crate::dialogue::valid_subject(subject)
    {
        return outcome(
            shared,
            subject,
            "failed",
            "COGITATION_SUBJECT_NOT_CONFIGURED",
            started,
        );
    }
    if !shared
        .turns
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .is_empty()
    {
        // Do not retain the turns mutex across an API call: the route proxy
        // needs it to attribute foreground work. New foreground turns remain
        // free to arrive while the bounded runtime attempt is in flight.
        return set(
            shared,
            json!({
                "state": "busy", "subject": subject,
                "reason": "COGITATION_FOREGROUND_ACTIVE",
                "checked_at": iso8601(unix_now()),
            }),
        );
    }
    let Some(dialogue) = &shared.config.dialogue else {
        return outcome(
            shared,
            subject,
            "failed",
            "COGITATION_DIALOGUE_UNCONFIGURED",
            started,
        );
    };
    if !dialogue.dir.join("kamimusuhi.sqlite").is_file() {
        return outcome(
            shared,
            subject,
            "failed",
            "COGITATION_INDIVIDUAL_UNINITIALIZED",
            started,
        );
    }
    let context = match crate::identity::context(shared) {
        Ok(context) => context.to_string(),
        Err(_) => {
            return outcome(
                shared,
                subject,
                "failed",
                "COGITATION_IDENTITY_UNAVAILABLE",
                started,
            );
        }
    };
    let mut env = vec![(crate::identity::CONTEXT_ENV, context.as_str())];
    if let Some(token) = shared.token.as_deref() {
        // No credential enters argv, output, status, or the cognition journal.
        env.push((shared.config.node.token_env.as_str(), token));
    }
    let dir = dialogue.dir.to_string_lossy();
    let binary = dialogue.runtime_binary.to_string_lossy();
    let args = [
        "cogitate",
        "--dir",
        dir.as_ref(),
        "--subject",
        subject,
        "--privacy",
        dialogue.privacy.as_str(),
    ];
    set(
        shared,
        json!({
            "state": "running", "subject": subject,
            "reason": "COGITATION_IN_PROGRESS", "since": iso8601(unix_now()),
        }),
    );
    let Some(output) = run_with_timeout_env(&binary, &args, &env, timeout) else {
        return outcome(
            shared,
            subject,
            "failed",
            "COGITATION_TIMEOUT_OR_START_FAILED",
            started,
        );
    };
    if !output.success {
        return outcome(
            shared,
            subject,
            "failed",
            "COGITATION_PROCESS_FAILED",
            started,
        );
    }
    if output.stdout.len() > MAX_REPORT_BYTES {
        return outcome(
            shared,
            subject,
            "failed",
            "COGITATION_REPORT_TOO_LARGE",
            started,
        );
    }
    let parsed = serde_json::from_str::<Value>(&output.stdout);
    let Ok(result) = parsed else {
        return outcome(
            shared,
            subject,
            "failed",
            "COGITATION_INVALID_REPORT",
            started,
        );
    };
    if result.get("subject").and_then(Value::as_str) != Some(subject) {
        return outcome(
            shared,
            subject,
            "failed",
            "COGITATION_SUBJECT_MISMATCH",
            started,
        );
    }
    let Some((state, reason)) = result
        .get("status")
        .and_then(Value::as_str)
        .and_then(classify)
    else {
        return outcome(
            shared,
            subject,
            "failed",
            "COGITATION_INVALID_REPORT",
            started,
        );
    };
    let mut report = report(subject, state, reason, started);
    report["runtime_status"] = result["status"].clone();
    if let Some(reason) = result.get("reason").and_then(Value::as_str)
        && [
            "COGNITION_BUSY",
            "NO_EXPERIENCE",
            "WAIT_FOR_IDLE",
            "NO_NEW_EXPERIENCE",
            "MIN_INTERVAL",
            "DAILY_BUDGET",
            "ATTEMPTS_EXHAUSTED",
            "RETRY_BACKOFF",
            "INTERRUPTED_UNCERTAIN",
            "REFLECTED",
        ]
        .contains(&reason)
    {
        report["runtime_reason"] = json!(reason);
    }
    // Keep operational metadata only. Reflection notes, model output and raw
    // stderr are deliberately absent from status and delivery journals.
    if let Some(digest) = result.get("source_digest").and_then(Value::as_str)
        && valid_digest(digest)
    {
        report["source_digest"] = json!(digest);
    }
    if let Some(reflection) = result.get("reflection").filter(|value| value.is_object()) {
        let mut summary = serde_json::Map::new();
        for key in [
            "episodes_presented",
            "corrections_linked",
            "canonical_drafts_submitted",
            "canonical_drafts_activated",
        ] {
            if let Some(count) = reflection.get(key).and_then(Value::as_u64) {
                summary.insert(key.to_owned(), json!(count));
            }
        }
        for (field, key) in [
            ("notes", "notes_count"),
            ("proposals_created", "proposals_created_count"),
            ("drafts_rejected", "drafts_rejected_count"),
        ] {
            if let Some(entries) = reflection.get(field).and_then(Value::as_array) {
                summary.insert(key.to_owned(), json!(entries.len()));
            }
        }
        report["reflection"] = Value::Object(summary);
    }
    publish(shared, report)
}

fn classify(status: &str) -> Option<(&'static str, &'static str)> {
    Some(match status {
        "completed" => ("completed", "COGITATION_COMPLETED"),
        "idle" => ("idle", "COGITATION_NO_WORK"),
        "cooldown" => ("backoff", "COGITATION_COOLDOWN"),
        "exhausted" => ("backoff", "COGITATION_BUDGET_EXHAUSTED"),
        "busy" => ("busy", "COGITATION_RUNTIME_BUSY"),
        "interrupted" => ("backoff", "COGITATION_INTERRUPTED"),
        _ => return None,
    })
}

fn valid_digest(digest: &str) -> bool {
    digest.strip_prefix("sha256:").is_some_and(|value| {
        value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn shared(root: &Path, script: &str) -> Shared {
        let runtime = root.join("individual");
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::write(runtime.join("kamimusuhi.sqlite"), []).unwrap();
        let binary = root.join("fixture-runtime");
        std::fs::write(&binary, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let config = serde_json::from_value(json!({
            "node": {"id":"pi", "role":"continuity", "listen":"127.0.0.1:7860", "token_env":"COGNITION_FIXTURE_TOKEN"},
            "paths": {"root":root},
            "dialogue": {"runtime_binary":binary, "dir":runtime, "privacy":"unconstrained"},
            "cognition": {"subjects":["alice"]},
        })).unwrap();
        let spool = crate::spool::Spool::new(root.join("spool"), "pi").unwrap();
        Shared::new(
            config,
            spool,
            1,
            Some("fixture-non-secret-token".to_owned()),
        )
    }

    fn reply(value: Value) -> String {
        format!(
            "printf '%s\\n' '{}'",
            value.to_string().replace('\'', "'\\''")
        )
    }

    #[test]
    fn unconfigured_and_foreground_busy_do_not_start_a_child() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("started");
        let script = format!(
            "touch '{}'\n{}",
            marker.display(),
            reply(json!({"status":"idle", "subject":"alice"}))
        );
        let mut shared = shared(dir.path(), &script);
        shared.begin_turn("foreground");
        let report = tick(&shared, "alice", Duration::from_secs(10));
        assert_eq!(report["state"], "busy");
        assert_eq!(report["reason"], "COGITATION_FOREGROUND_ACTIVE");
        assert!(!marker.exists());
        shared.end_turn("foreground");
        shared.config.cognition = None;
        assert_eq!(
            tick(&shared, "alice", Duration::from_secs(10))["state"],
            "disabled"
        );
        assert!(!marker.exists());
    }

    #[test]
    fn reviewed_context_privacy_and_token_reach_only_the_child_environment() {
        let dir = tempfile::tempdir().unwrap();
        let args_path = dir.path().join("args");
        let context_path = dir.path().join("context");
        let script = format!(
            "test -n \"$KAMIMUSUHI_IDENTITY_DECLARATIONS\" || exit 1\n\
             test \"$COGNITION_FIXTURE_TOKEN\" = fixture-non-secret-token || exit 2\n\
             printf '%s\\n' \"$@\" > '{}'\n\
             printf '%s\\n' \"$KAMIMUSUHI_IDENTITY_DECLARATIONS\" > '{}'\n{}",
            args_path.display(),
            context_path.display(),
            reply(
                json!({"status":"completed", "subject":"alice", "reason":"RAW_MODEL_TEXT", "source_digest":format!("sha256:{}", "a".repeat(64)),
                "reflection":{"episodes_presented":2, "notes":["PRIVATE_REFLECTION_TEXT"], "canonical_drafts_submitted":0, "proposals_created":[]}})
            ),
        );
        let shared = shared(dir.path(), &script);
        let report = tick(&shared, "alice", Duration::from_secs(10));
        assert_eq!(report["state"], "completed");
        assert_eq!(report["reason"], "COGITATION_COMPLETED");
        assert_eq!(report["reflection"]["episodes_presented"], 2);
        assert_eq!(report["reflection"]["notes_count"], 1);
        let args = std::fs::read_to_string(args_path).unwrap();
        assert_eq!(args.lines().next(), Some("cogitate"));
        assert!(args.contains("--subject\nalice\n--privacy\nunconstrained"));
        assert!(!args.contains("fixture-non-secret-token"));
        let context: Value = serde_json::from_slice(&std::fs::read(context_path).unwrap()).unwrap();
        assert_eq!(context["version"], 1);
        assert!(
            context["declarations"]["self"]
                .as_str()
                .unwrap()
                .starts_with("sha256:")
        );
        let status = shared.status();
        assert_eq!(status["cognition"], report);
        let visible = status["cognition"].to_string();
        assert!(!visible.contains("PRIVATE_REFLECTION_TEXT"));
        assert!(!visible.contains("RAW_MODEL_TEXT"));
        assert!(!visible.contains("fixture-non-secret-token"));
        assert!(
            crate::status::render(&status)
                .0
                .contains("COGNITION  state=completed")
        );
        shared.begin_turn("foreground");
        let busy = tick(&shared, "alice", Duration::from_secs(10));
        assert_eq!(busy["state"], "busy");
        assert_eq!(busy["last_result"]["state"], "completed");
        shared.end_turn("foreground");
    }

    #[test]
    fn all_runtime_wait_results_are_observable_without_model_prose() {
        for (status, expected) in [
            ("idle", "idle"),
            ("cooldown", "backoff"),
            ("exhausted", "backoff"),
            ("busy", "busy"),
            ("interrupted", "backoff"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let shared = shared(
                dir.path(),
                &reply(json!({"status":status, "subject":"alice", "reason":"PRIVATE_REASON"})),
            );
            let report = tick(&shared, "alice", Duration::from_secs(10));
            assert_eq!(report["state"], expected, "{status}");
            assert_eq!(report["runtime_status"], status);
            assert!(!report.to_string().contains("PRIVATE_REASON"));
        }
    }

    #[test]
    fn successful_subject_advances_fairly_but_cooldown_only_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let script = "subject=\nwhile [ \"$#\" -gt 0 ]; do\n  if [ \"$1\" = --subject ]; then shift; subject=$1; fi\n  shift\ndone\nprintf '{\"status\":\"completed\",\"subject\":\"%s\",\"reason\":\"REFLECTED\"}\\n' \"$subject\"";
        let mut shared = shared(dir.path(), script);
        shared
            .config
            .cognition
            .as_mut()
            .unwrap()
            .subjects
            .push("bob".to_owned());
        let config = shared.config.cognition.clone().unwrap();
        let mut next = 0;
        poll_cycle(&shared, &config, &mut next);
        assert_eq!(shared.status()["cognition"]["subject"], "alice");
        assert_eq!(next, 1);
        poll_cycle(&shared, &config, &mut next);
        assert_eq!(shared.status()["cognition"]["subject"], "bob");
        assert_eq!(next, 0);
        let bin = shared
            .config
            .dialogue
            .as_ref()
            .unwrap()
            .runtime_binary
            .clone();
        std::fs::write(
            bin,
            format!("#!/bin/sh\n{}\n", script.replace("completed", "cooldown")),
        )
        .unwrap();
        poll_cycle(&shared, &config, &mut next);
        assert_eq!(
            next, 0,
            "poll cadence cannot rotate the runtime budget winner"
        );
        assert_eq!(shared.status()["cognition"]["state"], "backoff");
    }

    #[test]
    fn invalid_reports_and_child_failures_publish_stable_errors_only() {
        for (script, reason) in [
            (
                "printf '%s' not-json; printf '%s' PRIVATE_STDERR >&2".to_owned(),
                "COGITATION_INVALID_REPORT",
            ),
            (
                reply(json!({"status":"completed", "subject":"other"})),
                "COGITATION_SUBJECT_MISMATCH",
            ),
            (
                reply(json!({"status":"invented", "subject":"alice"})),
                "COGITATION_INVALID_REPORT",
            ),
            (
                "printf '%s' PRIVATE_STDERR >&2; exit 2".to_owned(),
                "COGITATION_PROCESS_FAILED",
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let shared = shared(dir.path(), &script);
            let report = tick(&shared, "alice", Duration::from_secs(10));
            assert_eq!(report["state"], "failed");
            assert_eq!(report["reason"], reason);
            assert!(!report.to_string().contains("PRIVATE_STDERR"));
        }
    }

    #[test]
    fn deadline_uninitialized_and_unknown_subject_never_disclose_child_details() {
        let dir = tempfile::tempdir().unwrap();
        let shared = shared(dir.path(), "exec /bin/sleep 2");
        let before = Instant::now();
        let report = tick(&shared, "alice", Duration::from_millis(30));
        assert_eq!(report["reason"], "COGITATION_TIMEOUT_OR_START_FAILED");
        assert!(before.elapsed() < Duration::from_secs(1));
        assert_eq!(
            tick(&shared, "unconfigured", Duration::from_secs(1))["reason"],
            "COGITATION_SUBJECT_NOT_CONFIGURED"
        );
        std::fs::remove_file(
            shared
                .config
                .dialogue
                .as_ref()
                .unwrap()
                .dir
                .join("kamimusuhi.sqlite"),
        )
        .unwrap();
        assert_eq!(
            tick(&shared, "alice", Duration::from_secs(1))["reason"],
            "COGITATION_INDIVIDUAL_UNINITIALIZED"
        );
    }

    #[test]
    fn cognition_config_requires_explicit_unique_subjects_dialogue_and_bounded_limits() {
        let dir = tempfile::tempdir().unwrap();
        let base = shared(dir.path(), "exit 0").config;
        let config = base.cognition.as_ref().unwrap();
        assert_eq!(config.poll_interval_secs, 30);
        assert_eq!(config.timeout_secs, 180);
        base.validate().unwrap();
        for subjects in [vec![], vec!["alice", "alice"], vec!["--dir"], vec!["a b"]] {
            let mut config = base.clone();
            config.cognition.as_mut().unwrap().subjects =
                subjects.into_iter().map(str::to_owned).collect();
            assert!(config.validate().is_err());
        }
        let mut config = base.clone();
        config.dialogue = None;
        assert!(config.validate().is_err());
        for (poll, timeout) in [(0, 180), (3601, 180), (30, 0), (30, 601)] {
            let mut config = base.clone();
            let cognition = config.cognition.as_mut().unwrap();
            cognition.poll_interval_secs = poll;
            cognition.timeout_secs = timeout;
            assert!(config.validate().is_err());
        }
    }
}
