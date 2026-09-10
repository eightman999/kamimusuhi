//! W6: the router in the delegation path, across a real process boundary.
//!
//! The claim under test is that routing is a *decision with a reason*, not a
//! preference. A privacy-constrained turn that has nowhere local to go must
//! fail; it must not quietly go somewhere remote because that was the option
//! left. And whichever resource answers, the result is still external material
//! and the individual is still the same individual.
//!
//! Covers the W6 acceptance items for the delegation path, privacy-constrained
//! routing, routing decisions in the trace, and invariance under provider
//! failure and replacement.

use std::path::Path;
use std::process::{Command, Stdio};

use kamimusuhi_core::ids::ResourceId;
use kamimusuhi_core::routing::{
    CostClass, HealthState, LatencyClass, LocalityClass, Modality, Precedence, QualityTier,
    ResourceCapabilities,
};
use kamimusuhi_runtime::config::GENERAL_SLOT;
use kamimusuhi_runtime::{ProviderConfig, ResourceImplementation, RuntimeConfig};
use kamimusuhi_testkit::http_fixture::{FixtureResponse, FixtureServer};

const BINARY: &str = env!("CARGO_BIN_EXE_kamimusuhi-runtime");
const PROVIDER: ResourceId = ResourceId::from_u128(0x0B01);

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

fn canonical_snapshot(dir: &Path) -> Vec<(String, i64)> {
    let conn = rusqlite::Connection::open(dir.join("kamimusuhi.sqlite")).unwrap();
    [
        "individuals",
        "canonical_commits",
        "continuity_heads",
        "state_records",
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

fn capabilities(locality: LocalityClass, cost: CostClass) -> ResourceCapabilities {
    ResourceCapabilities {
        locality,
        precedence: Precedence::Ordinary,
        modalities: [Modality::Text].into_iter().collect(),
        context_capacity: 8_192,
        latency: LatencyClass::Fast,
        cost,
        quality: QualityTier::Standard,
        health: HealthState::Healthy,
    }
}

fn provider(server: &FixtureServer, capabilities: ResourceCapabilities) -> ProviderConfig {
    ProviderConfig {
        base_url: server.base_url(),
        model: "fixture-model".to_owned(),
        auth_env: None,
        timeout_ms: 2_000,
        max_attempts: 1,
        retry_backoff_ms: 1,
        resource_id: PROVIDER,
        tls_root_ca_path: None,
        capabilities,
    }
}

fn configure_provider(dir: &Path, provider: ProviderConfig) {
    let path = dir.join(RuntimeConfig::FILE_NAME);
    let mut config = RuntimeConfig::load(&path).expect("an initialized runtime");
    config.set_implementation(GENERAL_SLOT, ResourceImplementation::OpenaiCompatible);
    config.set_provider(GENERAL_SLOT, provider);
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
fn the_delegation_path_runs_through_the_router_and_records_its_decision() {
    let prepared = prepared();
    let dir = prepared.dir.to_str().unwrap().to_owned();
    let server = FixtureServer::always(FixtureResponse::ok("routed answer")).unwrap();
    configure_provider(
        &prepared.dir,
        provider(
            &server,
            capabilities(LocalityClass::External, CostClass::Low),
        ),
    );

    let report = resume(&dir, "30").expect("the routed call succeeds");

    // The router chose, and the resource that answered is the one it chose.
    assert_eq!(
        as_str(&report, "/resource/routing_decision/reason"),
        "SELECTED"
    );
    assert_eq!(
        as_str(&report, "/resource/routing_decision/slot"),
        GENERAL_SLOT
    );
    assert_eq!(
        as_str(&report, "/resource/routing_decision/resource_id"),
        as_str(&report, "/resource/resource_id"),
        "the call must be attributed to the resource routing picked"
    );
    assert_eq!(as_str(&report, "/resource/answer"), "routed answer");

    // The requirement is part of the record, not implicit in the code.
    assert_eq!(
        as_str(&report, "/resource/routing_request/task_class"),
        "summarize"
    );
    assert_eq!(
        as_str(&report, "/resource/routing_request/privacy"),
        "unconstrained"
    );
    assert_eq!(
        as_str(&report, "/resource/routing_request/urgency"),
        "interactive"
    );

    // Routing changed which resource answered, not what kind of thing an
    // answer is: it is still external material in the workspace.
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

    // And the decision is in the trace, correlated to the resource and turn.
    // The trace file spans every invocation against this directory, so scope
    // to this turn rather than assuming the file holds only one decision.
    let events = trace_events(&prepared.dir);
    let turn = as_str(&report, "/turn_id");
    let decided: Vec<_> = events
        .iter()
        .filter(|e| as_str(e, "/event_kind") == "routing.decided")
        .filter(|e| {
            e.pointer("/correlation/turn_id")
                .and_then(serde_json::Value::as_str)
                == Some(turn.as_str())
        })
        .collect();
    assert_eq!(decided.len(), 1, "one decision for this turn");
    assert_eq!(as_str(decided[0], "/detail/outcome"), "selected");
    assert_eq!(as_str(decided[0], "/detail/reason"), "SELECTED");
    assert_eq!(
        as_str(decided[0], "/correlation/resource_id"),
        as_str(&report, "/resource/resource_id")
    );
    assert_eq!(
        as_str(decided[0], "/correlation/turn_id"),
        as_str(&report, "/turn_id")
    );
    // Every candidate is accounted for, with a reason.
    let considered = decided[0]
        .pointer("/detail/considered")
        .unwrap()
        .as_array()
        .unwrap();
    assert!(!considered.is_empty());
    assert!(considered.iter().all(|c| c.get("reason").is_some()));
}

#[test]
fn a_local_only_turn_refuses_rather_than_reaching_an_external_provider() {
    let prepared = prepared();
    let dir = prepared.dir.to_str().unwrap().to_owned();
    let before = canonical_snapshot(&prepared.dir);

    // The only resource is external, and the demo asks for a local-only turn.
    let server = FixtureServer::always(FixtureResponse::ok("must not be reached")).unwrap();
    configure_provider(
        &prepared.dir,
        provider(
            &server,
            capabilities(LocalityClass::External, CostClass::Low),
        ),
    );

    let stderr = run(&[
        "demo-continuity",
        "--dir",
        &dir,
        "--phase",
        "resume",
        "--id-seed",
        "31",
        "--clock",
        "system",
        "--privacy",
        "local-only",
    ])
    .expect_err("a local-only turn must not be served by an external resource");

    assert!(stderr.contains("routing refused"), "{stderr}");
    // The decisive fact: nothing was sent.
    assert_eq!(
        server.request_count(),
        0,
        "a privacy refusal must happen before any request leaves the process"
    );

    // Refusing changed nothing canonical, and did not mint anything.
    assert_eq!(canonical_snapshot(&prepared.dir), before);
    let inspected = ok(&["inspect", "--dir", &dir]);
    assert_eq!(
        as_str(&inspected, "/individual_id"),
        as_str(&prepared.first, "/individual_id")
    );
    assert_eq!(
        as_str(&inspected, "/head/commit_id"),
        as_str(&prepared.first, "/head_after/commit_id")
    );

    // The refusal and its reason are visible.
    let events = trace_events(&prepared.dir);
    let refused: Vec<_> = events
        .iter()
        .filter(|e| as_str(e, "/event_kind") == "routing.decided")
        .filter(|e| as_str(e, "/detail/outcome") == "refused")
        .collect();
    assert_eq!(refused.len(), 1);
    let considered = refused[0]
        .pointer("/detail/considered")
        .unwrap()
        .as_array()
        .unwrap();
    assert!(
        considered
            .iter()
            .any(|c| as_str(c, "/reason") == "PRIVACY_EXCLUDED"),
        "the refusal must say it was about privacy: {considered:?}"
    );
}

#[test]
fn a_local_only_turn_is_served_when_a_local_resource_qualifies() {
    let prepared = prepared();
    let dir = prepared.dir.to_str().unwrap().to_owned();

    // The fixture resource is in-process, so a local-only turn has somewhere
    // to go and must not be refused.
    let report = run(&[
        "demo-continuity",
        "--dir",
        &dir,
        "--phase",
        "resume",
        "--seed",
        "32",
        "--resource",
        "fake-a",
        "--privacy",
        "local-only",
    ])
    .expect("an in-process resource satisfies local-only");

    assert_eq!(
        as_str(&report, "/resource/routing_request/privacy"),
        "local_only"
    );
    assert_eq!(
        as_str(&report, "/resource/routing_decision/reason"),
        "SELECTED"
    );
    assert_eq!(as_str(&report, "/resource/answer"), "result-a");
}

#[test]
fn an_unhealthy_declared_resource_is_excluded_before_it_is_called() {
    let prepared = prepared();
    let dir = prepared.dir.to_str().unwrap().to_owned();

    // `fake-unavailable` declares itself unavailable. The router must exclude
    // it on the declaration rather than discovering it by failing a call.
    let stderr = run(&[
        "demo-continuity",
        "--dir",
        &dir,
        "--phase",
        "resume",
        "--seed",
        "33",
        "--resource",
        "fake-unavailable",
    ])
    .expect_err("an unavailable resource cannot serve a turn");
    assert!(stderr.contains("routing refused"), "{stderr}");

    let refused = trace_events(&prepared.dir)
        .into_iter()
        .rfind(|e| {
            as_str(e, "/event_kind") == "routing.decided"
                && as_str(e, "/detail/outcome") == "refused"
        })
        .expect("a refusal was recorded");
    let considered = refused
        .pointer("/detail/considered")
        .unwrap()
        .as_array()
        .unwrap();
    assert!(
        considered
            .iter()
            .any(|c| as_str(c, "/reason") == "UNHEALTHY"),
        "{considered:?}"
    );

    // No call was attempted, so there is no failed call to attribute.
    let conn = rusqlite::Connection::open(prepared.dir.join("kamimusuhi.sqlite")).unwrap();
    let calls: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM resource_calls WHERE error_code IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(calls, 0, "an excluded resource must not be invoked");
}

#[test]
fn routing_is_reproducible_across_processes() {
    let first = prepared();
    let second = prepared();
    let a = resume(first.dir.to_str().unwrap(), "40").unwrap();
    let b = resume(second.dir.to_str().unwrap(), "40").unwrap();

    // Same request, same candidates, same decision — in two different
    // processes, on a real clock.
    assert_eq!(
        a.pointer("/resource/routing_request"),
        b.pointer("/resource/routing_request")
    );
    assert_eq!(
        a.pointer("/resource/routing_decision"),
        b.pointer("/resource/routing_decision")
    );
}

#[test]
fn a_provider_failure_behind_the_router_still_leaves_the_individual_alone() {
    let prepared = prepared();
    let dir = prepared.dir.to_str().unwrap().to_owned();
    let before = canonical_snapshot(&prepared.dir);

    // Routing succeeds; the call then fails. The router being in the path must
    // not change what a provider failure can touch.
    let server = FixtureServer::always(FixtureResponse::Status { code: 500 }).unwrap();
    configure_provider(
        &prepared.dir,
        provider(
            &server,
            capabilities(LocalityClass::External, CostClass::Low),
        ),
    );

    let stderr = resume(&dir, "41").expect_err("a 500 is not a result");
    assert!(stderr.contains("HTTP_STATUS"), "{stderr}");

    assert_eq!(canonical_snapshot(&prepared.dir), before);
    let inspected = ok(&["inspect", "--dir", &dir]);
    assert_eq!(
        as_str(&inspected, "/individual_id"),
        as_str(&prepared.first, "/individual_id")
    );
    assert_eq!(
        as_str(&inspected, "/head/commit_id"),
        as_str(&prepared.first, "/head_after/commit_id")
    );

    // The routing decision was still recorded, and so was the failed call.
    let events = trace_events(&prepared.dir);
    assert!(
        events
            .iter()
            .any(|e| as_str(e, "/event_kind") == "routing.decided"
                && as_str(e, "/detail/outcome") == "selected")
    );
    let conn = rusqlite::Connection::open(prepared.dir.join("kamimusuhi.sqlite")).unwrap();
    let code: String = conn
        .query_row(
            "SELECT error_code FROM resource_calls WHERE error_code IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(code, "HTTP_STATUS");
}

#[test]
fn replacing_the_provider_changes_no_self_or_memory_schema() {
    let prepared = prepared();
    let dir = prepared.dir.to_str().unwrap().to_owned();

    let schema_before = table_schema(&prepared.dir);
    let server = FixtureServer::always(FixtureResponse::ok("after replacement")).unwrap();
    configure_provider(
        &prepared.dir,
        provider(
            &server,
            capabilities(LocalityClass::External, CostClass::Low),
        ),
    );
    resume(&dir, "42").unwrap();

    // Config rewriting and provider use touch no schema: the individual's
    // state is stored the same way whoever is doing the thinking.
    assert_eq!(table_schema(&prepared.dir), schema_before);
    let inspected = ok(&["inspect", "--dir", &dir]);
    assert_eq!(
        as_str(&inspected, "/individual_id"),
        as_str(&prepared.first, "/individual_id")
    );
    assert_eq!(
        as_str(&inspected, "/head/commit_id"),
        as_str(&prepared.first, "/head_after/commit_id")
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
