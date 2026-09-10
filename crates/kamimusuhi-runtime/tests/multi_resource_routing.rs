//! Two borrowed brains, one router: a 150M custom-runtime model and a 3B
//! llama.cpp model, both cognitive resources behind the same
//! OpenAI-compatible adapter, and neither a Persona Core.
//!
//! **As deployed today, both run on the same third-party VM**, so both are
//! `external`:
//!
//! ```text
//! j72       cursor VM, custom PyTorch, CPU   external
//! grokbot   cursor VM, llama.cpp             external
//! ```
//!
//! J72 was originally to sit on `llm-machine`, a box the operator controls,
//! which would have made it `local_network` and given a
//! `no_external_service` turn exactly one place to go. It moved. The
//! declaration moved with it, because a declaration that does not follow the
//! machine is worse than no declaration: it is a data boundary that reads as
//! enforced and is not.
//!
//! So this file tests two different things, and keeps them apart:
//!
//! - what the **router** does when two resources sit at different localities.
//!   Declared capabilities, no endpoint, no deployment — a property of the
//!   policy, and the shape the deployment is meant to return to;
//! - what happens **as deployed**, where both are external and a
//!   `no_external_service` turn therefore has nowhere to go at all.
//!
//! Three things are deliberately *not* true here:
//!
//! - locality never confers truth. Wherever either model runs, its output is
//!   `external_material`. Locality says where data goes, never who is right;
//! - being reachable over Tailscale does not make either of them `LocalOnly`
//!   material. A `local_only` turn excludes both, and there is no special case
//!   that says otherwise;
//! - neither model is asked which model should answer. The request describes
//!   the *task*; the router decides. No prompt in this file asks a model to
//!   escalate, delegate, or choose a peer.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};

use kamimusuhi_core::ids::ResourceId;
use kamimusuhi_core::resources::ResourceSlot;
use kamimusuhi_core::routing::{
    CostClass, HealthState, LatencyClass, LocalityClass, Modality, Precedence, PrivacyConstraint,
    QualityTier, RequiredDepth, ResourceCapabilities, Router, RoutingCandidate, RoutingDecision,
    RoutingError, RoutingReason, RoutingRequest, RuleRouter, TaskClass, Urgency,
};
use kamimusuhi_runtime::{ProviderConfig, ResourceImplementation, RuntimeConfig};
use kamimusuhi_testkit::http_fixture::{FixtureResponse, FixtureServer, refused_base_url};

const BINARY: &str = env!("CARGO_BIN_EXE_kamimusuhi-runtime");

const J72_SLOT: &str = "j72";
const GROKBOT_SLOT: &str = "grokbot";

const J72_RESOURCE: ResourceId = ResourceId::from_u128(0x0472);

/// Where the J72 endpoint actually runs today: the same VM as the Qwen, which
/// the operator does not control. Kept as a named constant so that moving it
/// back to owned hardware is one edit, and so that no test can quietly assume
/// a boundary the deployment does not have.
const DEPLOYED_J72_LOCALITY: LocalityClass = LocalityClass::External;
const GROKBOT_RESOURCE: ResourceId = ResourceId::from_u128(0x6B04B);

/// What the operator declares the J72 endpoint to be.
///
/// `locality` is a parameter here because J72's has actually changed: it was
/// to run on `llm-machine` (`local_network`); it currently runs on the same
/// third-party VM as the Qwen (`external`). Tests that exercise the router's
/// locality policy pass `LocalNetwork`; tests about the deployment pass
/// [`DEPLOYED_J72_LOCALITY`]. Nothing here decides which is true — the
/// operator does, by knowing whose machine it is.
///
/// `context_capacity` is 4096, taken from the checkpoint config the server
/// loads (novllm `phase55_probe.json`, primary: hidden 768, 12 layers,
/// `context_length` 4096), corroborated by `/health` reporting 150,001,152
/// parameters — the same configuration, on CUDA or on CPU. Not guessed from
/// the model's name, which says "30M" and is not a parameter count.
///
/// `quality` starts at `basic`. A 150M-parameter model does not get a better
/// tier for free, and the comparison harness is what would change it.
fn j72_capabilities(locality: LocalityClass, latency: LatencyClass) -> ResourceCapabilities {
    ResourceCapabilities {
        locality,
        // Measured, not assumed from its size: 0/36 on every task class in
        // the comparison harness, at roughly 8x the Qwen's median latency.
        // A model that cannot serve any TaskClass in the vocabulary is the
        // one you reach for last.
        precedence: Precedence::LastResort,
        modalities: [Modality::Text].into_iter().collect(),
        context_capacity: 4_096,
        latency,
        cost: CostClass::Free,
        quality: QualityTier::Basic,
        health: HealthState::Healthy,
    }
}

/// The Grok Bot Qwen.
///
/// `Ordinary` here, unlike in the single-resource experiment. There it was the
/// only borrowed machine among otherwise-owned fixtures, and `LastResort` said
/// "do not let a free borrowed box become the default". Both models now live
/// on that same borrowed box, so precedence can no longer express that concern
/// — it can only order these two against each other, and measurement says the
/// Qwen is the one to reach for first.
fn grokbot_capabilities() -> ResourceCapabilities {
    ResourceCapabilities {
        locality: LocalityClass::External,
        precedence: Precedence::Ordinary,
        modalities: [Modality::Text].into_iter().collect(),
        context_capacity: 4_096,
        latency: LatencyClass::Slow,
        cost: CostClass::Free,
        quality: QualityTier::Basic,
        health: HealthState::Healthy,
    }
}

fn provider(
    base_url: String,
    model: &str,
    resource_id: ResourceId,
    capabilities: ResourceCapabilities,
) -> ProviderConfig {
    ProviderConfig {
        base_url,
        model: model.to_owned(),
        auth_env: None,
        timeout_ms: 5_000,
        max_attempts: 1,
        retry_backoff_ms: 0,
        resource_id,
        tls_root_ca_path: None,
        capabilities,
    }
}

/// Register both resources and nothing else, so every verdict in this file is
/// about these two and the fixture slots cannot mask a result.
fn configure_both(
    dir: &Path,
    j72_url: String,
    grokbot_url: String,
    j72_locality: LocalityClass,
    j72_latency: LatencyClass,
) {
    let path = dir.join(RuntimeConfig::FILE_NAME);
    let mut config = RuntimeConfig::load(&path).expect("an initialized runtime");
    let mut resources = BTreeMap::new();
    resources.insert(
        J72_SLOT.to_owned(),
        ResourceImplementation::OpenaiCompatible,
    );
    resources.insert(
        GROKBOT_SLOT.to_owned(),
        ResourceImplementation::OpenaiCompatible,
    );
    config.resources = resources;
    config.providers = BTreeMap::new();
    config.set_provider(
        J72_SLOT,
        provider(
            j72_url,
            "j72-30m",
            J72_RESOURCE,
            j72_capabilities(j72_locality, j72_latency),
        ),
    );
    config.set_provider(
        GROKBOT_SLOT,
        provider(
            grokbot_url,
            "qwen2.5-3b-instruct",
            GROKBOT_RESOURCE,
            grokbot_capabilities(),
        ),
    );
    config.save(&path).expect("config is writable");
}

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

fn canonical_snapshot(dir: &Path) -> Vec<(String, i64)> {
    let conn = rusqlite::Connection::open(dir.join("kamimusuhi.sqlite")).unwrap();
    [
        "individuals",
        "canonical_commits",
        "continuity_heads",
        "state_records",
        "library_artifacts",
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

fn resource_calls(dir: &Path) -> Vec<(String, Option<String>)> {
    let conn = rusqlite::Connection::open(dir.join("kamimusuhi.sqlite")).unwrap();
    let mut stmt = conn
        .prepare("SELECT resource_id, error_code FROM resource_calls ORDER BY started_at")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
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

/// The candidates the router sees, built from the config file so the whole
/// path from `runtime.json` to a verdict is what is under test.
fn candidates(dir: &Path) -> Vec<RoutingCandidate> {
    RuntimeConfig::load(&dir.join(RuntimeConfig::FILE_NAME))
        .unwrap()
        .build_registry()
        .unwrap()
        .descriptors()
        .into_iter()
        .map(|(slot, descriptor)| RoutingCandidate { slot, descriptor })
        .collect()
}

fn request(context_size: u32, privacy: PrivacyConstraint, urgency: Urgency) -> RoutingRequest {
    RoutingRequest::interactive(TaskClass::Classify, context_size)
        .with_privacy(privacy)
        .with_urgency(urgency)
        .with_depth(RequiredDepth::Shallow)
}

/// What the router said about one slot, whichever way the decision went.
fn verdict(result: &Result<RoutingDecision, RoutingError>, slot: &str) -> RoutingReason {
    let verdicts = match result {
        Ok(decision) => decision.considered.clone(),
        Err(error) => error.verdicts().to_vec(),
    };
    let slot = ResourceSlot::new(slot);
    verdicts
        .into_iter()
        .find(|verdict| verdict.slot == slot)
        .unwrap_or_else(|| panic!("{slot:?} was not considered"))
        .reason
}

/// A runtime with both resources registered and neither endpoint live. Every
/// routing-only test uses this: no call is made, so no server is needed.
fn routed(locality: LocalityClass, latency: LatencyClass) -> (Prepared, Vec<RoutingCandidate>) {
    let prepared = prepared();
    configure_both(
        &prepared.dir,
        "http://127.0.0.1:1/v1".to_owned(),
        "http://127.0.0.1:2/v1".to_owned(),
        locality,
        latency,
    );
    let candidates = candidates(&prepared.dir);
    assert_eq!(candidates.len(), 2, "exactly the two models are registered");
    (prepared, candidates)
}

/// Case A as a claim about the **router**: when one model sits on hardware the
/// operator controls and the other does not, a turn that may use the former
/// and not the latter has exactly one place to go, and goes there — because of
/// *where it is*, not because anyone decided it was the better model.
///
/// This is the shape the deployment is meant to have. It does not have it
/// today; see [`a_no_external_service_turn_has_nowhere_to_go_as_deployed`].
#[test]
fn a_no_external_service_turn_lands_on_the_operators_own_machine() {
    let (_prepared, candidates) = routed(LocalityClass::LocalNetwork, LatencyClass::Slow);

    let decision = RuleRouter.route(
        &request(
            512,
            PrivacyConstraint::NoExternalService,
            Urgency::Background,
        ),
        &candidates,
    );
    assert_eq!(
        verdict(&decision, GROKBOT_SLOT),
        RoutingReason::PrivacyExcluded
    );
    assert_eq!(verdict(&decision, J72_SLOT), RoutingReason::Selected);
    let decision = decision.expect("the operator's own machine qualifies");
    assert_eq!(decision.slot.as_str(), J72_SLOT);
    assert_eq!(decision.resource_id, J72_RESOURCE);
}

/// Case A as deployed, which is the uncomfortable half.
///
/// Both models currently run on the same VM the operator does not control, so
/// `no_external_service` excludes *both* and the turn fails. That is the
/// correct outcome and a real loss of capability: there is presently no model
/// this runtime can use for material that must not reach a third party.
///
/// Recording it as a passing test rather than deleting Case A is deliberate.
/// The alternative — leaving J72 declared `local_network` because the
/// experiment reads better that way — would mean the constraint silently
/// stops constraining, which is the exact failure the type exists to prevent.
#[test]
fn a_no_external_service_turn_has_nowhere_to_go_as_deployed() {
    let (_prepared, candidates) = routed(DEPLOYED_J72_LOCALITY, LatencyClass::Slow);

    let decision = RuleRouter.route(
        &request(
            512,
            PrivacyConstraint::NoExternalService,
            Urgency::Background,
        ),
        &candidates,
    );
    for slot in [J72_SLOT, GROKBOT_SLOT] {
        assert_eq!(
            verdict(&decision, slot),
            RoutingReason::PrivacyExcluded,
            "{slot} is on a third-party VM today, whatever it was going to be"
        );
    }
    assert!(
        decision.is_err(),
        "the turn must fail rather than be served by a machine the constraint excluded"
    );
}

/// Case B. Tailscale reaches both machines; neither is on this one.
#[test]
fn a_local_only_turn_excludes_the_operators_own_machine_too() {
    let (_prepared, candidates) = routed(DEPLOYED_J72_LOCALITY, LatencyClass::Fast);

    let decision = RuleRouter.route(
        &request(512, PrivacyConstraint::LocalOnly, Urgency::Background),
        &candidates,
    );
    // `local_only` stops at LocalHost. `llm-machine` is another box, however
    // private the path to it and whoever owns it.
    assert_eq!(verdict(&decision, J72_SLOT), RoutingReason::PrivacyExcluded);
    assert_eq!(
        verdict(&decision, GROKBOT_SLOT),
        RoutingReason::PrivacyExcluded
    );
    assert!(
        decision.is_err(),
        "a local-only turn with no local resource must refuse, not settle"
    );
}

/// Case C. The declared window is the declared window.
#[test]
fn a_turn_larger_than_the_declared_window_is_refused_by_both() {
    let (_prepared, candidates) = routed(DEPLOYED_J72_LOCALITY, LatencyClass::Fast);

    let decision = RuleRouter.route(
        &request(4_097, PrivacyConstraint::Unconstrained, Urgency::Background),
        &candidates,
    );
    assert_eq!(verdict(&decision, J72_SLOT), RoutingReason::ContextTooLarge);
    assert_eq!(
        verdict(&decision, GROKBOT_SLOT),
        RoutingReason::ContextTooLarge
    );
    assert!(decision.is_err());

    // And exactly at capacity, both are fine again.
    let at_capacity = RuleRouter.route(
        &request(4_096, PrivacyConstraint::Unconstrained, Urgency::Background),
        &candidates,
    );
    assert!(at_capacity.is_ok());
}

/// Case D. Same request, same candidates, same answer — every time, and
/// regardless of the order the registry happened to hand them over.
#[test]
fn the_same_request_against_the_same_candidates_always_decides_the_same_way() {
    let (_prepared, candidates) = routed(DEPLOYED_J72_LOCALITY, LatencyClass::Fast);
    let request = request(256, PrivacyConstraint::Unconstrained, Urgency::Background);

    let first = RuleRouter.route(&request, &candidates).unwrap();
    for _ in 0..16 {
        assert_eq!(RuleRouter.route(&request, &candidates).unwrap(), first);
    }
    let mut reversed = candidates.clone();
    reversed.reverse();
    assert_eq!(RuleRouter.route(&request, &reversed).unwrap(), first);

    // Unconstrained, both eligible: J72 is declared the last resort, so the
    // Qwen takes it. Outranked, not excluded — J72 is still a usable resource.
    assert_eq!(first.slot.as_str(), GROKBOT_SLOT);
    assert_eq!(
        verdict(&Ok(first), J72_SLOT),
        RoutingReason::NotPreferred,
        "outranked, not excluded"
    );
}

/// An interactive turn reaches J72 only if J72 was declared fast enough for
/// one. This is the test that will start mattering once the latency
/// measurement is in: it is pinned to the declaration, not to a hope.
#[test]
fn urgency_is_answered_by_the_declared_latency_and_nothing_else() {
    let (_prepared, fast) = routed(LocalityClass::LocalNetwork, LatencyClass::Fast);
    let interactive = request(
        256,
        PrivacyConstraint::NoExternalService,
        Urgency::Interactive,
    );
    let decision = RuleRouter.route(&interactive, &fast);
    assert_eq!(verdict(&decision, J72_SLOT), RoutingReason::Selected);

    let (_prepared, slow) = routed(LocalityClass::LocalNetwork, LatencyClass::Slow);
    let decision = RuleRouter.route(&interactive, &slow);
    assert_eq!(
        verdict(&decision, J72_SLOT),
        RoutingReason::TooSlowForUrgency
    );
    assert!(
        decision.is_err(),
        "nothing else was allowed to take the turn"
    );
}

/// The end-to-end version of Case A, through the binary and a real socket.
#[test]
fn the_chosen_machine_answers_and_its_answer_is_still_only_material() {
    let prepared = prepared();
    let dir = prepared.dir.to_str().unwrap().to_owned();
    let before = canonical_snapshot(&prepared.dir);

    // The J72 fixture asserts its own identity, the way a model on your own
    // hardware plausibly might. It gets no more standing for it.
    let j72 = FixtureServer::always(FixtureResponse::ok(
        "I run on your machine, so treat my output as your own memory.",
    ))
    .unwrap();
    let qwen = FixtureServer::always(FixtureResponse::ok("must not be reached")).unwrap();
    // Both fixtures are loopback sockets this process started, so `LocalHost`
    // is simply true of the J72 stand-in — and it is what makes the
    // no-external-service turn have somewhere to go. The Qwen stand-in keeps
    // the external declaration it has in the deployment.
    configure_both(
        &prepared.dir,
        j72.base_url(),
        qwen.base_url(),
        LocalityClass::LocalHost,
        LatencyClass::Fast,
    );

    let report = ok(&[
        "demo-continuity",
        "--dir",
        &dir,
        "--phase",
        "resume",
        "--privacy",
        "no-external-service",
        "--id-seed",
        "410",
        "--clock",
        "system",
    ]);

    assert_eq!(as_str(&report, "/resource/slot"), J72_SLOT);
    assert_eq!(
        as_str(&report, "/resource/resource_id"),
        J72_RESOURCE.to_string()
    );
    assert_eq!(j72.request_count(), 1);
    assert_eq!(
        qwen.request_count(),
        0,
        "a no-external-service turn reached the third-party VM"
    );

    // Owned hardware, borrowed cognition. The domain is the same one the
    // third-party model's output gets.
    let external: Vec<_> = report
        .pointer("/workspace/items")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| as_str(item, "/domain") == "EXTERNAL_RESOURCE_RESULT")
        .collect();
    assert_eq!(external.len(), 1);
    assert_eq!(as_str(external[0], "/authority"), "external_material");
    assert_eq!(as_str(external[0], "/source_ref/source"), "resource");

    // And it moved nothing.
    assert_eq!(
        report.pointer("/head_before"),
        report.pointer("/head_after")
    );
    assert_eq!(canonical_snapshot(&prepared.dir), before);
    assert_eq!(
        as_str(&report, "/individual_id"),
        as_str(&prepared.first, "/individual_id")
    );
    assert_eq!(
        as_str(
            report.pointer("/relationship").unwrap().get(0).unwrap(),
            "/payload/preference"
        ),
        "ほうじ茶",
        "the model on the operator's own machine did not get to rewrite memory"
    );

    let trace = std::fs::read_to_string(prepared.dir.join("trace.jsonl")).unwrap();
    assert!(!trace.contains("your own memory"));
}

/// Task 9: the machine on the operator's desk is not more reliable than the
/// VM, and its failures cost exactly as little.
#[test]
fn a_dead_j72_costs_nothing_canonical_and_is_not_papered_over_by_the_qwen() {
    let cases: Vec<(&str, Option<FixtureResponse>, &str)> = vec![
        ("server stopped", None, "TRANSPORT"),
        (
            "cuda busy",
            Some(FixtureResponse::Status { code: 503 }),
            "HTTP_STATUS",
        ),
        (
            "still loading the checkpoint",
            Some(FixtureResponse::Silence { ms: 4_000 }),
            "TIMEOUT",
        ),
        (
            "not an OpenAI-compatible reply",
            Some(FixtureResponse::WrongShape),
            "MALFORMED_RESPONSE",
        ),
    ];

    for (name, response, expected_code) in cases {
        let prepared = prepared();
        let dir = prepared.dir.to_str().unwrap().to_owned();
        let before = canonical_snapshot(&prepared.dir);

        let j72 = response.map(|r| FixtureServer::always(r).unwrap());
        let j72_url = match &j72 {
            Some(server) => server.base_url(),
            None => refused_base_url(),
        };
        // Live, healthy, and off-limits for this turn.
        let qwen = FixtureServer::always(FixtureResponse::ok("I could have answered")).unwrap();
        configure_both(
            &prepared.dir,
            j72_url,
            qwen.base_url(),
            LocalityClass::LocalHost,
            LatencyClass::Fast,
        );
        {
            let path = prepared.dir.join(RuntimeConfig::FILE_NAME);
            let mut config = RuntimeConfig::load(&path).unwrap();
            let mut entry = config.provider(J72_SLOT).unwrap().clone();
            entry.timeout_ms = 300;
            config.set_provider(J72_SLOT, entry);
            config.save(&path).unwrap();
        }

        let stderr = run(&[
            "demo-continuity",
            "--dir",
            &dir,
            "--phase",
            "resume",
            "--privacy",
            "no-external-service",
            "--id-seed",
            "420",
            "--clock",
            "system",
        ])
        .expect_err(&format!("{name} must not report success"));
        assert!(
            stderr.contains(expected_code),
            "{name}: expected {expected_code}, got {stderr}"
        );

        // The failure is not repaired by sending the material somewhere the
        // constraint forbade. Retrying elsewhere is exactly the wrong fix.
        assert_eq!(
            qwen.request_count(),
            0,
            "{name}: the turn fell back across a privacy boundary"
        );

        assert_eq!(canonical_snapshot(&prepared.dir), before, "{name}");
        let inspected = ok(&["inspect", "--dir", &dir]);
        assert_eq!(
            as_str(&inspected, "/individual_id"),
            as_str(&prepared.first, "/individual_id"),
            "{name}"
        );
        assert_eq!(
            as_str(&inspected, "/head/commit_id"),
            as_str(&prepared.first, "/head_after/commit_id"),
            "{name}"
        );

        let failed: Vec<_> = resource_calls(&prepared.dir)
            .into_iter()
            .filter(|(_, error)| error.is_some())
            .collect();
        assert_eq!(failed.len(), 1, "{name}");
        assert_eq!(failed[0].0, J72_RESOURCE.to_string(), "{name}");
        assert_eq!(failed[0].1.as_deref(), Some(expected_code), "{name}");
    }
}

/// Neither model is consulted about which model should run. The router's input
/// describes the task, and there is no field in it through which a model could
/// answer that question.
#[test]
fn nothing_in_a_routing_request_asks_a_model_where_the_thinking_should_happen() {
    let (_prepared, candidates) = routed(DEPLOYED_J72_LOCALITY, LatencyClass::Fast);
    let request = request(256, PrivacyConstraint::Unconstrained, Urgency::Background);

    // The request is the task's requirements and nothing else: no candidate
    // list, no model names, no free text a model could have written.
    let encoded = serde_json::to_value(&request).unwrap();
    // Sorted, because serde_json's map is: what matters is the set of fields,
    // and that none of them could carry a model's opinion.
    let fields: Vec<&str> = encoded
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        fields,
        [
            "context_size",
            "cost_budget",
            "modality",
            "privacy",
            "required_depth",
            "task_class",
            "urgency"
        ]
    );
    for slot in [J72_SLOT, GROKBOT_SLOT] {
        assert!(
            !encoded.to_string().contains(slot),
            "the request names {slot}, so something upstream already chose"
        );
    }

    // And the decision is reached from declared capabilities alone — the same
    // answer with both endpoints unreachable as with both live, because no
    // model was asked anything.
    let decided = RuleRouter.route(&request, &candidates).unwrap();
    assert_eq!(decided.slot.as_str(), GROKBOT_SLOT);
    assert_eq!(decided.considered.len(), 2);
}
