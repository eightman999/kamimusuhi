//! W4: the restart vertical slice, run across genuinely separate processes.
//!
//! Every phase here is a child process with its own PID. That is the whole
//! point of the file: two structs in one process would share a heap, and the
//! claim under test is precisely that nothing in that heap was needed. Process
//! B is started with a directory path and four flags — no stdin, no
//! transcript, no serialized context — and has to rebuild the individual from
//! canonical state alone.
//!
//! Covers issue #17 tests A (process boundary), B (Fake A → Fake B), C (typed
//! workspace after restart), D (trace reconstruction) and E (inspect is
//! read-only).

use std::path::Path;
use std::process::{Command, Stdio};

use kamimusuhi_runtime::scenario::{FIRST_INPUT, LIBRARY_CONTENT, RESUME_INPUT};

const BINARY: &str = env!("CARGO_BIN_EXE_kamimusuhi-runtime");

/// Run the runtime as a child process.
///
/// stdin is closed, so a phase cannot be fed a conversation even by accident,
/// and the arguments are asserted to carry no fixture text: whatever process B
/// knows, it read from disk.
fn run(args: &[&str]) -> serde_json::Value {
    for arg in args {
        assert!(
            !arg.contains(FIRST_INPUT) && !arg.contains(RESUME_INPUT),
            "a phase must not be handed conversation text on its command line: {arg:?}"
        );
    }
    let output = Command::new(BINARY)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("the runtime binary should be runnable");
    assert!(
        output.status.success(),
        "{args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("a command reports JSON on stdout")
}

fn run_expecting_failure(args: &[&str]) -> String {
    let output = Command::new(BINARY)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("the runtime binary should be runnable");
    assert!(!output.status.success(), "{args:?} unexpectedly succeeded");
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn as_str(value: &serde_json::Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("{pointer} is missing from {value}"))
        .to_owned()
}

fn as_u64(value: &serde_json::Value, pointer: &str) -> u64 {
    value
        .pointer(pointer)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_else(|| panic!("{pointer} is missing from {value}"))
}

/// Raw row counts, read through an independent connection so the runtime's own
/// view cannot mask what is on disk.
fn row_counts(dir: &Path) -> Vec<(String, i64)> {
    let conn = rusqlite::Connection::open(dir.join("kamimusuhi.sqlite")).unwrap();
    [
        "canonical_commits",
        "continuity_heads",
        "state_records",
        "evidence_records",
        "library_artifacts",
        "library_chunks",
        "resource_calls",
        "audit_events",
    ]
    .into_iter()
    .map(|table| {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        (table.to_owned(), count)
    })
    .collect()
}

fn trace_events(dir: &Path) -> Vec<serde_json::Value> {
    let text = std::fs::read_to_string(dir.join("trace.jsonl")).unwrap();
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// Run the whole scenario once: init, process A, process B.
struct Scenario {
    _dir: tempfile::TempDir,
    dir: std::path::PathBuf,
    init: serde_json::Value,
    first: serde_json::Value,
    resume: serde_json::Value,
}

fn scenario() -> Scenario {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    let text = path.to_str().unwrap().to_owned();

    let init = run(&[
        "init",
        "--dir",
        &text,
        "--resource",
        "fake-a",
        "--seed",
        "1",
    ]);
    // Each phase is a separate process. Distinct seeds because two processes
    // sharing one runtime directory must not mint colliding IDs.
    let first = run(&[
        "demo-continuity",
        "--dir",
        &text,
        "--phase",
        "first",
        "--resource",
        "fake-a",
        "--seed",
        "10",
    ]);
    let resume = run(&[
        "demo-continuity",
        "--dir",
        &text,
        "--phase",
        "resume",
        "--resource",
        "fake-b",
        "--seed",
        "20",
    ]);
    Scenario {
        _dir: dir,
        dir: path,
        init,
        first,
        resume,
    }
}

#[test]
fn restart_across_processes_restores_the_same_individual_without_a_chat_buffer() {
    let s = scenario();

    // Genuinely separate process lifetimes.
    let pid_a = as_u64(&s.first, "/process_id");
    let pid_b = as_u64(&s.resume, "/process_id");
    assert_ne!(pid_a, pid_b, "the two phases must not share a process");
    assert_ne!(pid_a, u64::from(std::process::id()));
    assert_ne!(pid_b, u64::from(std::process::id()));
    assert_ne!(
        as_str(&s.first, "/boot_id"),
        as_str(&s.resume, "/boot_id"),
        "each process is its own boot"
    );

    // The same individual, restored from disk rather than re-created.
    let individual = as_str(&s.init, "/individual_id");
    assert_eq!(as_str(&s.first, "/individual_id"), individual);
    assert_eq!(as_str(&s.resume, "/individual_id"), individual);
    assert_eq!(
        as_str(&s.resume, "/head_before/commit_id"),
        as_str(&s.first, "/head_after/commit_id"),
        "process B must resume the head process A left"
    );
    assert_eq!(as_u64(&s.first, "/head_before/generation"), 0);
    assert_eq!(as_u64(&s.first, "/head_after/generation"), 1);
    assert_eq!(as_u64(&s.resume, "/head_before/generation"), 1);

    // No new identity was minted at any point.
    let after = run(&["inspect", "--dir", s.dir.to_str().unwrap()]);
    assert_eq!(as_str(&after, "/individual_id"), individual);
    assert_eq!(
        as_str(&after, "/root_commit_id"),
        as_str(&s.init, "/root_commit_id")
    );
    assert_eq!(as_u64(&after, "/canonical_commits"), 2, "root + one fact");

    // Process B was handed no previous context.
    assert_eq!(as_str(&s.resume, "/prior_context"), "none");

    // What it does have is durable memory: the preference could only have come
    // from the canonical store, since nothing carried it across the boundary.
    let restored = s
        .resume
        .pointer("/relationship")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(
        as_str(&restored[0], "/payload/preference"),
        "ほうじ茶",
        "the relationship fact must survive the restart"
    );
    assert_eq!(
        as_str(&restored[0], "/state_record_id"),
        as_str(
            s.first.pointer("/relationship").unwrap().get(0).unwrap(),
            "/state_record_id"
        ),
        "the same durable record, not a re-derived one"
    );
    assert_eq!(as_u64(&restored[0], "/independent_evidence_count"), 1);
}

#[test]
fn the_library_survives_the_restart_and_is_not_reimported() {
    let s = scenario();
    assert!(as_u64(&s.first, "/library/chunk_count") >= 3);
    assert_eq!(
        s.first.pointer("/library/imported_now"),
        Some(&serde_json::Value::Bool(true))
    );
    // Process B retrieves the document without importing it: it is on disk.
    assert_eq!(
        s.resume.pointer("/library/imported_now"),
        Some(&serde_json::Value::Bool(false))
    );
    assert_eq!(
        as_str(&s.resume, "/library/artifact_id"),
        as_str(&s.first, "/library/artifact_id")
    );
    assert_eq!(
        as_str(&s.resume, "/library/chunk_id"),
        as_str(&s.first, "/library/chunk_id"),
        "the same chunk is still citable by the same ID"
    );

    let inspected = run(&["inspect", "--dir", s.dir.to_str().unwrap()]);
    assert_eq!(as_u64(&inspected, "/library/artifacts"), 1);
    assert_eq!(
        as_u64(&inspected, "/library/chunks"),
        as_u64(&s.first, "/library/chunk_count")
    );
}

#[test]
fn replacing_fake_a_with_fake_b_changes_the_material_and_not_the_individual() {
    let s = scenario();

    // The material and its attribution change.
    assert_eq!(as_str(&s.first, "/resource/implementation"), "fake-a");
    assert_eq!(as_str(&s.resume, "/resource/implementation"), "fake-b");
    assert_eq!(as_str(&s.first, "/resource/answer"), "result-a");
    assert_eq!(as_str(&s.resume, "/resource/answer"), "result-b");
    assert_ne!(
        as_str(&s.first, "/resource/resource_id"),
        as_str(&s.resume, "/resource/resource_id"),
        "the call must be attributed to the resource that actually answered"
    );
    // The slot — the cognitive role — is the thing that stayed the same.
    assert_eq!(as_str(&s.first, "/resource/slot"), "general");
    assert_eq!(as_str(&s.resume, "/resource/slot"), "general");

    // The individual does not.
    assert_eq!(
        as_str(&s.first, "/individual_id"),
        as_str(&s.resume, "/individual_id")
    );
    // Replacement plus a read-only invocation moved nothing canonical: the
    // resume phase opens on generation 1 and leaves on generation 1.
    assert_eq!(
        s.resume.pointer("/head_before"),
        s.resume.pointer("/head_after"),
        "read-only resource use must not advance the head"
    );

    let inspected = run(&["inspect", "--dir", s.dir.to_str().unwrap()]);
    assert_eq!(as_u64(&inspected, "/resource_calls/total"), 2);
    assert_eq!(as_u64(&inspected, "/resource_calls/ok"), 2);
    let used = inspected
        .pointer("/resource_calls/resources_used")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(
        used.len(),
        2,
        "both resources are attributable after the fact"
    );
    // Durable memory is untouched by any of it.
    assert_eq!(as_u64(&inspected, "/relationship/active"), 1);
    assert_eq!(as_u64(&inspected, "/relationship/total"), 1);
}

#[test]
fn the_rebuilt_workspace_keeps_all_four_domains_distinguishable_by_type() {
    let s = scenario();
    let items = s
        .resume
        .pointer("/workspace/items")
        .unwrap()
        .as_array()
        .unwrap();

    // Positions are dense and ordered; the assembly is not a bag.
    for (index, item) in items.iter().enumerate() {
        assert_eq!(as_u64(item, "/position"), index as u64);
    }

    let find = |domain: &str| {
        items
            .iter()
            .find(|item| as_str(item, "/domain") == domain)
            .unwrap_or_else(|| panic!("{domain} is missing from the rebuilt workspace"))
    };

    // Each domain is identified by its typed source ref, never by reading the
    // item's content.
    let input = find("CURRENT_INPUT");
    assert_eq!(as_str(input, "/source_ref/source"), "input");
    assert_eq!(
        as_str(input, "/source_ref/evidence_id"),
        as_str(&s.resume, "/current_input_evidence_id"),
        "current input points at this turn's evidence, not at memory"
    );
    assert_eq!(as_str(input, "/authority"), "direct_input");

    let memory = find("RELATIONSHIP_MEMORY");
    assert_eq!(as_str(memory, "/source_ref/source"), "memory");
    assert_eq!(as_str(memory, "/source_ref/domain"), "relationship");
    assert_eq!(as_str(memory, "/authority"), "canonical_state");
    assert!(
        !memory
            .pointer("/source_ref/evidence_refs")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty(),
        "durable memory keeps its provenance into the workspace"
    );

    let library = find("LIBRARY_EVIDENCE");
    assert_eq!(as_str(library, "/source_ref/source"), "library");
    assert_eq!(
        as_str(library, "/source_ref/artifact_id"),
        as_str(&s.resume, "/library/artifact_id")
    );
    assert_eq!(as_str(library, "/authority"), "external_material");

    let resource = find("EXTERNAL_RESOURCE_RESULT");
    assert_eq!(as_str(resource, "/source_ref/source"), "resource");
    assert_eq!(
        as_str(resource, "/source_ref/resource_id"),
        as_str(&s.resume, "/resource/resource_id")
    );
    assert_eq!(
        as_str(resource, "/source_ref/resource_call_id"),
        as_str(&s.resume, "/resource/resource_call_id")
    );
    assert_eq!(as_str(resource, "/authority"), "external_material");

    // Current input is not confusable with the other three.
    assert_ne!(as_str(input, "/domain"), as_str(memory, "/domain"));
    assert_ne!(as_str(input, "/source_ref/source"), "memory");
    assert_ne!(as_str(input, "/source_ref/source"), "library");
    assert_ne!(as_str(input, "/source_ref/source"), "resource");

    // The Persona Core saw the same typed attribution, counted by domain.
    let response = as_str(&s.resume, "/persona_response");
    for domain in [
        "CURRENT_INPUT=1",
        "RELATIONSHIP_MEMORY=1",
        "LIBRARY_EVIDENCE=1",
        "EXTERNAL_RESOURCE_RESULT=1",
    ] {
        assert!(
            response.contains(domain),
            "persona lost {domain}: {response}"
        );
    }
}

#[test]
fn the_trace_reconstructs_the_scenario_from_correlation_ids_alone() {
    let s = scenario();
    let events = trace_events(&s.dir);

    let kind = |e: &serde_json::Value| as_str(e, "/event_kind");
    let of_kind = |k: &str| -> Vec<serde_json::Value> {
        events.iter().filter(|e| kind(e) == k).cloned().collect()
    };
    let one = |k: &str| -> serde_json::Value {
        let found = of_kind(k);
        assert_eq!(found.len(), 1, "expected exactly one {k} event");
        found[0].clone()
    };

    // Three process lifetimes wrote to one file: init, first, resume. The
    // boundary is visible as distinct boot IDs, not as a message.
    let boots = of_kind("runtime.boot");
    assert_eq!(boots.len(), 3);
    let boot_ids: std::collections::BTreeSet<String> = boots
        .iter()
        .map(|e| as_str(e, "/correlation/boot_id"))
        .collect();
    assert_eq!(boot_ids.len(), 3, "each process is its own boot");
    let pids: std::collections::BTreeSet<u64> = boots
        .iter()
        .map(|e| as_u64(e, "/correlation/process_id"))
        .collect();
    assert_eq!(pids.len(), 3, "each boot is its own process");

    // Phase A's chain, followed by ID rather than by text.
    let evidence = of_kind("evidence.recorded");
    assert_eq!(evidence.len(), 2, "one input per phase");
    let first_evidence = as_str(&evidence[0], "/correlation/evidence_id");
    assert_eq!(
        first_evidence,
        as_str(&s.first, "/current_input_evidence_id")
    );

    let proposed = one("mutation.proposed");
    assert_eq!(
        as_str(&proposed, "/correlation/evidence_id"),
        first_evidence,
        "the proposal cites the evidence recorded for that input"
    );
    let proposal_id = as_str(&proposed, "/correlation/proposal_id");
    assert_eq!(proposal_id, as_str(&s.first, "/proposal/proposal_id"));

    let decided = one("mutation.decided");
    assert_eq!(as_str(&decided, "/correlation/proposal_id"), proposal_id);
    assert_eq!(as_str(&decided, "/detail/reason_code"), "ACCEPTED");

    let receipt = one("continuity.receipt_observed");
    assert_eq!(as_str(&receipt, "/correlation/proposal_id"), proposal_id);
    assert_eq!(
        as_str(&receipt, "/correlation/commit_id"),
        as_str(&s.first, "/proposal/commit_id")
    );
    assert_eq!(
        as_str(&receipt, "/correlation/receipt_id"),
        as_str(&s.first, "/proposal/receipt_id")
    );

    // The activation happened in phase A's boot; the retrievals that follow
    // happened in a different one. That is the restart, read off the trace.
    let activation_boot = as_str(&receipt, "/correlation/boot_id");
    let retrievals = of_kind("library.retrieved");
    assert_eq!(retrievals.len(), 2);
    let resume_retrieval = &retrievals[1];
    assert_ne!(
        as_str(resume_retrieval, "/correlation/boot_id"),
        activation_boot,
        "the second retrieval must come from a later process"
    );
    assert_eq!(
        as_str(resume_retrieval, "/correlation/artifact_id"),
        as_str(&s.resume, "/library/artifact_id")
    );

    // Resource B's call, attributed to the resource that answered.
    let completions = of_kind("resource.completed");
    assert_eq!(completions.len(), 2);
    assert_eq!(
        as_str(&completions[0], "/correlation/resource_id"),
        as_str(&s.first, "/resource/resource_id")
    );
    assert_eq!(
        as_str(&completions[1], "/correlation/resource_id"),
        as_str(&s.resume, "/resource/resource_id")
    );
    assert_eq!(
        as_str(&completions[1], "/correlation/resource_call_id"),
        as_str(&s.resume, "/resource/resource_call_id")
    );
    assert_ne!(
        as_str(&completions[0], "/detail/result_digest"),
        as_str(&completions[1], "/detail/result_digest"),
        "different resources produced different material"
    );

    // Workspace assembly and the persona turn close the chain.
    let assembled = of_kind("workspace.assembled");
    assert_eq!(assembled.len(), 2);
    assert_eq!(
        as_str(&assembled[1], "/correlation/workspace_digest"),
        as_str(&s.resume, "/workspace/digest")
    );
    let persona = of_kind("persona.completed");
    assert_eq!(persona.len(), 2);
    assert_eq!(
        as_str(&persona[1], "/correlation/workspace_digest"),
        as_str(&s.resume, "/workspace/digest"),
        "the persona thought with the workspace that was assembled"
    );

    // Everything in one phase shares that phase's session and turn.
    let resume_session = as_str(&s.resume, "/session_id");
    for event in events
        .iter()
        .filter(|e| e.pointer("/correlation/session_id").is_some())
        .filter(|e| as_str(e, "/correlation/session_id") == resume_session)
    {
        assert_eq!(
            as_str(event, "/correlation/turn_id"),
            as_str(&s.resume, "/turn_id")
        );
    }
}

#[test]
fn the_trace_carries_ids_and_digests_rather_than_what_was_said() {
    let s = scenario();
    let raw = std::fs::read_to_string(s.dir.join("trace.jsonl")).unwrap();

    // The utterances live in the evidence store. Copying them into a file with
    // weaker guarantees would duplicate private content for no gain.
    assert!(!raw.contains(FIRST_INPUT));
    assert!(!raw.contains(RESUME_INPUT));
    assert!(
        !raw.contains("ほうじ茶"),
        "the activated preference must not be transcribed"
    );
    for line in LIBRARY_CONTENT.lines().filter(|l| l.len() > 8) {
        assert!(
            !raw.contains(line),
            "library text must not be transcribed: {line}"
        );
    }

    // What it does carry is enough to find any of it.
    let events = trace_events(&s.dir);
    let evidence = events
        .iter()
        .find(|e| as_str(e, "/event_kind") == "evidence.recorded")
        .unwrap();
    assert!(as_str(evidence, "/detail/content_digest").starts_with("sha256:"));
    assert_eq!(as_str(evidence, "/detail/kind"), "user_utterance");
}

#[test]
fn inspect_changes_nothing() {
    let s = scenario();
    let dir = s.dir.to_str().unwrap();

    let before_rows = row_counts(&s.dir);
    let before = run(&["inspect", "--dir", dir]);
    let middle_rows = row_counts(&s.dir);
    let after = run(&["inspect", "--dir", dir]);
    let after_rows = row_counts(&s.dir);

    // Not one canonical row moves, including the writer epoch: inspection
    // never claims writer authority.
    assert_eq!(before_rows, middle_rows);
    assert_eq!(before_rows, after_rows);
    assert_eq!(before, after);
    assert_eq!(
        as_str(&before, "/head/commit_id"),
        as_str(&s.resume, "/head_after/commit_id")
    );
    assert_eq!(
        as_u64(&before, "/head/writer_epoch"),
        as_u64(&s.resume, "/writer_epoch"),
        "inspect must not take a new writer epoch"
    );

    // The only record of an inspection is the non-canonical trace.
    let inspections = trace_events(&s.dir)
        .into_iter()
        .filter(|e| as_str(e, "/event_kind") == "inspect.invoked")
        .count();
    assert!(inspections >= 2, "inspections are observable in the trace");
}

#[test]
fn a_runtime_that_cannot_be_restored_refuses_rather_than_minting_a_new_individual() {
    let dir = tempfile::tempdir().unwrap();
    let text = dir.path().to_str().unwrap().to_owned();

    // Nothing initialized yet: resuming must not create anybody.
    let stderr = run_expecting_failure(&[
        "demo-continuity",
        "--dir",
        &text,
        "--phase",
        "resume",
        "--seed",
        "1",
    ]);
    assert!(stderr.contains("not initialized"), "{stderr}");
    assert!(!dir.path().join("kamimusuhi.sqlite").exists());

    let init = run(&["init", "--dir", &text, "--seed", "1"]);
    let individual = as_str(&init, "/individual_id");

    // Re-running init does not fork the individual.
    let stderr = run_expecting_failure(&["init", "--dir", &text, "--seed", "2"]);
    assert!(stderr.contains("already initialized"), "{stderr}");
    assert_eq!(
        as_str(&run(&["inspect", "--dir", &text]), "/individual_id"),
        individual
    );

    // A database whose head is gone is unrestorable, and staying broken is the
    // correct outcome: becoming a different individual would be worse.
    let conn = rusqlite::Connection::open(dir.path().join("kamimusuhi.sqlite")).unwrap();
    conn.execute_batch("PRAGMA foreign_keys = OFF; DELETE FROM continuity_heads;")
        .unwrap();
    drop(conn);

    let stderr = run_expecting_failure(&["inspect", "--dir", &text]);
    assert!(stderr.contains("could not be restored"), "{stderr}");
    let conn = rusqlite::Connection::open(dir.path().join("kamimusuhi.sqlite")).unwrap();
    let individuals: i64 = conn
        .query_row("SELECT COUNT(*) FROM individuals", [], |r| r.get(0))
        .unwrap();
    assert_eq!(individuals, 1, "no replacement individual was minted");
}

#[test]
fn the_scenario_is_reproducible_from_a_clean_directory() {
    // Same seeds, same fixture, same IDs: the demo is a fixture, not a
    // recording of one lucky run.
    let first = scenario();
    let second = scenario();
    for pointer in [
        "/individual_id",
        "/head_after/commit_id",
        "/current_input_evidence_id",
        "/proposal/proposal_id",
        "/library/artifact_id",
        "/resource/resource_call_id",
        "/workspace/digest",
        "/persona_response",
    ] {
        assert_eq!(
            as_str(&first.first, pointer),
            as_str(&second.first, pointer),
            "{pointer} differed between two clean runs"
        );
    }
    assert_eq!(
        as_str(&first.resume, "/workspace/digest"),
        as_str(&second.resume, "/workspace/digest")
    );
    // The PIDs are the one thing that must differ.
    assert_ne!(
        as_u64(&first.first, "/process_id"),
        as_u64(&second.first, "/process_id")
    );
}
