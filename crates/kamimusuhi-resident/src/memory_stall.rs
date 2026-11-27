//! Periodically read the runtime's memory stall signal into `/status`.
//!
//! Canonical memory grows only when a proposal (mostly from the reflection
//! cycle) is accepted. If the reflector breaks, is not set to run, or every
//! proposal is refused, the conversation still works and memory silently
//! stops growing. This poller makes that visible on the existing operator
//! surface: the `memory_stall` block of `GET /status`, the `MEMORY` line of
//! `kamimusuhi status` (which then exits non-zero), and a `logs/memory_stall`
//! journal line whenever the verdict changes.
//!
//! Like the memory endpoint, the resident never opens the canonical store
//! itself: it runs `kamimusuhi-runtime memory-stall`, which opens the store
//! without a writer epoch and only runs `SELECT`s. The poller only observes.
//! It never triggers a reflection, retries anything, or edits configuration.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serde_json::{Map, Value, json};

use crate::state::Shared;
use crate::util::{iso8601, run_with_timeout_env, unix_now};

const MAX_REPORT_BYTES: usize = 64 * 1024;

/// Fields copied from the runtime report. All are codes, counts or
/// timestamps — the runtime report holds no content, and anything it might
/// add later stays out of `/status` until listed here.
const FIELDS: &[&str] = &[
    "state",
    "reason",
    "thresholds",
    "baseline",
    "baseline_at",
    "last_memory_at",
    "state_records_total",
    "quiet_secs",
    "user_utterances_since_memory",
    "last_user_utterance_at",
    "ephemeral_user_utterances_since_memory",
    "reflections_since_memory",
    "last_reflection_at",
    "reflection_drafts_rejected",
    "memory_proposals",
    "memory_decisions",
    "c0_proposals",
];

fn read(shared: &Shared) -> Value {
    shared
        .memory_stall
        .read()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
}

fn verdict(value: &Value) -> Option<&str> {
    value
        .get("state")
        .and_then(Value::as_str)
        .filter(|s| matches!(*s, "ok" | "stalled"))
}

/// Store `report`, journaling only when the verdict changes so a healthy
/// node does not grow the spool every interval.
fn publish(shared: &Shared, mut report: Value) -> Value {
    let previous = read(shared);
    if verdict(&report).is_none()
        && let Some(last) = previous
            .get("last_verdict")
            .cloned()
            .or_else(|| verdict(&previous).map(|_| previous.clone()))
    {
        // A failed check does not erase what was last known.
        report["last_verdict"] = last;
    }
    let changed = previous.get("state") != report.get("state")
        || previous.get("reason") != report.get("reason");
    if changed {
        let _ = shared.spool.append("logs/memory_stall", report.clone());
    }
    *shared
        .memory_stall
        .write()
        .unwrap_or_else(|p| p.into_inner()) = report.clone();
    report
}

fn unavailable(shared: &Shared, reason: &str) -> Value {
    publish(
        shared,
        json!({"state": "unavailable", "reason": reason, "checked_at": iso8601(unix_now())}),
    )
}

/// Start the poller. Disabled, or without a dialogue (no individual on this
/// node), it does nothing.
pub fn spawn(shared: &Arc<Shared>) {
    let config = shared.config.memory_stall.clone();
    if !config.enabled {
        publish(shared, json!({"state": "disabled"}));
        return;
    }
    if shared.config.dialogue.is_none() {
        publish(
            shared,
            json!({"state": "disabled", "reason": "MEMORY_STALL_DIALOGUE_UNCONFIGURED"}),
        );
        return;
    }
    let worker = Arc::clone(shared);
    let spawned = thread::Builder::new()
        .name("memory-stall".to_owned())
        .spawn(move || {
            loop {
                check(&worker);
                thread::sleep(Duration::from_secs(config.check_interval_secs));
            }
        });
    if spawned.is_err() {
        unavailable(shared, "MEMORY_STALL_THREAD_START_FAILED");
    }
}

/// One check: run the runtime's read-only report and publish the verdict.
pub fn check(shared: &Shared) -> Value {
    let config = &shared.config.memory_stall;
    let Some(dialogue) = &shared.config.dialogue else {
        return publish(
            shared,
            json!({"state": "disabled", "reason": "MEMORY_STALL_DIALOGUE_UNCONFIGURED"}),
        );
    };
    if !dialogue.dir.join("kamimusuhi.sqlite").is_file() {
        return unavailable(shared, "MEMORY_STALL_INDIVIDUAL_UNINITIALIZED");
    }
    let dir = dialogue.dir.to_string_lossy().into_owned();
    let binary = dialogue.runtime_binary.to_string_lossy().into_owned();
    let utterances = config.min_user_utterances.to_string();
    let quiet = config.min_quiet_secs.to_string();
    let args = [
        "memory-stall",
        "--dir",
        dir.as_str(),
        "--min-user-utterances",
        utterances.as_str(),
        "--min-quiet-secs",
        quiet.as_str(),
    ];
    let Some(output) = run_with_timeout_env(
        &binary,
        &args,
        &[],
        Duration::from_secs(config.timeout_secs),
    ) else {
        return unavailable(shared, "MEMORY_STALL_TIMEOUT_OR_START_FAILED");
    };
    if !output.success {
        return unavailable(shared, "MEMORY_STALL_PROCESS_FAILED");
    }
    if output.stdout.len() > MAX_REPORT_BYTES {
        return unavailable(shared, "MEMORY_STALL_REPORT_TOO_LARGE");
    }
    let Ok(parsed) = serde_json::from_str::<Value>(&output.stdout) else {
        return unavailable(shared, "MEMORY_STALL_INVALID_REPORT");
    };
    if verdict(&parsed).is_none() || !parsed.get("reason").is_some_and(Value::is_string) {
        return unavailable(shared, "MEMORY_STALL_INVALID_REPORT");
    }
    let mut report = Map::new();
    for key in FIELDS {
        if let Some(value) = parsed.get(*key) {
            report.insert((*key).to_owned(), value.clone());
        }
    }
    report.insert("checked_at".to_owned(), json!(iso8601(unix_now())));
    publish(shared, Value::Object(report))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn shared(root: &Path, script: &str, extra: Value) -> Shared {
        let runtime = root.join("individual");
        std::fs::create_dir_all(&runtime).unwrap();
        std::fs::write(runtime.join("kamimusuhi.sqlite"), []).unwrap();
        let binary = root.join("fixture-runtime");
        std::fs::write(&binary, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut config = json!({
            "node": {"id":"pi", "role":"continuity", "listen":"127.0.0.1:7860"},
            "paths": {"root":root},
            "dialogue": {"runtime_binary":binary, "dir":runtime, "privacy":"unconstrained"},
        });
        if let Some(extra) = extra.as_object() {
            for (k, v) in extra {
                config[k] = v.clone();
            }
        }
        let config: crate::config::Config = serde_json::from_value(config).unwrap();
        config.validate().unwrap();
        let spool = crate::spool::Spool::new(root.join("spool"), "pi").unwrap();
        Shared::new(config, spool, 1, None)
    }

    fn reply(value: &Value) -> String {
        format!(
            "printf '%s\\n' '{}'",
            value.to_string().replace('\'', "'\\''")
        )
    }

    fn report(state: &str, reason: &str) -> Value {
        json!({
            "state": state, "reason": reason,
            "individual_id": "00000000000000000000000000000001",
            "generated_at": "2026-11-27T00:00:00Z",
            "thresholds": {"min_user_utterances": 40, "min_quiet_secs": 172800},
            "baseline": "last_memory", "baseline_at": "2026-11-20T00:00:00Z",
            "last_memory_at": "2026-11-20T00:00:00Z", "state_records_total": 9,
            "quiet_secs": 604800, "user_utterances_since_memory": 55,
            "last_user_utterance_at": "2026-11-26T23:00:00Z",
            "ephemeral_user_utterances_since_memory": 312,
            "reflections_since_memory": 0, "last_reflection_at": null,
            "reflection_drafts_rejected": 0, "memory_proposals": 0,
            "memory_decisions": {}, "c0_proposals": {},
            "future_field_with_text": "PRIVATE_TEXT",
        })
    }

    #[test]
    fn defaults_are_on_conservative_and_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let base = shared(dir.path(), "exit 0", json!({})).config;
        let config = &base.memory_stall;
        assert!(config.enabled);
        assert_eq!(config.min_user_utterances, 40);
        assert_eq!(config.min_quiet_secs, 172_800);
        assert_eq!(config.check_interval_secs, 900);
        for patch in [
            json!({"min_user_utterances": 0}),
            json!({"check_interval_secs": 59}),
            json!({"timeout_secs": 0}),
            json!({"min_quiet_secs": 400 * 86_400}),
        ] {
            let mut value = serde_json::to_value(&base).unwrap();
            for (k, v) in patch.as_object().unwrap() {
                value["memory_stall"][k] = v.clone();
            }
            let config: crate::config::Config = serde_json::from_value(value).unwrap();
            assert!(config.validate().is_err(), "{patch}");
        }
        let unknown = json!({"memory_stall": {"auto_repair": true}});
        let mut value = serde_json::to_value(&base).unwrap();
        value["memory_stall"] = unknown["memory_stall"].clone();
        assert!(serde_json::from_value::<crate::config::Config>(value).is_err());
    }

    #[test]
    fn stalled_verdict_reaches_status_render_and_journal_once() {
        let dir = tempfile::tempdir().unwrap();
        let args_path = dir.path().join("args");
        let script = format!(
            "printf '%s\\n' \"$@\" > '{}'\n{}",
            args_path.display(),
            reply(&report("stalled", "STALLED_NO_REFLECTION"))
        );
        let shared = shared(
            dir.path(),
            &script,
            json!({"memory_stall": {"min_user_utterances": 12, "min_quiet_secs": 3600}}),
        );
        let first = check(&shared);
        assert_eq!(first["state"], "stalled");
        assert_eq!(first["reason"], "STALLED_NO_REFLECTION");
        assert_eq!(first["user_utterances_since_memory"], 55);
        assert!(first.get("future_field_with_text").is_none());
        let args = std::fs::read_to_string(&args_path).unwrap();
        assert!(args.starts_with("memory-stall\n--dir\n"));
        assert!(args.contains("--min-user-utterances\n12\n--min-quiet-secs\n3600\n"));

        let status = shared.status();
        assert_eq!(status["memory_stall"], first);
        let (text, ok) = crate::status::render(&status);
        assert!(text.contains("MEMORY  [WARN] state=stalled reason=STALLED_NO_REFLECTION"));
        assert!(text.contains("utterances_since_memory=55/40"));
        assert_eq!(first["ephemeral_user_utterances_since_memory"], 312);
        assert!(text.contains("ephemeral_utterances_not_counted=312"));
        assert!(!text.contains("PRIVATE_TEXT"));
        assert!(!ok, "a stall makes `kamimusuhi status` exit non-zero");

        // Same verdict again: no new journal line.
        check(&shared);
        let journal = dir.path().join("spool");
        let lines = count_lines(&journal, "memory_stall");
        assert_eq!(lines, 1);
    }

    fn count_lines(root: &Path, needle: &str) -> usize {
        let mut total = 0;
        let mut stack = vec![root.to_path_buf()];
        while let Some(path) = stack.pop() {
            if path.is_dir() {
                for entry in std::fs::read_dir(&path).unwrap() {
                    stack.push(entry.unwrap().path());
                }
            } else if path.to_string_lossy().contains(needle) {
                total += std::fs::read_to_string(&path).unwrap().lines().count();
            }
        }
        total
    }

    #[test]
    fn ok_verdict_does_not_fail_status() {
        let dir = tempfile::tempdir().unwrap();
        let shared = shared(
            dir.path(),
            &reply(&report("ok", "BELOW_THRESHOLD")),
            json!({}),
        );
        check(&shared);
        let (text, ok) = crate::status::render(&shared.status());
        assert!(text.contains("MEMORY  [OK  ] state=ok reason=BELOW_THRESHOLD"));
        // Other sections (no tiers configured here) decide `ok`; MEMORY
        // must not be the one failing it.
        let mut without = shared.status();
        without["memory_stall"] = Value::Null;
        assert_eq!(ok, crate::status::render(&without).1);
    }

    #[test]
    fn failures_publish_stable_codes_and_keep_the_last_verdict() {
        let dir = tempfile::tempdir().unwrap();
        let shared = shared(
            dir.path(),
            &reply(&report("stalled", "STALLED_NO_REFLECTION")),
            json!({}),
        );
        check(&shared);
        let binary = shared
            .config
            .dialogue
            .as_ref()
            .unwrap()
            .runtime_binary
            .clone();
        for (script, reason) in [
            (
                "printf '%s' PRIVATE_STDERR >&2; exit 1",
                "MEMORY_STALL_PROCESS_FAILED",
            ),
            ("printf '%s' not-json", "MEMORY_STALL_INVALID_REPORT"),
            (
                "printf '%s' '{\"state\":\"invented\",\"reason\":\"X\"}'",
                "MEMORY_STALL_INVALID_REPORT",
            ),
        ] {
            std::fs::write(&binary, format!("#!/bin/sh\n{script}\n")).unwrap();
            let failed = check(&shared);
            assert_eq!(failed["state"], "unavailable");
            assert_eq!(failed["reason"], reason);
            assert_eq!(failed["last_verdict"]["state"], "stalled");
            assert!(!failed.to_string().contains("PRIVATE_STDERR"));
        }
        // An unavailable check is shown but is not itself a stall.
        let (text, _) = crate::status::render(&shared.status());
        assert!(text.contains("MEMORY  [----] state=unavailable"));

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
            check(&shared)["reason"],
            "MEMORY_STALL_INDIVIDUAL_UNINITIALIZED"
        );
    }

    #[test]
    fn disabled_or_dialogue_less_nodes_never_spawn_a_check() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("started");
        let script = format!("touch '{}'\nexit 0", marker.display());
        let shared = Arc::new(shared(
            dir.path(),
            &script,
            json!({"memory_stall": {"enabled": false}}),
        ));
        spawn(&shared);
        assert_eq!(shared.status()["memory_stall"]["state"], "disabled");
        assert!(!marker.exists());
    }
}
