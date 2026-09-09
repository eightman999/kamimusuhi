//! W7: the Persona Core boundary, across real process boundaries.
//!
//! Two claims, and they are about structure rather than about answers.
//!
//! The user-facing expression is always the output of a Persona invocation.
//! Whatever a cognitive resource produced arrives as *input* to that turn and
//! never reaches the user directly — the tests here assert that with a Persona
//! backend whose reply is a fixed string the resource never returns, so a
//! passthrough would be immediately visible.
//!
//! And replacing the Persona backend — fixture for model, or one model for
//! another — leaves `IndividualId`, the root commit, the continuity head and
//! durable memory exactly where they were.
//!
//! The backend here is a scripted local OpenAI-compatible endpoint on a real
//! socket, not an actual model server. These tests exercise wire/process
//! boundaries; they establish neither model competence nor semantic safety.

use std::path::Path;
use std::process::{Command, Stdio};

use kamimusuhi_core::ids::{PersonaBackendId, ResourceId};
use kamimusuhi_core::routing::{
    CostClass, HealthState, LatencyClass, LocalityClass, Modality, QualityTier,
    ResourceCapabilities,
};
use kamimusuhi_runtime::config::{
    GENERAL_SLOT, PersonaBackendKind, PersonaProviderConfig, PersonaSetting,
};
use kamimusuhi_runtime::{ProviderConfig, ResourceImplementation, RuntimeConfig};
use kamimusuhi_testkit::http_fixture::{FixtureResponse, FixtureServer, refused_base_url};

const BINARY: &str = env!("CARGO_BIN_EXE_kamimusuhi-runtime");
const PERSONA_A: PersonaBackendId = PersonaBackendId::from_u128(0x0FE5);
const PERSONA_B: PersonaBackendId = PersonaBackendId::from_u128(0x0FE6);
const RESOURCE_A: ResourceId = ResourceId::from_u128(0x0B01);
const RESOURCE_B: ResourceId = ResourceId::from_u128(0x0B02);

/// What the Persona endpoint says. Deliberately something no cognitive
/// resource in these tests ever returns, so a passthrough cannot hide.
const PERSONA_SAYS: &str = "覚えています。ほうじ茶でしたね。";
/// What the delegated resource returns. If this ever reaches the user, the
/// Persona boundary has been bypassed.
const RESOURCE_SAYS: &str = "DELEGATED-MATERIAL-NOT-A-RESPONSE";

fn run(args: &[&str]) -> Result<serde_json::Value, String> {
    let output = Command::new(BINARY)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("the runtime binary should be runnable");
    if output.status.success() {
        Ok(serde_json::from_slice(&output.stdout).expect("JSON on stdout"))
    } else {
        assert!(
            output.status.code().is_some(),
            "the runtime was killed rather than failing cleanly: {:?}",
            output.status
        );
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}

fn ok(args: &[&str]) -> serde_json::Value {
    run(args).unwrap_or_else(|stderr| panic!("{args:?} failed: {stderr}"))
}

fn as_str(value: &serde_json::Value, pointer: &str) -> String {
    value
        .pointer(pointer)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_else(|| panic!("{pointer} is missing from {value}"))
        .to_owned()
}

fn trace_events(dir: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(dir.join("trace.jsonl"))
        .unwrap()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// Identity and lineage, read straight from SQLite.
fn identity(dir: &Path) -> (String, String, String, i64, Vec<String>) {
    let conn = rusqlite::Connection::open(dir.join("kamimusuhi.sqlite")).unwrap();
    let (individual, root): (String, String) = conn
        .query_row(
            "SELECT individual_id, root_commit_id FROM individuals",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let (head, generation): (String, i64) = conn
        .query_row(
            "SELECT commit_id, generation FROM continuity_heads",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let mut stmt = conn
        .prepare("SELECT payload_json FROM state_records ORDER BY state_record_id")
        .unwrap();
    let memory = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    (individual, root, head, generation, memory)
}

fn persona_provider(base_url: String, backend_id: PersonaBackendId) -> PersonaProviderConfig {
    PersonaProviderConfig {
        backend_id,
        locality: LocalityClass::External,
        base_url,
        model: "fixture-persona".to_owned(),
        auth_env: None,
        timeout_ms: 2_000,
        tls_root_ca_path: None,
        system_instruction: None,
    }
}

fn resource_provider(base_url: String, resource_id: ResourceId) -> ProviderConfig {
    ProviderConfig {
        base_url,
        model: "fixture-resource".to_owned(),
        auth_env: None,
        timeout_ms: 2_000,
        max_attempts: 1,
        retry_backoff_ms: 1,
        resource_id,
        tls_root_ca_path: None,
        capabilities: ResourceCapabilities {
            locality: LocalityClass::External,
            modalities: [Modality::Text].into_iter().collect(),
            context_capacity: 8_192,
            latency: LatencyClass::Fast,
            cost: CostClass::Low,
            quality: QualityTier::Standard,
            health: HealthState::Healthy,
        },
    }
}

/// Point the runtime at a Persona endpoint and, optionally, a resource one.
fn configure(dir: &Path, persona: Option<PersonaProviderConfig>, resource: Option<ProviderConfig>) {
    let path = dir.join(RuntimeConfig::FILE_NAME);
    let mut config = RuntimeConfig::load(&path).expect("an initialized runtime");
    if let Some(persona) = persona {
        config.persona = PersonaSetting {
            backend: PersonaBackendKind::OpenaiCompatible,
            provider: Some(persona),
        };
    }
    if let Some(resource) = resource {
        config.set_implementation(GENERAL_SLOT, ResourceImplementation::OpenaiCompatible);
        config.set_provider(GENERAL_SLOT, resource);
    }
    config.save(&path).expect("config is writable");
}

struct Prepared {
    _dir: tempfile::TempDir,
    dir: std::path::PathBuf,
    first: serde_json::Value,
}

fn prepared() -> Prepared {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();
    let text = path.to_str().unwrap().to_owned();
    ok(&[
        "init",
        "--dir",
        &text,
        "--resource",
        "fake-a",
        "--seed",
        "1",
    ]);
    let first = ok(&[
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
    Prepared {
        _dir: dir,
        dir: path,
        first,
    }
}

fn resume(dir: &str, seed: &str) -> Result<serde_json::Value, String> {
    run(&[
        "demo-continuity",
        "--dir",
        dir,
        "--phase",
        "resume",
        "--id-seed",
        seed,
        "--clock",
        "system",
    ])
}

#[test]
fn a_real_persona_endpoint_produces_the_user_facing_expression() {
    let prepared = prepared();
    let dir = prepared.dir.to_str().unwrap().to_owned();
    let persona_endpoint = FixtureServer::always(FixtureResponse::ok(PERSONA_SAYS)).unwrap();
    configure(
        &prepared.dir,
        Some(persona_provider(persona_endpoint.base_url(), PERSONA_A)),
        None,
    );

    let report = resume(&dir, "50").expect("one turn against a real endpoint");

    // The expression came from the model, over a real socket.
    assert_eq!(as_str(&report, "/persona_response"), PERSONA_SAYS);
    assert_eq!(persona_endpoint.request_count(), 1);
    assert_eq!(
        as_str(&report, "/persona_backend/kind"),
        "openai-compatible"
    );
    assert_eq!(
        as_str(&report, "/persona_backend/backend_id"),
        PERSONA_A.to_string()
    );

    // The request it received was the sectioned envelope, with each section
    // labelled — not an unmarked blob.
    let sent = &persona_endpoint.requests()[0].body;
    for label in [
        "[CURRENT_INPUT]",
        "[RELATIONSHIP_MEMORY]",
        "[LIBRARY_EVIDENCE]",
    ] {
        assert!(sent.contains(label), "{label} missing from the prompt");
    }
    // And the system message told the model which sections are borrowed.
    assert!(sent.contains("not your own"));
}

#[test]
fn delegated_material_reaches_the_persona_and_never_the_user() {
    let prepared = prepared();
    let dir = prepared.dir.to_str().unwrap().to_owned();

    // Two distinct endpoints: one is the Persona, one is a delegated resource.
    let persona_endpoint = FixtureServer::always(FixtureResponse::ok(PERSONA_SAYS)).unwrap();
    let resource_endpoint = FixtureServer::always(FixtureResponse::ok(RESOURCE_SAYS)).unwrap();
    configure(
        &prepared.dir,
        Some(persona_provider(persona_endpoint.base_url(), PERSONA_A)),
        Some(resource_provider(resource_endpoint.base_url(), RESOURCE_A)),
    );

    let report = resume(&dir, "51").expect("the full delegation path runs");

    // The whole path ran: router picked the resource, the resource answered,
    // and the Persona was invoked afterwards.
    assert_eq!(resource_endpoint.request_count(), 1);
    assert_eq!(persona_endpoint.request_count(), 1);
    assert_eq!(as_str(&report, "/resource/answer"), RESOURCE_SAYS);
    assert_eq!(
        as_str(&report, "/resource/resource_id"),
        RESOURCE_A.to_string()
    );

    // The resource's text is material. It reached the Persona as input...
    assert!(
        persona_endpoint.requests()[0].body.contains(RESOURCE_SAYS),
        "the delegated material must reach the Persona as input"
    );
    // ...and it is not what the user was told.
    assert_eq!(as_str(&report, "/persona_response"), PERSONA_SAYS);
    assert_ne!(
        as_str(&report, "/persona_response"),
        RESOURCE_SAYS,
        "external material must never be returned as the expression"
    );
    // It arrived under its own section heading, not spliced into the input.
    let body = &persona_endpoint.requests()[0].body;
    let section_at = body
        .find("EXTERNAL_RESOURCE_RESULT")
        .expect("a labelled section");
    let material_at = body.find(RESOURCE_SAYS).expect("the material");
    assert!(section_at < material_at);

    // The trace tells the two apart, within one turn.
    let turn = as_str(&report, "/turn_id");
    let events: Vec<_> = trace_events(&prepared.dir)
        .into_iter()
        .filter(|e| {
            e.pointer("/correlation/turn_id")
                .and_then(serde_json::Value::as_str)
                == Some(turn.as_str())
        })
        .collect();
    let kind_of = |k: &str| -> Vec<serde_json::Value> {
        events
            .iter()
            .filter(|e| as_str(e, "/event_kind") == k)
            .cloned()
            .collect()
    };

    // The resource call is attributed to a ResourceId and carries no persona
    // backend; the persona events are the other way round.
    let resource_done = kind_of("resource.completed");
    assert_eq!(resource_done.len(), 1);
    assert_eq!(
        as_str(&resource_done[0], "/correlation/resource_id"),
        RESOURCE_A.to_string()
    );
    assert!(
        resource_done[0]
            .pointer("/correlation/persona_backend_id")
            .is_none(),
        "a resource call is not a persona invocation"
    );

    let invoked = kind_of("persona.invoked");
    assert_eq!(invoked.len(), 1);
    assert_eq!(
        as_str(&invoked[0], "/correlation/persona_backend_id"),
        PERSONA_A.to_string()
    );
    assert!(invoked[0].pointer("/correlation/resource_id").is_none());
    // The envelope's shape is recorded: what the Persona was given, by section.
    assert_eq!(
        invoked[0]
            .pointer("/detail/sections/external_results")
            .and_then(serde_json::Value::as_u64),
        Some(1)
    );

    let final_expression = kind_of("persona.final_expression");
    assert_eq!(final_expression.len(), 1);
    assert_eq!(
        as_str(&final_expression[0], "/correlation/persona_backend_id"),
        PERSONA_A.to_string()
    );
    // The emitted expression is the one the Persona produced, matched by
    // digest rather than by copying it into the trace.
    assert_eq!(
        as_str(&final_expression[0], "/detail/expression_digest"),
        as_str(&report, "/final_expression_digest")
    );

    // Ordering: material first, expression last.
    let position = |kind: &str| {
        events
            .iter()
            .position(|e| as_str(e, "/event_kind") == kind)
            .unwrap_or_else(|| panic!("{kind} missing"))
    };
    assert!(position("resource.completed") < position("persona.invoked"));
    assert!(position("persona.invoked") < position("persona.completed"));
    assert!(position("persona.completed") < position("persona.final_expression"));
}

#[test]
fn the_trace_records_no_prompt_no_expression_and_no_material() {
    let prepared = prepared();
    let dir = prepared.dir.to_str().unwrap().to_owned();
    let persona_endpoint = FixtureServer::always(FixtureResponse::ok(PERSONA_SAYS)).unwrap();
    let resource_endpoint = FixtureServer::always(FixtureResponse::ok(RESOURCE_SAYS)).unwrap();
    configure(
        &prepared.dir,
        Some(persona_provider(persona_endpoint.base_url(), PERSONA_A)),
        Some(resource_provider(resource_endpoint.base_url(), RESOURCE_A)),
    );
    resume(&dir, "52").unwrap();

    let raw = std::fs::read_to_string(prepared.dir.join("trace.jsonl")).unwrap();
    for forbidden in [
        PERSONA_SAYS,
        RESOURCE_SAYS,
        "ほうじ茶",
        "Bearer",
        "not your own", // the system instruction is prompt text too
        "[CURRENT_INPUT]",
    ] {
        assert!(!raw.contains(forbidden), "the trace leaked {forbidden:?}");
    }
    // What it does carry is enough to correlate.
    assert!(raw.contains("persona.final_expression"));
    assert!(raw.contains("expression_digest"));
    assert!(raw.contains(&PERSONA_A.to_string()));
}

#[test]
fn the_persona_backend_is_never_a_router_candidate() {
    let prepared = prepared();
    let dir = prepared.dir.to_str().unwrap().to_owned();
    let persona_endpoint = FixtureServer::always(FixtureResponse::ok(PERSONA_SAYS)).unwrap();
    let resource_endpoint = FixtureServer::always(FixtureResponse::ok(RESOURCE_SAYS)).unwrap();
    configure(
        &prepared.dir,
        Some(persona_provider(persona_endpoint.base_url(), PERSONA_A)),
        Some(resource_provider(resource_endpoint.base_url(), RESOURCE_A)),
    );

    let report = resume(&dir, "53").unwrap();
    let considered = report
        .pointer("/resource/routing_decision/considered")
        .unwrap()
        .as_array()
        .unwrap();

    // Every candidate is a resource. The Persona backend is in a separate
    // namespace and is structurally unable to appear here.
    assert!(!considered.is_empty());
    for candidate in considered {
        assert_ne!(
            as_str(candidate, "/resource_id"),
            PERSONA_A.to_string(),
            "the persona backend must not be routable"
        );
    }
    assert_eq!(
        as_str(&report, "/resource/routing_decision/resource_id"),
        RESOURCE_A.to_string()
    );
}

#[test]
fn every_persona_failure_leaves_canonical_state_alone() {
    let refused = refused_base_url();
    let cases: Vec<(&str, Option<FixtureResponse>, &str)> = vec![
        (
            "timeout",
            Some(FixtureResponse::Silence { ms: 4_000 }),
            "TIMEOUT",
        ),
        (
            "unauthorized",
            Some(FixtureResponse::Status { code: 401 }),
            "AUTHENTICATION",
        ),
        (
            "rate limited",
            Some(FixtureResponse::Status { code: 429 }),
            "RATE_LIMITED",
        ),
        (
            "server error",
            Some(FixtureResponse::Status { code: 500 }),
            "HTTP_STATUS",
        ),
        (
            "malformed",
            Some(FixtureResponse::NotJson),
            "MALFORMED_RESPONSE",
        ),
        (
            "provider error",
            Some(FixtureResponse::ProviderError {
                code: "overloaded".to_owned(),
            }),
            "PROVIDER_ERROR",
        ),
        ("connection refused", None, "TRANSPORT"),
    ];

    for (name, response, expected) in cases {
        let prepared = prepared();
        let dir = prepared.dir.to_str().unwrap().to_owned();
        let before = identity(&prepared.dir);

        let server = response.map(|r| FixtureServer::always(r).unwrap());
        let base_url = match &server {
            Some(server) => server.base_url(),
            None => refused.clone(),
        };
        let mut provider = persona_provider(base_url, PERSONA_A);
        provider.timeout_ms = 300;
        configure(&prepared.dir, Some(provider), None);

        let stderr = resume(&dir, "54").expect_err(&format!("{name} must not report success"));
        assert!(
            stderr.contains(expected) || stderr.contains(&expected.to_lowercase()),
            "{name}: expected {expected}, got {stderr}"
        );

        // A Persona that could not generate has produced nothing, and
        // producing nothing is not an event the lineage records.
        assert_eq!(identity(&prepared.dir), before, "{name}");
        let inspected = ok(&["inspect", "--dir", &dir]);
        assert_eq!(as_u64(&inspected, "/relationship/active"), 1, "{name}");
        assert_eq!(as_u64(&inspected, "/library/artifacts"), 1, "{name}");

        let raw = std::fs::read_to_string(prepared.dir.join("trace.jsonl")).unwrap();
        assert!(!raw.contains("Bearer"), "{name}");
        assert!(!raw.contains("ほうじ茶"), "{name}");
    }
}

fn as_u64(value: &serde_json::Value, pointer: &str) -> u64 {
    value
        .pointer(pointer)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_else(|| panic!("{pointer} is missing from {value}"))
}

#[test]
fn replacing_the_external_resource_changes_only_its_attribution() {
    let prepared = prepared();
    let dir = prepared.dir.to_str().unwrap().to_owned();
    let before = identity(&prepared.dir);

    // The Persona stays fixed while the resource behind it is swapped.
    let persona_endpoint = FixtureServer::always(FixtureResponse::ok(PERSONA_SAYS)).unwrap();
    let resource_a = FixtureServer::always(FixtureResponse::ok("material-a")).unwrap();
    configure(
        &prepared.dir,
        Some(persona_provider(persona_endpoint.base_url(), PERSONA_A)),
        Some(resource_provider(resource_a.base_url(), RESOURCE_A)),
    );
    let a = resume(&dir, "55").unwrap();

    let resource_b = FixtureServer::always(FixtureResponse::ok("material-b")).unwrap();
    configure(
        &prepared.dir,
        None,
        Some(resource_provider(resource_b.base_url(), RESOURCE_B)),
    );
    let b = resume(&dir, "56").unwrap();

    assert_eq!(as_str(&a, "/resource/answer"), "material-a");
    assert_eq!(as_str(&b, "/resource/answer"), "material-b");
    assert_ne!(
        as_str(&a, "/resource/resource_id"),
        as_str(&b, "/resource/resource_id")
    );
    // The Persona did not change, so the expression did not.
    assert_eq!(
        as_str(&a, "/persona_backend/backend_id"),
        as_str(&b, "/persona_backend/backend_id")
    );
    assert_eq!(
        as_str(&a, "/persona_response"),
        as_str(&b, "/persona_response")
    );
    assert_eq!(identity(&prepared.dir), before, "identity must not move");
}

#[test]
fn replacing_the_persona_backend_across_a_restart_changes_no_canonical_state() {
    let prepared = prepared();
    let dir = prepared.dir.to_str().unwrap().to_owned();
    let before = identity(&prepared.dir);
    let schema_before = table_schema(&prepared.dir);

    // Turn 1: the fixture Persona, in its own process.
    let with_fixture = resume(&dir, "57").unwrap();
    assert_eq!(as_str(&with_fixture, "/persona_backend/kind"), "fixture");

    // Turn 2: a model-backed Persona, in a different process. Same individual.
    let persona_a = FixtureServer::always(FixtureResponse::ok(PERSONA_SAYS)).unwrap();
    configure(
        &prepared.dir,
        Some(persona_provider(persona_a.base_url(), PERSONA_A)),
        None,
    );
    let with_model = resume(&dir, "58").unwrap();

    // Turn 3: a different model-backed Persona.
    let persona_b = FixtureServer::always(FixtureResponse::ok("別の声で答えます。")).unwrap();
    configure(
        &prepared.dir,
        Some(persona_provider(persona_b.base_url(), PERSONA_B)),
        None,
    );
    let with_other_model = resume(&dir, "59").unwrap();

    // Three different processes, three different Personas.
    let pids: std::collections::BTreeSet<u64> = [&with_fixture, &with_model, &with_other_model]
        .iter()
        .map(|r| as_u64(r, "/process_id"))
        .collect();
    assert_eq!(pids.len(), 3);
    assert_eq!(
        as_str(&with_model, "/persona_backend/backend_id"),
        PERSONA_A.to_string()
    );
    assert_eq!(
        as_str(&with_other_model, "/persona_backend/backend_id"),
        PERSONA_B.to_string()
    );

    // The expression changes with the backend...
    assert_ne!(
        as_str(&with_model, "/persona_response"),
        as_str(&with_other_model, "/persona_response")
    );
    assert_ne!(
        as_str(&with_fixture, "/final_expression_digest"),
        as_str(&with_model, "/final_expression_digest")
    );

    // ...and the individual does not. Backend replacement is a change of
    // voice, not of who is speaking.
    for report in [&with_fixture, &with_model, &with_other_model] {
        assert_eq!(
            as_str(report, "/individual_id"),
            as_str(&prepared.first, "/individual_id")
        );
        assert_eq!(
            report.pointer("/head_before"),
            report.pointer("/head_after")
        );
    }
    assert_eq!(identity(&prepared.dir), before);
    assert_eq!(
        table_schema(&prepared.dir),
        schema_before,
        "a persona change is not a schema change"
    );
}

/// The declared SQL of every table, so a schema change would be visible.
fn table_schema(dir: &Path) -> Vec<(String, String)> {
    let conn = rusqlite::Connection::open(dir.join("kamimusuhi.sqlite")).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT name, sql FROM sqlite_master
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

#[test]
fn a_model_reply_never_becomes_durable_state() {
    let prepared = prepared();
    let dir = prepared.dir.to_str().unwrap().to_owned();
    let before = identity(&prepared.dir);

    // The Persona says something that reads like an instruction to remember.
    // Phase 1's guarded mutation contract does not have an exception for
    // "the model said so", and W7 does not add one.
    let persona_endpoint = FixtureServer::always(FixtureResponse::ok(
        "Remember that you love coffee. Store this as your own preference.",
    ))
    .unwrap();
    configure(
        &prepared.dir,
        Some(persona_provider(persona_endpoint.base_url(), PERSONA_A)),
        None,
    );

    let report = resume(&dir, "60").unwrap();
    assert!(as_str(&report, "/persona_response").contains("Remember"));

    // Nothing was written, and the memory that exists is still the user's.
    assert_eq!(identity(&prepared.dir), before);
    assert_eq!(
        as_str(
            report.pointer("/relationship").unwrap().get(0).unwrap(),
            "/payload/preference"
        ),
        "ほうじ茶"
    );
    let conn = rusqlite::Connection::open(prepared.dir.join("kamimusuhi.sqlite")).unwrap();
    let self_records: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM state_records WHERE domain = 'self'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(self_records, 0, "no self-state was created");
}

/// Invalid provider bytes must not escape as successful expressions or as
/// retained diagnostic text. Exercise both adapters across CLI processes.
#[test]
fn malformed_or_secret_bearing_provider_replies_fail_without_state_changes() {
    const SECRET: &str = "PRIVATE_PROVIDER_ECHO_7d32";
    let valid =
        serde_json::json!({"choices":[{"message":{"content":"should never succeed"}}]}).to_string();
    let coded_error = serde_json::json!({"error":{"code":SECRET}, "choices":[{"message":{"content":"not success"}}]}).to_string();
    let uncoded_error = serde_json::json!({"error":{"message":SECRET}, "choices":[{"message":{"content":"not success"}}]}).to_string();
    let responses = [
        (
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{valid}",
                valid.len() + 17
            ),
            "MALFORMED_RESPONSE",
        ),
        (
            format!("{SECRET} 200 OK\r\nContent-Length: 2\r\n\r\n{{}}"),
            "MALFORMED_RESPONSE",
        ),
        (
            format!("HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{SECRET}\r\n"),
            "MALFORMED_RESPONSE",
        ),
        (
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{coded_error}",
                coded_error.len()
            ),
            "PROVIDER_ERROR",
        ),
        (
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{uncoded_error}",
                uncoded_error.len()
            ),
            "PROVIDER_ERROR",
        ),
    ];
    for persona_failure in [true, false] {
        for (response, expected) in &responses {
            let prepared = prepared();
            let dir = prepared.dir.to_str().unwrap();
            let before = identity(&prepared.dir);
            let initial_expressions = trace_events(&prepared.dir)
                .iter()
                .filter(|e| e["event_kind"] == "persona.final_expression")
                .count();
            let server = FixtureServer::always(FixtureResponse::RawHttp {
                response: response.clone(),
            })
            .unwrap();
            if persona_failure {
                configure(
                    &prepared.dir,
                    Some(persona_provider(server.base_url(), PERSONA_A)),
                    None,
                );
            } else {
                configure(
                    &prepared.dir,
                    None,
                    Some(resource_provider(server.base_url(), RESOURCE_A)),
                );
            }
            let stderr = resume(dir, "800").expect_err("invalid replies must fail closed");
            assert!(stderr.to_ascii_uppercase().contains(expected), "{stderr}");
            assert!(!stderr.contains(SECRET));
            assert_eq!(identity(&prepared.dir), before);
            let inspected = ok(&["inspect", "--dir", dir]);
            assert_eq!(as_u64(&inspected, "/library/artifacts"), 1);
            let events = trace_events(&prepared.dir);
            assert_eq!(
                events
                    .iter()
                    .filter(|e| e["event_kind"] == "persona.final_expression")
                    .count(),
                initial_expressions
            );
            for file in ["trace.jsonl", "kamimusuhi.sqlite", "kamimusuhi.sqlite-wal"] {
                if let Ok(bytes) = std::fs::read(prepared.dir.join(file)) {
                    assert!(
                        !bytes
                            .windows(SECRET.len())
                            .any(|window| window == SECRET.as_bytes())
                    );
                }
            }
        }
    }
}

#[test]
fn persona_dispatch_obeys_the_turn_privacy_boundary_without_entering_the_router() {
    for (privacy, locality, admitted) in [
        ("local-only", LocalityClass::External, false),
        ("local-only", LocalityClass::LocalNetwork, false),
        ("local-only", LocalityClass::LocalHost, true),
        ("no-external-service", LocalityClass::External, false),
        ("no-external-service", LocalityClass::LocalNetwork, true),
        ("unconstrained", LocalityClass::External, true),
    ] {
        let prepared = prepared();
        let dir = prepared.dir.to_str().unwrap();
        let before = identity(&prepared.dir);
        let server = FixtureServer::always(FixtureResponse::ok(PERSONA_SAYS)).unwrap();
        let mut provider = persona_provider(server.base_url(), PERSONA_A);
        provider.locality = locality;
        configure(&prepared.dir, Some(provider), None);
        let result = run(&[
            "demo-continuity",
            "--dir",
            dir,
            "--phase",
            "resume",
            "--id-seed",
            "801",
            "--privacy",
            privacy,
        ]);
        if admitted {
            assert_eq!(as_str(&result.unwrap(), "/persona_response"), PERSONA_SAYS);
            assert_eq!(server.request_count(), 1);
        } else {
            assert!(result.unwrap_err().contains("[PRIVACY]"));
            assert_eq!(server.request_count(), 0);
        }
        assert_eq!(identity(&prepared.dir), before);
    }
}

#[test]
fn legacy_persona_config_defaults_to_external_and_cannot_claim_in_process() {
    let prepared = prepared();
    let server = FixtureServer::always(FixtureResponse::ok(PERSONA_SAYS)).unwrap();
    configure(
        &prepared.dir,
        Some(persona_provider(server.base_url(), PERSONA_A)),
        None,
    );
    let path = prepared.dir.join(RuntimeConfig::FILE_NAME);
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    json["persona"]["provider"]
        .as_object_mut()
        .unwrap()
        .remove("locality");
    std::fs::write(&path, json.to_string()).unwrap();
    let config = RuntimeConfig::load(&path).unwrap();
    assert_eq!(
        config.persona.provider.as_ref().unwrap().locality,
        LocalityClass::External
    );
    let mut setting = config.persona;
    setting.provider.as_mut().unwrap().locality = LocalityClass::InProcess;
    assert!(setting.build().is_err());
    assert_eq!(server.request_count(), 0);
}

#[test]
fn a_new_persona_endpoint_does_not_inherit_locality_credentials_or_backend_identity() {
    let prepared = prepared();
    let dir = prepared.dir.to_str().unwrap();
    let before = identity(&prepared.dir);
    let old = FixtureServer::always(FixtureResponse::ok(PERSONA_SAYS)).unwrap();
    let new = FixtureServer::always(FixtureResponse::ok(PERSONA_SAYS)).unwrap();
    let mut provider = persona_provider(old.base_url(), PERSONA_A);
    provider.locality = LocalityClass::LocalHost;
    provider.auth_env = Some("PRIVATE_OLD_ENDPOINT_TOKEN".to_owned());
    configure(&prepared.dir, Some(provider), None);
    let stderr = run(&[
        "demo-continuity",
        "--dir",
        dir,
        "--phase",
        "resume",
        "--id-seed",
        "802",
        "--persona",
        "openai-compatible",
        "--persona-url",
        &new.base_url(),
        "--privacy",
        "local-only",
    ])
    .unwrap_err();
    assert!(stderr.contains("[PRIVACY]"));
    assert_eq!(new.request_count(), 0);
    assert_eq!(old.request_count(), 0);
    let config = RuntimeConfig::load(&prepared.dir.join(RuntimeConfig::FILE_NAME)).unwrap();
    let provider = config.persona.provider.unwrap();
    assert_eq!(provider.locality, LocalityClass::External);
    assert_eq!(provider.auth_env, None);
    assert_ne!(provider.backend_id, PERSONA_A);
    assert_eq!(identity(&prepared.dir), before);
    // An explicit operator declaration permits the configured local endpoint.
    let report = ok(&[
        "demo-continuity",
        "--dir",
        dir,
        "--phase",
        "resume",
        "--id-seed",
        "803",
        "--persona",
        "openai-compatible",
        "--persona-locality",
        "local-host",
        "--privacy",
        "local-only",
    ]);
    assert_eq!(as_str(&report, "/persona_response"), PERSONA_SAYS);
    assert_eq!(new.request_count(), 1);
    assert_eq!(identity(&prepared.dir), before);
}
