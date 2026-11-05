//! End-to-end light/heavy node delegation over real TCP sockets.
//!
//! The failure matrix from the light/heavy design doc, run for real:
//! a light node (continuity) delegates typed tasks to a heavy node
//! (cognition) or a scripted fixture standing in for one, survives its
//! loss, and reconciles when it returns.

use std::net::TcpListener;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use kamimusuhi_resident::client;
use kamimusuhi_resident::config::Config;
use kamimusuhi_resident::delegation;
use kamimusuhi_resident::server;
use kamimusuhi_resident::spool::Spool;
use kamimusuhi_resident::state::Shared;
use kamimusuhi_testkit::http_fixture::{FixtureResponse, FixtureServer};
use serde_json::{Value, json};

const SOON: Duration = Duration::from_secs(15);

fn raw(status: u16, body: &str) -> FixtureResponse {
    FixtureResponse::RawHttp {
        response: format!(
            "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    }
}

fn ack(task_id: &str) -> FixtureResponse {
    raw(
        202,
        &json!({"task_id": task_id, "type": "llm.generate", "state": "running",
                "attempts": 0, "created_at": 1, "updated_at": 1})
        .to_string(),
    )
}

fn done(task_id: &str, result: &Value) -> FixtureResponse {
    raw(
        200,
        &json!({"task_id": task_id, "type": "llm.generate", "state": "completed",
                "result": result, "attempts": 1, "created_at": 1, "updated_at": 1})
        .to_string(),
    )
}

fn shared_node(dir: &std::path::Path, config_json: Value) -> Arc<Shared> {
    let config: Config = serde_json::from_value(config_json).expect("config parses");
    config.validate().expect("config valid");
    let spool = Spool::new(dir.join("spool"), &config.node.id).expect("spool");
    Arc::new(Shared::new(config, spool, 1, None))
}

/// A real node on an ephemeral port; the accept loop keeps running for the
/// life of the test process.
fn serve(shared: &Arc<Shared>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("port").port();
    let shared = Arc::clone(shared);
    thread::spawn(move || {
        let _ = server::serve_listener(&shared, listener);
    });
    format!("http://127.0.0.1:{port}")
}

fn mark_peer(shared: &Shared, id: &str, caps: &[&str], healthy: bool) {
    let mut peers = shared.peers.write().unwrap_or_else(|p| p.into_inner());
    let probe = peers.get_mut(id).expect("peer");
    if healthy {
        let caps: Vec<Value> = caps.iter().map(|c| json!(c)).collect();
        probe.record(Ok(json!({"capabilities": caps})), 1);
    } else {
        probe.record(Err("unreachable".to_owned()), 1);
    }
}

/// Poll `GET /task/<id>` until terminal or the deadline.
fn wait_terminal(url: &str, task_id: &str) -> Value {
    let deadline = Instant::now() + SOON;
    loop {
        let record = client::task_record(url, None, task_id).expect("record");
        let state = record["state"].as_str().unwrap_or("");
        if ["completed", "failed", "timeout", "cancelled"].contains(&state) {
            return record;
        }
        assert!(Instant::now() < deadline, "task never settled: {record}");
        thread::sleep(Duration::from_millis(50));
    }
}

fn light_config(dir: &std::path::Path, heavy_url: &str, heavy_caps: &[&str]) -> Value {
    let caps: Vec<Value> = heavy_caps.iter().map(|c| json!(c)).collect();
    json!({
        "node": {"id": "pi-test", "role": "continuity", "listen": "127.0.0.1:0"},
        "paths": {"root": dir},
        "peers": [{"id": "heavy", "role": "cognition", "url": heavy_url,
                   "capabilities": caps, "priority": 100}],
        "delegation": {"poll_ms": 50, "retry_secs": 0, "http_timeout_secs": 2,
                       "poll_error_limit": 3, "workers": 1},
    })
}

fn heavy_config(dir: &std::path::Path, tier_url: &str) -> Value {
    json!({
        "node": {"id": "heavy", "role": "cognition", "listen": "127.0.0.1:0"},
        "paths": {"root": dir},
        "tiers": [{"name": "local", "base_url": tier_url, "model": "fixture-m",
                   "billing": "local", "node_local": true, "timeout_secs": 5,
                   "probe_interval_secs": 60}],
        "delegation": {"poll_ms": 50, "retry_secs": 0, "http_timeout_secs": 2,
                       "workers": 1},
    })
}

// ── Test A: a light node alone stays alive and still does local work ────

#[test]
fn light_only_node_runs_local_tasks_and_rejects_unservable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let light = shared_node(
        dir.path(),
        json!({
            "node": {"id": "pi-test", "role": "continuity", "listen": "127.0.0.1:0"},
            "paths": {"root": dir.path()},
            "delegation": {"poll_ms": 50, "workers": 1},
        }),
    );
    delegation::spawn(&light);
    let url = serve(&light);

    // Local type executes on the node itself, synchronously visible.
    let reply = client::submit_task(&url, None, &json!({"type": "node.ping", "wait_ms": 10_000}))
        .expect("submit");
    assert_eq!(reply["state"], json!("completed"), "{reply}");
    assert_eq!(reply["result"]["node"], json!("pi-test"));

    // No node can ever serve this: reject rather than park.
    let reply = client::submit_task(
        &url,
        None,
        &json!({"type": "llm.generate", "requires": ["llm.generate"]}),
    );
    assert!(reply.is_err(), "unservable task must be refused");

    // The node itself is still healthy and reports light-mode state.
    let health = kamimusuhi_resident::probes::get_json(
        &url,
        "/health",
        &client::auth(None),
        Duration::from_secs(5),
    )
    .expect("health");
    assert_eq!(health["mode"], json!("light"));
    assert!(
        health["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("node.ping"))
    );
}

// ── Test B: Pi → Heavy delegation completes end to end ──────────────────

#[test]
fn llm_generate_delegates_to_a_capable_peer_and_returns() {
    // The heavy node's local LLM backend is a scripted completion server.
    let llm = FixtureServer::start(vec![FixtureResponse::ok("考えた結果")]).expect("llm");
    let heavy_dir = tempfile::tempdir().expect("heavy dir");
    let heavy = shared_node(
        heavy_dir.path(),
        heavy_config(heavy_dir.path(), &llm.base_url()),
    );
    delegation::spawn(&heavy);
    let heavy_url = serve(&heavy);

    let light_dir = tempfile::tempdir().expect("light dir");
    let light = shared_node(
        light_dir.path(),
        light_config(light_dir.path(), &heavy_url, &["llm.generate"]),
    );
    delegation::spawn(&light);
    let url = serve(&light);
    mark_peer(&light, "heavy", &["llm.generate"], true);

    let reply = client::submit_task(
        &url,
        None,
        &json!({"task_id": "nt-e2e-1", "type": "llm.generate",
                "requires": ["llm.generate"],
                "payload": {"messages": [{"role": "user", "content": "hi"}]},
                "timeout_ms": 30_000}),
    )
    .expect("submit");
    assert_eq!(reply["task_id"], json!("nt-e2e-1"));
    assert!(!reply["state"].as_str().unwrap().starts_with("failed"));

    let record = wait_terminal(&url, "nt-e2e-1");
    assert_eq!(record["state"], json!("completed"), "{record}");
    assert_eq!(record["node"], json!("heavy"));
    assert_eq!(
        record["result"].pointer("/choices/0/message/content"),
        Some(&json!("考えた結果")),
        "{record}"
    );

    // Idempotent: resubmitting the same id returns the record, not a rerun.
    let again = client::submit_task(
        &url,
        None,
        &json!({"task_id": "nt-e2e-1", "type": "llm.generate",
                "requires": ["llm.generate"], "payload": {}}),
    )
    .expect("resubmit");
    assert_eq!(again["deduplicated"], json!(true));
    assert_eq!(again["state"], json!("completed"));
}

// ── Test C: heavy dies mid-flight — the light node survives ─────────────

#[test]
fn heavy_dying_mid_poll_parks_the_task_and_light_stays_up() {
    // A stub that acknowledges the task, then hangs up on every poll.
    let stub = FixtureServer::start(vec![
        ack("nt-dead-1"),
        FixtureResponse::Hangup,
        FixtureResponse::Hangup,
        FixtureResponse::Hangup,
        FixtureResponse::Hangup,
        FixtureResponse::Hangup,
    ])
    .expect("stub");
    let heavy_url = format!("http://127.0.0.1:{}", stub.port());

    let dir = tempfile::tempdir().expect("tempdir");
    // retry_secs=2 keeps the parked state observable between attempts.
    let mut config = light_config(dir.path(), &heavy_url, &["llm.generate"]);
    config["delegation"]["retry_secs"] = json!(2);
    let light = shared_node(dir.path(), config);
    delegation::spawn(&light);
    let url = serve(&light);
    mark_peer(&light, "heavy", &["llm.generate"], true);

    client::submit_task(
        &url,
        None,
        &json!({"task_id": "nt-dead-1", "type": "llm.generate",
                "requires": ["llm.generate"], "payload": {"messages": []},
                "timeout_ms": 60_000}),
    )
    .expect("submit");

    // Poll failures push it back to retry_pending (bounded retries then
    // turn it into `failed` if the peer stays dead — both are correct;
    // what must never happen is an infinite wait or a wedged node).
    let deadline = Instant::now() + SOON;
    let record = loop {
        let record = client::task_record(&url, None, "nt-dead-1").expect("record");
        let state = record["state"].as_str().unwrap_or("");
        if ["retry_pending", "failed"].contains(&state) {
            break record;
        }
        assert!(Instant::now() < deadline, "never parked: {record}");
        thread::sleep(Duration::from_millis(50));
    };
    assert!(
        ["retry_pending", "failed"].contains(&record["state"].as_str().unwrap_or("")),
        "{record}"
    );

    // The light node's event loop and HTTP surface are alive.
    let health = kamimusuhi_resident::probes::get_json(
        &url,
        "/health",
        &client::auth(None),
        Duration::from_secs(5),
    )
    .expect("light survives");
    assert_eq!(health["node"], json!("pi-test"));
}

// ── Tests D+E: restart restores the queue; recovery completes the task ──

#[test]
fn restart_restores_pending_tasks_and_recovery_completes_them() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Peer is configured-capable but unreachable: nothing is listening yet.
    let config = light_config(dir.path(), "http://127.0.0.1:1", &["llm.generate"]);
    let light = shared_node(dir.path(), config);
    // No workers yet — this Shared only persists the submission.
    let reply = delegation::submit(
        &light,
        &json!({"task_id": "nt-rst-1", "type": "llm.generate",
                "requires": ["llm.generate"], "payload": {"messages": []},
                "timeout_ms": 300_000}),
    );
    assert_eq!(reply.0, 202, "{reply:?}");
    assert_eq!(light.delegations.pending_count(), 1);
    drop(light);

    // "Restart": a fresh Shared over the same state dir. The parked task
    // is still pending — the queue is durable, not in-memory.
    let config = light_config(dir.path(), "http://127.0.0.1:1", &["llm.generate"]);
    let light = shared_node(dir.path(), config);
    let restored = light
        .delegations
        .get("nt-rst-1")
        .expect("task survived restart");
    assert!(
        ["queued", "retry_pending"].contains(&restored.state.as_str()),
        "{restored:?}"
    );

    // Heavy comes up (a scripted stub on a real port): submit-ack, then a
    // completed record on the first poll.
    let stub = FixtureServer::start(vec![
        ack("nt-rst-1"),
        done(
            "nt-rst-1",
            &json!({"choices": [{"message": {"content": "ok"}}]}),
        ),
    ])
    .expect("stub");
    light
        .peer_urls
        .write()
        .unwrap_or_else(|p| p.into_inner())
        .insert("heavy".into(), format!("http://127.0.0.1:{}", stub.port()));
    mark_peer(&light, "heavy", &["llm.generate"], true);
    delegation::spawn(&light);

    let done = light
        .delegations
        .wait_terminal("nt-rst-1", SOON)
        .expect("task");
    assert_eq!(done.state.as_str(), "completed", "{done:?}");
    assert_eq!(done.node.as_deref(), Some("heavy"));
}
