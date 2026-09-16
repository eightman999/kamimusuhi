//! Real-socket MIO observation and separate Persona API integration. These
//! fixtures verify provenance and failure boundaries, not live neural state.

use std::path::Path;
use std::process::{Command, Output, Stdio};

use kamimusuhi_core::digest::json_digest;
use kamimusuhi_core::evidence::{EvidenceKind, EvidenceStore};
use kamimusuhi_core::ids::{EvidenceId, PersonaBackendId};
use kamimusuhi_core::library::LibraryRepository;
use kamimusuhi_core::mutation::OriginClass;
use kamimusuhi_core::routing::{LocalityClass, PrivacyConstraint};
use kamimusuhi_runtime::config::{PersonaBackendKind, PersonaProviderConfig, PersonaSetting};
use kamimusuhi_runtime::dialogue::DialogueSession;
use kamimusuhi_runtime::mio::MioBinding;
use kamimusuhi_runtime::{ResourceImplementation, Runtime, RuntimeOptions};
use kamimusuhi_testkit::FixedClock;
use kamimusuhi_testkit::http_fixture::{FixtureResponse, FixtureServer};
use serde_json::{Value, json};

const EXPERIMENT: &str = "mio-experiment-fixture";
const GENOME: &str = "mio-genome-fixture";
const BINARY: &str = env!("CARGO_BIN_EXE_kamimusuhi-runtime");

fn snapshot(now: i64) -> Value {
    json!({
        "schema_version": 1,
        "kind": "DERIVED",
        "state_scope": "recorded_evaluation",
        "observed_at_unix_ms": now,
        "experiment_id": EXPERIMENT,
        "genome_id": GENOME,
        "experiment_status": "running",
        "generation": 7,
        "implementation": {
            "action_feedback": "not_connected",
            "lifetime_plasticity": "configured_not_applied",
            "neural_state_scope": "evaluation_local"
        },
        "structure": {
            "n_artificial_neurons": 240,
            "n_enabled_organs": 2,
            "n_enabled_attachments": 4,
            "counts": {"functional": 3, "neutral_structure": 1, "disabled": 2}
        },
        "evaluation": {
            "kind": "RECORDED",
            "evaluation_id": "evaluation-fixture-7",
            "job_id": "job-fixture-7",
            "finished_at_unix_ms": now - 1_000,
            "backend": "mock",
            "scientific_config_hash": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "summary": {"mean_rate_hz": 4.5, "active_fraction": 0.75, "spikes_total": 15},
            "metrics": {"homeostasis_score": 0.8, "task_score": -0.1}
        }
    })
}

fn json_response(value: &Value) -> FixtureResponse {
    let body = value.to_string();
    FixtureResponse::RawHttp {
        response: format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    }
}

fn binding(server: &FixtureServer) -> MioBinding {
    MioBinding::new(mio_url(server), EXPERIMENT.to_owned(), GENOME.to_owned())
}

fn mio_url(server: &FixtureServer) -> String {
    // The shared chat fixture includes /v1; the coordinator API lives at /api.
    server.base_url().trim_end_matches("/v1").to_owned()
}

fn prepared(persona: &FixtureServer, mio: &FixtureServer) -> (tempfile::TempDir, Runtime) {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = Runtime::init(
        dir.path(),
        RuntimeOptions::deterministic(73),
        ResourceImplementation::FakeA,
    )
    .unwrap();
    let mut config = runtime.config().clone();
    config.persona = PersonaSetting {
        backend: PersonaBackendKind::OpenaiCompatible,
        provider: Some(PersonaProviderConfig {
            backend_id: PersonaBackendId::from_u128(0xD1A1),
            locality: LocalityClass::LocalHost,
            base_url: persona.base_url(),
            model: "mio-persona-fixture".to_owned(),
            auth_env: None,
            timeout_ms: 2_000,
            tls_root_ca_path: None,
            system_instruction: None,
        }),
        seed: config.persona.seed.clone(),
    };
    config.mio = Some(binding(mio));
    runtime.save_config(config).unwrap();
    (dir, runtime)
}

fn prompt(persona: &FixtureServer, index: usize) -> String {
    let request = &persona.requests()[index];
    assert_eq!(request.path, "/v1/chat/completions");
    let body: Value = serde_json::from_str(&request.body).unwrap();
    body["messages"][1]["content"].as_str().unwrap().to_owned()
}

fn section(prompt: &str, name: &str) -> Option<Value> {
    let line = prompt
        .split_once(&format!("[{name}]\n"))?
        .1
        .lines()
        .next()?;
    Some(serde_json::from_str(line).unwrap())
}

#[test]
fn snapshot_provenance_reaches_persona_and_evidence_without_changing_canonical_head() {
    let source = snapshot(FixedClock::BASELINE_UNIX_MILLIS);
    let mio = FixtureServer::always(json_response(&source)).unwrap();
    let persona = FixtureServer::always(FixtureResponse::ok("記録を参照した返答。")).unwrap();
    let (_dir, mut runtime) = prepared(&persona, &mio);
    assert_eq!(
        runtime.now().unix_millis(),
        FixedClock::BASELINE_UNIX_MILLIS
    );
    let head = runtime.head().unwrap();
    let mut session =
        DialogueSession::start(&mut runtime, "alice", PrivacyConstraint::LocalOnly).unwrap();
    let reply = session
        .turn(&mut runtime, "PRIVATE_USER_INPUT", |_| Ok(()))
        .unwrap();
    let context = reply.mio_observation.as_ref().unwrap();
    let observation = &context["observation"];
    assert_eq!(observation["connection"], "connected");
    assert_eq!(observation["association"], "operator_selected_mio");
    assert_eq!(observation["experiment_id"], EXPERIMENT);
    assert_eq!(observation["genome_id"], GENOME);
    assert_eq!(observation["snapshot"], source);
    assert_eq!(observation["snapshot_digest"], json_digest(&source));
    assert_eq!(observation["evaluation_freshness"], "recent_record");
    assert_eq!(observation["evaluation_age_ms"], 1_000);
    assert_eq!(
        section(&prompt(&persona, 0), "MIO_OBSERVATION"),
        Some(context.clone())
    );
    let observed = section(&prompt(&persona, 0), "OBSERVED_RUNTIME").unwrap();
    assert_eq!(observed["individual_id"], json!(runtime.individual_id()));
    assert_ne!(observed["individual_id"], observation["genome_id"]);
    assert_eq!(
        observed["experimental_neural_state"],
        "recorded_mio_evaluations_only"
    );

    let evidence_id: EvidenceId = serde_json::from_value(context["evidence_id"].clone()).unwrap();
    let evidence = runtime.store().get(evidence_id).unwrap().unwrap();
    assert_eq!(evidence.kind, EvidenceKind::SystemEvent);
    assert_eq!(evidence.origin_class, OriginClass::Observed);
    assert_eq!(evidence.individual_id, runtime.individual_id());
    assert_eq!(evidence.session_id, Some(reply.session_id));
    assert_eq!(evidence.turn_id, Some(reply.turn_id));
    assert_eq!(evidence.payload["event"], "mio_observed");
    assert_eq!(&evidence.payload["observation"], observation);
    let generated = runtime
        .store()
        .get(reply.generated_evidence_id)
        .unwrap()
        .unwrap();
    assert_eq!(&generated.payload["mio_observation"], context);
    assert_eq!(runtime.head().unwrap(), head);
    let requests = mio.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].path,
        format!("/api/dialogue/organisms/{GENOME}")
    );
    assert!(
        requests[0].body.is_empty(),
        "MIO must receive no conversation body"
    );
    assert!(
        requests[0].authorization.is_none(),
        "MIO received API credentials"
    );
}

#[test]
fn large_integer_spike_counts_are_preserved_without_float_rounding() {
    for count in [9_007_199_254_740_993_u64, u64::MAX] {
        let mut source = snapshot(FixedClock::BASELINE_UNIX_MILLIS);
        source["evaluation"]["summary"]["spikes_total"] = json!(count);
        let mio = FixtureServer::always(json_response(&source)).unwrap();
        let observation = binding(&mio).observe(&FixedClock::baseline());
        assert_eq!(observation.connection, "connected");
        assert_eq!(serde_json::to_value(&observation.snapshot).unwrap(), source);
        assert_eq!(observation.snapshot_digest, Some(json_digest(&source)));
    }
}

#[test]
fn research_recall_preserves_invalid_results_limits_and_library_evidence() {
    let mio =
        FixtureServer::always(json_response(&snapshot(FixedClock::BASELINE_UNIX_MILLIS))).unwrap();
    let persona = FixtureServer::always(FixtureResponse::ok("記録を参照する模擬応答。")).unwrap();
    let (_dir, mut runtime) = prepared(&persona, &mio);
    let head = runtime.head().unwrap();
    let mut session =
        DialogueSession::start(&mut runtime, "alice", PrivacyConstraint::LocalOnly).unwrap();
    let reply = session
        .turn(&mut runtime, "G0-v6の実験成果を教えて。", |_| {
            Ok(())
        })
        .unwrap();
    let selected = reply.research_findings["selected"].as_array().unwrap();
    assert!(!selected.is_empty() && selected.len() <= 4);
    assert_eq!(selected[0]["finding"]["id"], "g0-grounding-lineage");
    assert_eq!(selected[0]["finding"]["status"], "invalid");
    assert!(
        !selected[0]["finding"]["limitations"]
            .as_str()
            .unwrap()
            .is_empty()
    );
    assert!(
        !selected[0]["finding"]["sources"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        section(&prompt(&persona, 0), "RESEARCH_FINDINGS"),
        Some(reply.research_findings.clone())
    );
    let evidence_id: EvidenceId =
        serde_json::from_value(reply.research_findings["evidence_id"].clone()).unwrap();
    let evidence = runtime.store().get(evidence_id).unwrap().unwrap();
    assert_eq!(evidence.kind, EvidenceKind::LibraryExcerpt);
    assert_eq!(evidence.origin_class, OriginClass::Reported);
    assert_eq!(
        evidence.payload["context"]["selected"],
        reply.research_findings["selected"]
    );
    assert_eq!(
        LibraryRepository::artifacts(runtime.store()).unwrap().len(),
        8
    );
    let greeting = session
        .turn(&mut runtime, "こんにちは。", |_| Ok(()))
        .unwrap();
    assert_eq!(greeting.research_findings["selected"], json!([]));
    assert_eq!(runtime.head().unwrap(), head);
}

#[test]
fn mio_state_selects_research_using_one_snapshot_per_turn() {
    let mio = FixtureServer::start(vec![
        json_response(&snapshot(FixedClock::BASELINE_UNIX_MILLIS)),
        FixtureResponse::Status { code: 503 },
    ])
    .unwrap();
    let persona = FixtureServer::always(FixtureResponse::ok("記録に沿う模擬応答。")).unwrap();
    let (_dir, mut runtime) = prepared(&persona, &mio);
    let mut session =
        DialogueSession::start(&mut runtime, "alice", PrivacyConstraint::LocalOnly).unwrap();
    let first = session
        .turn(&mut runtime, "MIOの状態を教えて。", |_| Ok(()))
        .unwrap();
    assert_eq!(
        first.research_findings["selected"][0]["finding"]["id"],
        "h0-homeostasis"
    );
    assert_eq!(
        first.mio_observation.as_ref().unwrap()["observation"]["snapshot"]["implementation"]["action_feedback"],
        "not_connected"
    );
    let second = session
        .turn(&mut runtime, "MIOの状態を教えて。", |_| Ok(()))
        .unwrap();
    assert_eq!(
        second.research_findings["selected"][0]["finding"]["id"],
        "o0-object-continuity"
    );
    assert_eq!(
        second.mio_observation.as_ref().unwrap()["observation"]["connection"],
        "unavailable"
    );
    assert!(
        second.mio_observation.as_ref().unwrap()["observation"]
            .get("snapshot")
            .is_none()
    );
    assert_eq!(mio.request_count(), 2);
    assert_eq!(persona.request_count(), 2);
}

#[test]
fn invalid_identity_schema_ranges_and_snapshot_times_are_not_admitted() {
    let now = FixedClock::BASELINE_UNIX_MILLIS;
    let cases = [
        ("/genome_id", json!("other-genome"), "mio_identity_mismatch"),
        (
            "/experiment_id",
            json!("other-experiment"),
            "mio_identity_mismatch",
        ),
        ("/schema_version", json!(2), "unsupported_snapshot"),
        (
            "/implementation/action_feedback",
            json!("connected"),
            "unsupported_implementation",
        ),
        (
            "/state_scope",
            json!("live_neural_state"),
            "unsupported_snapshot",
        ),
        (
            "/evaluation/summary/active_fraction",
            json!(1.1),
            "invalid_activity",
        ),
        (
            "/evaluation/summary/spikes_total",
            json!(2.5),
            "invalid_activity",
        ),
        (
            "/evaluation/summary/spikes_total",
            json!(2.0),
            "invalid_activity",
        ),
        (
            "/evaluation/summary/spikes_total",
            json!(1e30),
            "invalid_activity",
        ),
        (
            "/evaluation/scientific_config_hash",
            json!("unknown-hash"),
            "invalid_evaluation",
        ),
        (
            "/evaluation/metrics/homeostasis_score",
            json!(-0.1),
            "invalid_metrics",
        ),
        (
            "/observed_at_unix_ms",
            json!(now + 5_001),
            "source_clock_invalid",
        ),
        (
            "/observed_at_unix_ms",
            json!(now - 300_001),
            "snapshot_stale",
        ),
        (
            "/evaluation/finished_at_unix_ms",
            json!(now + 5_001),
            "evaluation_clock_invalid",
        ),
        (
            "/structure/n_artificial_neurons",
            json!(-1),
            "invalid_snapshot",
        ),
        (
            "/structure",
            json!({"counts": {}, "instruction": "untrusted extra field"}),
            "invalid_snapshot",
        ),
    ];
    for (path, replacement, error) in cases {
        let mut source = snapshot(now);
        *source.pointer_mut(path).unwrap() = replacement;
        let mio = FixtureServer::always(json_response(&source)).unwrap();
        let result = binding(&mio).observe(&FixedClock::baseline());
        assert_eq!(result.connection, "unavailable", "{path}");
        assert_eq!(result.error_code.as_deref(), Some(error), "{path}");
        assert!(result.snapshot.is_none(), "{path}");
        assert!(result.snapshot_digest.is_none(), "{path}");
        assert!(result.evaluation_freshness.is_none(), "{path}");
    }
}

#[test]
fn completed_record_age_is_separate_from_successful_snapshot_retrieval() {
    let now = FixedClock::BASELINE_UNIX_MILLIS;
    for (age, expected) in [
        (Some(300_000), "recent_record"),
        (Some(300_001), "stale_record"),
        (None, "unknown_timestamp"),
    ] {
        let mut source = snapshot(now);
        match age {
            Some(age) => source["evaluation"]["finished_at_unix_ms"] = json!(now - age),
            None => {
                source["evaluation"]
                    .as_object_mut()
                    .unwrap()
                    .remove("finished_at_unix_ms");
            }
        }
        let mio = FixtureServer::always(json_response(&source)).unwrap();
        let result = binding(&mio).observe(&FixedClock::baseline());
        assert_eq!(result.connection, "connected");
        assert_eq!(result.evaluation_freshness.as_deref(), Some(expected));
        assert_eq!(result.evaluation_age_ms, age.map(|age| age as u64));
        assert!(result.snapshot.is_some());
    }
    let mut source = snapshot(now);
    source.as_object_mut().unwrap().remove("evaluation");
    let mio = FixtureServer::always(json_response(&source)).unwrap();
    let result = binding(&mio).observe(&FixedClock::baseline());
    assert_eq!(result.connection, "connected");
    assert_eq!(result.evaluation_freshness.as_deref(), Some("no_record"));
    assert!(result.evaluation_age_ms.is_none());
}

#[test]
fn source_failure_after_success_does_not_reuse_the_snapshot_and_dialogue_continues() {
    let mio = FixtureServer::start(vec![
        json_response(&snapshot(FixedClock::BASELINE_UNIX_MILLIS)),
        FixtureResponse::Status { code: 503 },
    ])
    .unwrap();
    let persona = FixtureServer::always(FixtureResponse::ok("対話を続ける。")).unwrap();
    let (_dir, mut runtime) = prepared(&persona, &mio);
    let head = runtime.head().unwrap();
    let mut session =
        DialogueSession::start(&mut runtime, "alice", PrivacyConstraint::LocalOnly).unwrap();
    let first = session.turn(&mut runtime, "一回目", |_| Ok(())).unwrap();
    let second = session.turn(&mut runtime, "二回目", |_| Ok(())).unwrap();
    assert_eq!(
        first.mio_observation.as_ref().unwrap()["observation"]["connection"],
        "connected"
    );
    assert_eq!(second.response, "対話を続ける。");
    let context = second.mio_observation.as_ref().unwrap();
    let observation = &context["observation"];
    assert_eq!(observation["connection"], "unavailable");
    assert_eq!(observation["error_code"], "source_http_error");
    assert!(observation.get("snapshot").is_none());
    assert!(observation.get("snapshot_digest").is_none());
    assert!(observation.get("evaluation_freshness").is_none());
    assert_ne!(
        context["evidence_id"],
        first.mio_observation.unwrap()["evidence_id"]
    );
    let sent = prompt(&persona, 1);
    assert_eq!(section(&sent, "MIO_OBSERVATION"), Some(context.clone()));
    assert!(!sent.contains("evaluation-fixture-7"));
    assert_eq!(
        second.observed_runtime["experimental_neural_state"],
        "mio_unavailable"
    );
    assert_eq!(mio.request_count(), 2);
    assert_eq!(persona.request_count(), 2);
    assert_eq!(runtime.head().unwrap(), head);
}

fn run(dir: &Path, command: &str, flags: &[&str]) -> Output {
    Command::new(BINARY)
        .arg(command)
        .arg("--dir")
        .arg(dir)
        .args(["--clock", "fixed"])
        .args(flags)
        .env(
            "KAMIMUSUHI_MIO_FIXTURE_KEY",
            "dummy-mio-fixture-key-not-real",
        )
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn cli_pin_persists_across_processes_disconnects_and_never_forwards_persona_auth_to_mio() {
    let dir = tempfile::tempdir().unwrap();
    success(run(dir.path(), "init", &[]));
    let mio =
        FixtureServer::always(json_response(&snapshot(FixedClock::BASELINE_UNIX_MILLIS))).unwrap();
    let persona = FixtureServer::always(FixtureResponse::ok("API fixture reply")).unwrap();
    let first = success(run(
        dir.path(),
        "talk",
        &[
            "--message",
            "PRIVATE_CLI_USER_INPUT",
            "--persona",
            "openai-compatible",
            "--persona-url",
            &persona.base_url(),
            "--persona-model",
            "mio-cli-fixture",
            "--persona-locality",
            "local-host",
            "--persona-auth-env",
            "KAMIMUSUHI_MIO_FIXTURE_KEY",
            "--mio-url",
            &mio_url(&mio),
            "--mio-experiment",
            EXPERIMENT,
            "--mio-genome",
            GENOME,
            "--mio-max-age-secs",
            "60",
        ],
    ));
    assert_eq!(
        first["mio_observation"]["observation"]["connection"],
        "connected"
    );
    let configured = std::fs::read(dir.path().join("runtime.json")).unwrap();
    let config: Value = serde_json::from_slice(&configured).unwrap();
    assert_eq!(config["mio"]["base_url"], mio_url(&mio));
    assert_eq!(config["mio"]["experiment_id"], EXPERIMENT);
    assert_eq!(config["mio"]["genome_id"], GENOME);
    assert_eq!(config["mio"]["max_age_ms"], 60_000);
    let next = success(run(dir.path(), "talk", &["--message", "second process"]));
    assert_eq!(
        next["mio_observation"]["observation"]["connection"],
        "connected"
    );
    assert_eq!(
        std::fs::read(dir.path().join("runtime.json")).unwrap(),
        configured
    );
    assert_eq!(mio.request_count(), 2);
    for request in mio.requests() {
        assert!(request.body.is_empty(), "MIO received user material");
        assert!(
            request.authorization.is_none(),
            "MIO received API credentials"
        );
    }
    for request in persona.requests() {
        assert!(
            request.authorization.is_some(),
            "Persona API was not authenticated"
        );
        assert!(!request.body.contains("dummy-mio-fixture-key-not-real"));
    }
    let disconnected = success(run(
        dir.path(),
        "talk",
        &["--message", "disconnect", "--no-mio"],
    ));
    assert!(disconnected["mio_observation"].is_null());
    assert_eq!(mio.request_count(), 2);
    assert!(section(&prompt(&persona, 2), "MIO_OBSERVATION").is_none());
    let saved: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("runtime.json")).unwrap()).unwrap();
    assert!(saved["mio"].is_null());
    success(run(
        dir.path(),
        "talk",
        &["--message", "still disconnected"],
    ));
    assert_eq!(mio.request_count(), 2);
    assert_eq!(persona.request_count(), 4);
}

#[test]
fn incomplete_or_conflicting_cli_pins_do_not_persist_or_contact_either_service() {
    let dir = tempfile::tempdir().unwrap();
    success(run(dir.path(), "init", &[]));
    let configured = std::fs::read(dir.path().join("runtime.json")).unwrap();
    let mio =
        FixtureServer::always(json_response(&snapshot(FixedClock::BASELINE_UNIX_MILLIS))).unwrap();
    let persona = FixtureServer::always(FixtureResponse::ok("must not be reached")).unwrap();
    let mio_url = mio_url(&mio);
    for flags in [
        vec!["--mio-url", mio_url.as_str()],
        vec!["--no-mio", "--mio-url", mio_url.as_str()],
        vec![
            "--mio-url",
            mio_url.as_str(),
            "--mio-experiment",
            EXPERIMENT,
            "--mio-genome",
            "../other",
        ],
        vec![
            "--mio-url",
            mio_url.as_str(),
            "--mio-experiment",
            EXPERIMENT,
            "--mio-genome",
            GENOME,
            "--mio-max-age-secs",
            "0",
        ],
    ] {
        let persona_url = persona.base_url();
        let mut args = vec![
            "--message",
            "never transmitted",
            "--persona",
            "openai-compatible",
            "--persona-url",
            &persona_url,
            "--persona-model",
            "mio-fixture",
            "--persona-locality",
            "local-host",
        ];
        args.extend(flags);
        let output = run(dir.path(), "talk", &args);
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(
            std::fs::read(dir.path().join("runtime.json")).unwrap(),
            configured
        );
    }
    assert_eq!(mio.request_count(), 0);
    assert_eq!(persona.request_count(), 0);
}
