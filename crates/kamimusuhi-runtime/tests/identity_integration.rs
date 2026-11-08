//! FI3 at real runtime/session boundaries. These use trusted local fixtures,
//! never a paid or remote model. Independent SQLite queries prove that refusal
//! precedes canonical/session/evidence writes.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use kamimusuhi_core::digest::content_digest;
use kamimusuhi_core::ids::PersonaBackendId;
use kamimusuhi_core::routing::{LocalityClass, PrivacyConstraint};
use kamimusuhi_runtime::config::{PersonaProviderConfig, ReasoningSetting, RuntimeConfig};
use kamimusuhi_runtime::dialogue::DialogueSession;
use kamimusuhi_runtime::identity_gate::{GateError, GateState, MANIFEST_FILE};
use kamimusuhi_runtime::llm_jev::{MockLanguageProvider, RuleBasedDecisionProvider};
use kamimusuhi_runtime::{ResourceImplementation, Runtime, RuntimeError, RuntimeOptions};
use serde_json::json;

fn init(path: &Path) -> Runtime {
    Runtime::init(
        path,
        RuntimeOptions::deterministic(0xF130),
        ResourceImplementation::FakeA,
    )
    .unwrap()
}
fn reopen(path: &Path) -> Runtime {
    Runtime::open(path, RuntimeOptions::deterministic(0xF230)).unwrap()
}
fn start(runtime: &mut Runtime) -> Result<DialogueSession, RuntimeError> {
    DialogueSession::start_with_providers(
        runtime,
        "identity-fixture",
        PrivacyConstraint::LocalOnly,
        Box::new(RuleBasedDecisionProvider),
        Some(Box::new(MockLanguageProvider)),
    )
}
fn accept(runtime: &Runtime) {
    let status = runtime.identity_status().unwrap();
    runtime.accept_identity(&status.review_token).unwrap();
}
fn identity_refusal(error: &RuntimeError) -> bool {
    matches!(
        error,
        RuntimeError::IdentityGate(
            GateError::AcceptanceRequired { .. } | GateError::Blocked(_) | GateError::StaleReview
        )
    )
}
fn counters(path: &Path) -> Vec<i64> {
    let c = rusqlite::Connection::open_with_flags(
        path.join("kamimusuhi.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    [
        "individuals",
        "continuity_heads",
        "writer_epochs",
        "canonical_commits",
        "sessions",
        "turns",
        "evidence_records",
        "state_records",
    ]
    .iter()
    .map(|table| {
        c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    })
    .collect()
}
fn provider(model: &str) -> PersonaProviderConfig {
    PersonaProviderConfig {
        backend_id: PersonaBackendId::from_u128(0xFA01),
        locality: LocalityClass::External,
        base_url: "https://fixture.invalid/v1".to_owned(),
        model: model.to_owned(),
        auth_env: Some("UNREAD_FIXTURE_KEY".to_owned()),
        timeout_ms: 1000,
        tls_root_ca_path: None,
        system_instruction: None,
        reasoning: ReasoningSetting::Off,
        extra_body: None,
    }
}

#[test]
fn new_individual_is_enrolled_and_reopens_with_same_identity() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = init(dir.path());
    let individual = runtime.individual();
    let head = runtime.head().unwrap();
    assert_eq!(
        runtime.identity_status().unwrap().state,
        GateState::Continue
    );
    assert!(dir.path().join(MANIFEST_FILE).exists());
    drop(runtime);
    let restored = reopen(dir.path());
    assert_eq!(restored.individual(), individual);
    assert_eq!(restored.head().unwrap(), head);
    restored.ensure_identity().unwrap();
}

#[test]
fn legacy_database_remains_readable_but_cannot_start_dialogue_before_explicit_acceptance() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = init(dir.path());
    let individual = runtime.individual();
    let head = runtime.head().unwrap();
    drop(runtime);
    std::fs::remove_file(dir.path().join(MANIFEST_FILE)).unwrap();
    let before = counters(dir.path());
    let mut legacy = reopen(dir.path());
    assert_eq!(legacy.individual(), individual);
    assert_eq!(legacy.head().unwrap(), head);
    assert_eq!(
        legacy.identity_status().unwrap().state,
        GateState::AcceptRequired
    );
    assert!(!dir.path().join(MANIFEST_FILE).exists());
    let error = match start(&mut legacy) {
        Ok(_) => panic!("unreviewed legacy session was admitted"),
        Err(e) => e,
    };
    assert!(identity_refusal(&error));
    assert_eq!(counters(dir.path()), before);
    assert!(!dir.path().join(MANIFEST_FILE).exists());
    accept(&legacy);
    let mut session = start(&mut legacy).unwrap();
    let reply = session.turn(&mut legacy, "こんにちは", |_| Ok(())).unwrap();
    assert!(reply.response.starts_with("mock:"));
    assert_eq!(legacy.individual(), individual);
}

#[test]
fn model_prompt_changes_require_review_without_changing_the_canonical_head() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());
    // Fake backend and explicit language fixture keep this declaration test offline.
    let mut config = runtime.config().clone();
    config.persona.provider = Some(provider("model-a"));
    runtime.save_config(config).unwrap();
    accept(&runtime);
    let individual = runtime.individual();
    let head = runtime.head().unwrap();
    let before = counters(dir.path());
    let mut changed = runtime.config().clone();
    changed.persona.provider.as_mut().unwrap().model = "model-b".to_owned();
    changed
        .persona
        .provider
        .as_mut()
        .unwrap()
        .system_instruction = Some("operator-authored replacement framing".to_owned());
    runtime.save_config(changed).unwrap();
    assert_eq!(runtime.head().unwrap(), head);
    assert_eq!(counters(dir.path()), before);
    assert_eq!(
        runtime.identity_status().unwrap().state,
        GateState::AcceptRequired
    );
    let error = match start(&mut runtime) {
        Ok(_) => panic!("unreviewed model entered a session"),
        Err(e) => e,
    };
    assert!(identity_refusal(&error));
    assert_eq!(counters(dir.path()), before);
    accept(&runtime);
    runtime.ensure_identity().unwrap();
    let mut session = start(&mut runtime).unwrap();
    let reply = session
        .turn(&mut runtime, "こんにちは", |_| Ok(()))
        .unwrap();
    assert!(reply.response.starts_with("mock:"));
    assert_eq!(runtime.individual(), individual);
    assert_eq!(runtime.head().unwrap(), head);
}

#[test]
fn already_open_session_is_refused_after_another_config_revision_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());
    let mut session = start(&mut runtime).unwrap();
    let mut changed = runtime.config().clone();
    changed.avatar_self_description = Some("new operator framing".to_owned());
    runtime.save_config(changed).unwrap();
    accept(&runtime);
    runtime.ensure_identity().unwrap();
    let before = counters(dir.path());
    let delivered = AtomicBool::new(false);
    let error = session
        .turn(&mut runtime, "こんにちは", |_| {
            delivered.store(true, Ordering::SeqCst);
            Ok(())
        })
        .unwrap_err();
    assert!(
        identity_refusal(&error),
        "expected a stable identity refusal, got {error:?}"
    );
    assert!(!delivered.load(Ordering::SeqCst));
    assert_eq!(counters(dir.path()), before);
    let mut fresh = start(&mut runtime).unwrap();
    assert!(
        fresh
            .turn(&mut runtime, "こんにちは", |_| Ok(()))
            .unwrap()
            .response
            .starts_with("mock:")
    );
}

#[test]
fn external_config_edit_is_detected_even_after_acceptance_while_runtime_snapshot_is_old() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());
    let original = runtime.identity_manifest().unwrap();
    let mut changed = runtime.config().clone();
    changed.avatar_name = Some("new display framing".to_owned());
    changed
        .save(&dir.path().join(RuntimeConfig::FILE_NAME))
        .unwrap();
    assert_eq!(runtime.identity_manifest().unwrap(), original);
    assert_eq!(
        runtime.identity_status().unwrap().state,
        GateState::AcceptRequired
    );
    let before = counters(dir.path());
    assert!(runtime.ensure_identity().is_err());
    accept(&runtime);
    assert_eq!(
        runtime.identity_status().unwrap().state,
        GateState::Continue
    );
    assert!(runtime.ensure_identity().is_err());
    let error = match start(&mut runtime) {
        Ok(_) => panic!("old runtime snapshot entered a session"),
        Err(e) => e,
    };
    assert!(identity_refusal(&error));
    assert_eq!(counters(dir.path()), before);
    drop(runtime);
    let restored = reopen(dir.path());
    restored.ensure_identity().unwrap();
}

#[test]
fn accepted_proxy_model_set_allows_order_changes_but_new_models_require_review() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());
    let head = runtime.head().unwrap();
    let a = content_digest(b"actual-a");
    let b = content_digest(b"actual-b");
    let context = json!({"version":1,"model_hashes":[a,b],"route_pins":{"http://node":content_digest(b"declared stable routes")},"declarations":{"self":content_digest(b"same behavior")}});
    runtime.set_identity_context(context.clone()).unwrap();
    assert_eq!(
        runtime.identity_status().unwrap().state,
        GateState::AcceptRequired
    );
    accept(&runtime);
    assert!(
        runtime
            .identity_manifest()
            .unwrap()
            .allows_model_name("actual-a")
    );
    assert!(
        runtime
            .identity_manifest()
            .unwrap()
            .allows_model_name("actual-b")
    );
    let mut reordered = context;
    reordered["model_hashes"].as_array_mut().unwrap().reverse();
    runtime.set_identity_context(reordered.clone()).unwrap();
    runtime.ensure_identity().unwrap();
    reordered["model_hashes"]
        .as_array_mut()
        .unwrap()
        .push(json!(content_digest(b"unreviewed-model")));
    runtime.set_identity_context(reordered).unwrap();
    assert_eq!(
        runtime.identity_status().unwrap().state,
        GateState::AcceptRequired
    );
    assert!(runtime.ensure_identity().is_err());
    assert_eq!(runtime.head().unwrap(), head);
}

#[test]
fn stale_review_token_and_corrupt_baseline_cannot_adopt_an_unreviewed_revision() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());
    let head = runtime.head().unwrap();
    let mut changed = runtime.config().clone();
    changed.avatar_name = Some("revision-a".to_owned());
    runtime.save_config(changed).unwrap();
    let stale = runtime.identity_status().unwrap().review_token;
    let mut next = runtime.config().clone();
    next.avatar_name = Some("revision-b".to_owned());
    runtime.save_config(next).unwrap();
    assert!(matches!(
        runtime.accept_identity(&stale),
        Err(RuntimeError::IdentityGate(GateError::StaleReview))
    ));
    assert_eq!(
        runtime.identity_status().unwrap().state,
        GateState::AcceptRequired
    );
    accept(&runtime);
    let path = dir.path().join(MANIFEST_FILE);
    std::fs::write(&path, b"broken identity manifest").unwrap();
    assert_eq!(runtime.identity_status().unwrap().state, GateState::Blocked);
    let error = match start(&mut runtime) {
        Ok(_) => panic!("corrupt baseline admitted"),
        Err(e) => e,
    };
    assert!(identity_refusal(&error));
    assert_eq!(std::fs::read(&path).unwrap(), b"broken identity manifest");
    assert_eq!(runtime.head().unwrap(), head);
}

#[test]
fn init_adopting_an_existing_database_does_not_silently_enroll_missing_baseline() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = init(dir.path());
    let individual = runtime.individual();
    let head = runtime.head().unwrap();
    drop(runtime);
    std::fs::remove_file(dir.path().join(RuntimeConfig::FILE_NAME)).unwrap();
    std::fs::remove_file(dir.path().join(MANIFEST_FILE)).unwrap();
    let adopted = Runtime::init(
        dir.path(),
        RuntimeOptions::deterministic(0xF330),
        ResourceImplementation::FakeA,
    )
    .unwrap();
    assert_eq!(adopted.individual(), individual);
    assert_eq!(adopted.head().unwrap(), head);
    assert_eq!(
        adopted.identity_status().unwrap().state,
        GateState::AcceptRequired
    );
    assert!(!dir.path().join(MANIFEST_FILE).exists());
}

#[test]
fn cli_review_token_cas_and_explicit_proxy_context_preserve_the_individual() {
    use std::process::{Command, Output};
    fn cli(dir: &Path, command: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_kamimusuhi-runtime"))
            .arg(command)
            .arg("--dir")
            .arg(dir)
            .args(args)
            .env_remove("KAMIMUSUHI_IDENTITY_DECLARATIONS")
            .output()
            .unwrap()
    }
    fn status(dir: &Path, args: &[&str]) -> serde_json::Value {
        let output = cli(dir, "identity-status", args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
    let dir = tempfile::tempdir().unwrap();
    let runtime = init(dir.path());
    let individual = runtime.individual();
    let head = runtime.head().unwrap();
    let before = counters(dir.path());
    drop(runtime);
    let context_path = dir.path().join("operator-route-review.json");
    let context = json!({"version":1,"model_hashes":[content_digest(b"declared-api-model")],"route_pins":{"http://node":content_digest(b"pinned declaration")}});
    std::fs::write(&context_path, serde_json::to_vec(&context).unwrap()).unwrap();
    let args = ["--identity-context-file", context_path.to_str().unwrap()];
    let review = status(dir.path(), &args);
    assert_eq!(review["state"], "accept_required");
    let stale = review["review_token"].as_str().unwrap().to_owned();
    let mut changed = context;
    changed["model_hashes"]
        .as_array_mut()
        .unwrap()
        .push(json!(content_digest(b"second-api-model")));
    std::fs::write(&context_path, serde_json::to_vec(&changed).unwrap()).unwrap();
    let refused = cli(
        dir.path(),
        "identity-accept",
        &[args[0], args[1], "--review-token", &stale],
    );
    assert!(!refused.status.success());
    assert_eq!(status(dir.path(), &args)["state"], "accept_required");
    let current = status(dir.path(), &args);
    let token = current["review_token"].as_str().unwrap();
    let accepted = cli(
        dir.path(),
        "identity-accept",
        &[args[0], args[1], "--review-token", token],
    );
    assert!(
        accepted.status.success(),
        "{}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    let accepted: serde_json::Value = serde_json::from_slice(&accepted.stdout).unwrap();
    assert_eq!(accepted["state"], "continue");
    assert_eq!(status(dir.path(), &args)["state"], "continue");
    assert_eq!(counters(dir.path()), before);
    let mut restored = reopen(dir.path());
    restored.set_identity_context(changed).unwrap();
    restored.ensure_identity().unwrap();
    assert_eq!(restored.individual(), individual);
    assert_eq!(restored.head().unwrap(), head);
}

#[test]
fn acceptance_waits_for_inflight_dispatch_and_old_output_is_not_delivered() {
    use kamimusuhi_core::persona::PersonaBackendDescriptor;
    use kamimusuhi_runtime::llm_jev::{
        ConversationError, LanguageProvider, LanguageRequest, LanguageResult,
    };
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::Duration;
    struct PausedLanguage {
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }
    impl LanguageProvider for PausedLanguage {
        fn generate(&self, request: &LanguageRequest) -> Result<LanguageResult, ConversationError> {
            self.entered.send(()).unwrap();
            self.release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(3))
                .unwrap();
            MockLanguageProvider.generate(request)
        }
        fn descriptor(&self) -> PersonaBackendDescriptor {
            MockLanguageProvider.descriptor()
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let mut session = DialogueSession::start_with_providers(
        &mut runtime,
        "inflight",
        PrivacyConstraint::LocalOnly,
        Box::new(RuleBasedDecisionProvider),
        Some(Box::new(PausedLanguage {
            entered: entered_tx,
            release: Mutex::new(release_rx),
        })),
    )
    .unwrap();
    let emitted = Arc::new(AtomicBool::new(false));
    let turn_emitted = emitted.clone();
    let turn = std::thread::spawn(move || {
        session.turn(&mut runtime, "こんにちは", |_| {
            turn_emitted.store(true, Ordering::SeqCst);
            Ok(())
        })
    });
    entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let mut reviewer = Runtime::open(dir.path(), RuntimeOptions::deterministic(0xF430)).unwrap();
    let mut config = reviewer.config().clone();
    config.avatar_self_description = Some("reviewed while inference was running".to_owned());
    reviewer.save_config(config).unwrap();
    let token = reviewer.identity_status().unwrap().review_token;
    let before = counters(dir.path());
    let accepted = Arc::new(AtomicBool::new(false));
    let accepting = accepted.clone();
    let review = std::thread::spawn(move || {
        reviewer.accept_identity(&token).unwrap();
        accepting.store(true, Ordering::SeqCst);
    });
    std::thread::sleep(Duration::from_millis(30));
    assert!(
        !accepted.load(Ordering::SeqCst),
        "acceptance raced through the active identity lease"
    );
    release_tx.send(()).unwrap();
    let error = turn.join().unwrap().unwrap_err();
    assert!(identity_refusal(&error));
    review.join().unwrap();
    assert!(accepted.load(Ordering::SeqCst));
    assert!(!emitted.load(Ordering::SeqCst));
    assert_eq!(counters(dir.path()), before);
    let fresh = reopen(dir.path());
    fresh.ensure_identity().unwrap();
}

#[test]
fn dynamic_provider_registration_cannot_bypass_the_accepted_session_declaration() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());
    let mut config = runtime.config().clone();
    config
        .language_providers
        .insert("reviewed".into(), provider("declared-model"));
    runtime.save_config(config).unwrap();
    accept(&runtime);
    let mut session = start(&mut runtime).unwrap();
    let before = counters(dir.path());
    assert!(identity_refusal(
        &session
            .set_language_provider("reviewed", &provider("changed-model"))
            .unwrap_err()
    ));
    assert!(identity_refusal(
        &session
            .set_language_provider("unreviewed", &provider("declared-model"))
            .unwrap_err()
    ));
    assert_eq!(counters(dir.path()), before);
}
