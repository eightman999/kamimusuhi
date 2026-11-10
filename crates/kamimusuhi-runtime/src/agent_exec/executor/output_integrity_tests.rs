// Included by executor::tests so these regressions reuse its configuration
// and request helpers without widening their visibility.
#[cfg(unix)]
mod output_integrity_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::Ordering;

    const ANSWER: &str = "partial answer";
    const OPENCODE_ANSWER: &str =
        r#"{"type":"text","sessionID":"s1","part":{"text":"partial answer"}}"#;
    const COMMAND_CODE_ANSWER: &str =
        r#"{"type":"result","subtype":"success","sessionId":"s1","finalText":"partial answer"}"#;

    fn quoted(value: &str) -> String {
        format!("'{}'", value.replace('\'', "'\\''"))
    }

    fn emit(line: &str) -> String {
        format!("printf '%s\\n' {}\n", quoted(line))
    }

    fn oversized_line(prefix: &str, suffix: &str) -> String {
        let block = "x".repeat(4096);
        let count = process::STDOUT_LINE_CAP / block.len() + 1;
        format!(
            "printf '%s' {}\ni=0\nwhile [ \"$i\" -lt {count} ]; do\n\
             printf '%s' {}\ni=$((i+1))\ndone\nprintf '%s\\n' {}\n",
            quoted(prefix),
            quoted(&block),
            quoted(suffix),
        )
    }

    fn stderr_flood() -> String {
        let block = "e".repeat(4096);
        let count = process::STDERR_CAP / block.len() + 1;
        format!(
            "i=0\nwhile [ \"$i\" -lt {count} ]; do\n\
             printf '%s' {} >&2\ni=$((i+1))\ndone\n",
            quoted(&block),
        )
    }

    fn fixture(output: &str, body: &str) -> (tempfile::TempDir, CliExecutor, TaskRequest) {
        let dir = tempfile::tempdir().expect("fixture directory");
        let script = dir.path().join("fake-agent");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = \"--version\" ]; then printf 'fixture 1\\n'; exit 0; fi\n\
                 {body}\n"
            ),
        )
        .expect("fixture script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("executable fixture");
        let ex = executor(serde_json::json!({
            "name": "output-integrity-fixture",
            "adapter": "generic",
            "command": script,
            "run": {"args": ["{prompt}"], "output": output},
            "discover": {"args": ["--list"], "format": "lines"},
            "permission_args": {"read_only": []},
            "default_billing": "local",
            "timeout_secs": 10,
            "discover_timeout_secs": 10
        }));
        let mut req = request(None, TaskPermissions::ReadOnly);
        req.workspace = dir.path().to_path_buf();
        (dir, ex, req)
    }

    fn assert_incomplete_answer(outcome: &TaskOutcome, output: &str) {
        assert_eq!(outcome.exit_code, Some(0), "{outcome:?}");
        assert_eq!(outcome.summary, ANSWER, "retain the available evidence");
        if output != "text" {
            assert_eq!(outcome.session_id.as_deref(), Some("s1"));
        }
        assert_eq!(
            outcome.end,
            RunEnd::Failed,
            "{output}: missing stdout must not certify completion: {outcome:?}"
        );
        let error = outcome.error.as_deref().expect("incomplete stdout reason");
        assert!(
            error.contains("stdout") && error.contains("truncat"),
            "{error}"
        );
    }

    fn check_oversized_line(output: &str, first: &str, prefix: &str, suffix: &str) {
        let body = format!("{}{}exit 0\n", emit(first), oversized_line(prefix, suffix));
        let (_dir, ex, req) = fixture(output, &body);
        let outcome = ex.run(&req, &AtomicBool::new(false), &mut |_| {});
        assert_incomplete_answer(&outcome, output);
    }

    #[test]
    fn text_answer_with_a_discarded_line_is_incomplete() {
        check_oversized_line("text", ANSWER, "unread remainder: ", "");
    }

    #[test]
    fn opencode_answer_cannot_hide_a_discarded_error_event() {
        check_oversized_line(
            "opencode_json",
            OPENCODE_ANSWER,
            r#"{"type":"error","error":{"message":""#,
            r#""}}"#,
        );
    }

    #[test]
    fn command_code_result_cannot_hide_a_discarded_error_event() {
        // A lone oversized final result would already fail with no answer.
        // This fixture checks loss after a result that the parser accepted.
        check_oversized_line(
            "command_code_ndjson",
            COMMAND_CODE_ANSWER,
            r#"{"type":"event","event":{"type":"run_error","error":{"message":""#,
            r#""}}}"#,
        );
    }

    #[test]
    fn aggregate_stdout_loss_cannot_hide_a_small_final_error() {
        let padding = serde_json::json!({
            "type": "fixture_padding",
            "value": "x".repeat(4096)
        })
        .to_string();
        assert!(padding.len() < process::STDOUT_LINE_CAP);
        let count = process::STDOUT_CAP / (padding.len() + 1) + 2;
        let body = format!(
            "{}i=0\nwhile [ \"$i\" -lt {count} ]; do\n{}i=$((i+1))\ndone\n{}exit 0\n",
            emit(OPENCODE_ANSWER),
            emit(&padding),
            emit(r#"{"type":"error","error":{"message":"late failure"}}"#),
        );
        let (_dir, ex, req) = fixture("opencode_json", &body);
        let outcome = ex.run(&req, &AtomicBool::new(false), &mut |_| {});
        assert_incomplete_answer(&outcome, "opencode_json");
    }

    #[test]
    fn complete_answers_succeed_with_and_without_stderr_truncation() {
        for (output, first) in [
            ("text", ANSWER),
            ("opencode_json", OPENCODE_ANSWER),
            ("command_code_ndjson", COMMAND_CODE_ANSWER),
        ] {
            for noisy_stderr in [false, true] {
                let mut body = emit(first);
                if output == "opencode_json" {
                    body.push_str(&emit(r#"{"type":"step_finish","part":{"reason":"stop"}}"#));
                }
                if noisy_stderr {
                    body.push_str(&stderr_flood());
                }
                body.push_str("exit 0\n");
                let (_dir, ex, req) = fixture(output, &body);
                let outcome = ex.run(&req, &AtomicBool::new(false), &mut |_| {});
                assert_eq!(
                    outcome.end,
                    RunEnd::Succeeded,
                    "{output}, noisy_stderr={noisy_stderr}: {outcome:?}"
                );
                assert_eq!(outcome.summary, ANSWER);
                assert_eq!(outcome.exit_code, Some(0));
                assert_eq!(outcome.error, None);
            }
        }
    }

    #[test]
    fn discovery_rejects_a_partial_model_listing() {
        let body = format!(
            "{}{}exit 0\n",
            emit("m-1 first model"),
            oversized_line("m-2 ", ""),
        );
        let (_dir, ex, _req) = fixture("text", &body);
        let entry = ex.discover(42);
        assert_eq!(entry.checked_at, 42);
        assert_eq!(entry.harness_version.as_deref(), Some("fixture 1"));
        assert!(
            entry.error.is_some(),
            "a retained model must not make an incomplete refresh successful: {entry:?}"
        );
        let error = entry.error.as_deref().expect("incomplete discovery reason");
        assert!(
            error.contains("stdout") && error.contains("truncat"),
            "{error}"
        );
        assert_eq!(entry.discovered_at, 0, "no new successful discovery");
        assert!(
            entry.models.is_empty(),
            "leave last-good retention to the caller"
        );
    }

    #[test]
    fn complete_discovery_survives_stderr_truncation() {
        let body = format!(
            "{}{}{}exit 0\n",
            emit("m-1 first model"),
            emit("m-2 second model"),
            stderr_flood(),
        );
        let (_dir, ex, _req) = fixture("text", &body);
        let entry = ex.discover(42);
        assert_eq!(entry.error, None, "{entry:?}");
        assert_eq!(entry.checked_at, 42);
        assert_eq!(entry.discovered_at, 42);
        assert_eq!(entry.harness_version.as_deref(), Some("fixture 1"));
        assert_eq!(
            entry
                .models
                .iter()
                .map(|m| m.id.model.as_str())
                .collect::<Vec<_>>(),
            ["m-1", "m-2"]
        );
    }

    #[test]
    fn cancellation_after_stdout_loss_keeps_its_status() {
        // The session event is delivered only after the oversized line was
        // consumed, so cancellation does not race the start of that output.
        let body = format!(
            "{}{}sleep 30\n",
            oversized_line("", ""),
            emit(OPENCODE_ANSWER),
        );
        let (_dir, ex, req) = fixture("opencode_json", &body);
        let cancel = AtomicBool::new(false);
        let mut sessions = 0;
        let outcome = ex.run(&req, &cancel, &mut |event| {
            if let TaskEvent::Session { id } = event {
                assert_eq!(id, "s1");
                sessions += 1;
                cancel.store(true, Ordering::Relaxed);
            }
        });
        assert_eq!(sessions, 1);
        assert_eq!(outcome.summary, ANSWER);
        assert_eq!(outcome.session_id.as_deref(), Some("s1"));
        assert_eq!(outcome.end, RunEnd::Cancelled, "{outcome:?}");
        assert_eq!(outcome.error.as_deref(), Some("cancelled"));
    }

    #[test]
    fn timeout_after_stdout_loss_keeps_its_status() {
        let body = format!(
            "{}{}sleep 30\n",
            emit(OPENCODE_ANSWER),
            oversized_line("", ""),
        );
        let (_dir, mut ex, req) = fixture("opencode_json", &body);
        ex.spec.timeout_secs = 1;
        let outcome = ex.run(&req, &AtomicBool::new(false), &mut |_| {});
        assert_eq!(outcome.summary, ANSWER);
        assert_eq!(outcome.end, RunEnd::TimedOut, "{outcome:?}");
        assert!(
            outcome
                .error
                .as_deref()
                .is_some_and(|e| e.contains("timed out")),
            "{outcome:?}"
        );
    }
}
