//! Background loops: health probes, spool→NAS sync, heartbeat and snapshots.
//!
//! Each loop owns one thread. A loop that blocks (a hung NFS mount, a peer
//! that accepts TCP but never answers) delays only itself.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use kamimusuhi_resource_http::{Endpoint, Header, HttpError, TrustAnchors, http};
use serde_json::{Value, json};

use crate::config::{NodeRole, TierConfig};
use crate::state::Shared;
use crate::util::{
    atomic_write, call_with_timeout, file_stamp, iso8601, run_with_timeout, unix_now,
};

pub const ROUTE_HEADER: &str = "X-Kamimusuhi-Route";

/// Resolve a credential by environment variable name. Never logs the value.
pub fn bearer(auth_env: Option<&str>) -> Result<Vec<Header>, String> {
    let Some(name) = auth_env else {
        return Ok(Vec::new());
    };
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => Ok(vec![Header {
            name: "Authorization".to_owned(),
            value: format!("Bearer {}", value.trim()),
        }]),
        _ => Err(format!("credential env {name} is not set")),
    }
}

pub fn describe(error: &HttpError) -> String {
    match error {
        HttpError::InvalidRequest(m) => format!("invalid request: {m}"),
        HttpError::Timeout { elapsed_ms, phase } => {
            format!("timeout in {phase} after {elapsed_ms}ms")
        }
        HttpError::Transport(m) => format!("transport: {m}"),
        HttpError::Malformed(m) => format!("malformed response: {m}"),
        HttpError::Tls { kind, detail } => format!("tls {kind}: {detail}"),
    }
}

/// GET `base + suffix` and parse JSON. Non-2xx is an error.
pub fn get_json(
    base: &str,
    suffix: &str,
    headers: &[Header],
    timeout: Duration,
) -> Result<Value, String> {
    let endpoint = Endpoint::parse(base, suffix)?;
    let response = http::get_json(&endpoint, headers, timeout, &TrustAnchors::Webpki)
        .map_err(|e| describe(&e))?;
    if !response.is_success() {
        return Err(format!("HTTP {}", response.status));
    }
    serde_json::from_str(&response.body).map_err(|_| "response is not JSON".to_owned())
}

fn timed<T>(f: impl FnOnce() -> T) -> (T, u64) {
    let started = Instant::now();
    let value = f();
    (
        value,
        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    )
}

fn every(name: &str, period: Duration, mut tick: impl FnMut() + Send + 'static) {
    let _ = thread::Builder::new().name(name.to_owned()).spawn(move || {
        loop {
            let started = Instant::now();
            tick();
            thread::sleep(
                period
                    .saturating_sub(started.elapsed())
                    .max(Duration::from_secs(1)),
            );
        }
    });
}

pub fn probe_tier(tier: &TierConfig) -> Result<Value, String> {
    let mut headers = bearer(tier.auth_env.as_deref())?;
    if tier.peer_local_only {
        headers.push(Header {
            name: ROUTE_HEADER.to_owned(),
            value: "local".to_owned(),
        });
    }
    let models = get_json(&tier.base_url, "/models", &headers, Duration::from_secs(6))?;
    let ids: Vec<Value> = models
        .get("data")
        .and_then(Value::as_array)
        .map(|d| d.iter().filter_map(|m| m.get("id").cloned()).collect())
        .unwrap_or_default();
    if ids.is_empty() {
        return Err("backend lists no models".to_owned());
    }
    let has_model = ids
        .iter()
        .any(|id| id.as_str() == Some(tier.model.as_str()));
    Ok(json!({"models": ids.len(), "configured_model_listed": has_model}))
}

pub fn spawn_all(shared: &Arc<Shared>) {
    let config = &shared.config;

    for tier in config.tiers.clone() {
        let s = Arc::clone(shared);
        every(
            &format!("probe-{}", tier.name),
            Duration::from_secs(tier.probe_interval_secs),
            move || {
                let (outcome, ms) = timed(|| probe_tier(&tier));
                s.set_tier(&tier.name, outcome, ms);
            },
        );
    }

    for peer in config.peers.clone() {
        let s = Arc::clone(shared);
        every(
            &format!("peer-{}", peer.id),
            Duration::from_secs(config.intervals.peer_secs),
            move || {
                let (outcome, ms) =
                    timed(|| get_json(&peer.url, "/health", &[], Duration::from_secs(5)));
                let mut peers = s.peers.write().unwrap_or_else(|p| p.into_inner());
                peers
                    .entry(peer.id.clone())
                    .or_default()
                    .record(outcome, ms);
            },
        );
    }

    if let Some(account) = config.hai_account.clone() {
        let s = Arc::clone(shared);
        every(
            "hai-account",
            Duration::from_secs(account.interval_secs),
            move || {
                let (outcome, ms) = timed(|| {
                    let headers = bearer(Some(&account.auth_env))?;
                    let credits = get_json(
                        &account.base_url,
                        "/credits",
                        &headers,
                        Duration::from_secs(10),
                    )?;
                    let balance = credits
                        .get("balanceJpy")
                        .and_then(|b| {
                            b.as_str()
                                .map(str::to_owned)
                                .or_else(|| Some(b.to_string()))
                        })
                        .unwrap_or_default();
                    let numeric = balance.parse::<f64>().ok();
                    let low = numeric.is_some_and(|b| b < account.low_balance_jpy);
                    Ok(json!({
                        "balance_jpy": balance,
                        "held_jpy": credits.get("heldJpy"),
                        "low_balance": low,
                    }))
                });
                let mut slot = s.account.write().unwrap_or_else(|p| p.into_inner());
                slot.get_or_insert_with(Default::default)
                    .record(outcome, ms);
            },
        );
    }

    if config.node.role == NodeRole::Cognition {
        let s = Arc::clone(shared);
        every(
            "gpu",
            Duration::from_secs(config.intervals.gpu_secs),
            move || {
                let (outcome, ms) = timed(gpu_snapshot);
                let mut slot = s.gpu.write().unwrap_or_else(|p| p.into_inner());
                slot.get_or_insert_with(Default::default)
                    .record(outcome, ms);
            },
        );
    }

    if let Some(nas) = config.nas.clone() {
        let s = Arc::clone(shared);
        let in_flight = Arc::new(AtomicBool::new(false));
        every(
            "nas-probe",
            Duration::from_secs(config.intervals.nas_probe_secs),
            move || {
                let (outcome, ms) = timed(|| {
                    probe_nas(
                        &nas.root,
                        &nas.marker,
                        &s.config.node.id,
                        Duration::from_secs(nas.probe_timeout_secs),
                        &in_flight,
                    )
                });
                s.nas
                    .write()
                    .unwrap_or_else(|p| p.into_inner())
                    .record(outcome, ms);
            },
        );

        let s = Arc::clone(shared);
        let root = config
            .nas
            .as_ref()
            .map(|n| n.root.clone())
            .unwrap_or_default();
        every(
            "nas-sync",
            Duration::from_secs(config.intervals.sync_secs),
            move || {
                if !s.nas_healthy() {
                    return;
                }
                let now = unix_now();
                {
                    let mut sync = s.sync.write().unwrap_or_else(|p| p.into_inner());
                    sync.in_progress_since = now;
                    sync.last_run = now;
                }
                // Blocking here is acceptable: this thread does nothing else.
                let report = s.spool.deliver(&root);
                let mut sync = s.sync.write().unwrap_or_else(|p| p.into_inner());
                sync.in_progress_since = 0;
                sync.total_delivered_files += report.delivered_files;
                if report.error.is_none() {
                    sync.last_success = unix_now();
                } else {
                    // Let the prober re-establish health before the next attempt.
                    s.nas.write().unwrap_or_else(|p| p.into_inner()).healthy = false;
                }
                sync.last = report;
            },
        );
    }

    let s = Arc::clone(shared);
    every(
        "heartbeat",
        Duration::from_secs(config.intervals.heartbeat_secs),
        move || heartbeat(&s),
    );

    if config.node.role == NodeRole::Continuity
        && let Some(snapshot) = config.snapshot.clone()
    {
        let s = Arc::clone(shared);
        every(
            "snapshot",
            Duration::from_secs(snapshot.interval_secs),
            move || {
                let result = take_snapshot(&s, &snapshot.database, snapshot.spool_keep);
                let value = match result {
                    Ok(path) => json!({"ok": true, "at": iso8601(unix_now()), "staged": path}),
                    Err(e) => json!({"ok": false, "at": iso8601(unix_now()), "error": e}),
                };
                *s.snapshot.write().unwrap_or_else(|p| p.into_inner()) = value;
            },
        );
    }
}

/// Write-probe the NAS without letting a hung mount block the caller.
fn probe_nas(
    root: &Path,
    marker: &str,
    node: &str,
    timeout: Duration,
    in_flight: &Arc<AtomicBool>,
) -> Result<Value, String> {
    if in_flight.swap(true, Ordering::SeqCst) {
        return Err("previous NAS probe still blocked (mount hung?)".to_owned());
    }
    let root = root.to_path_buf();
    let marker = root.join(marker);
    let probe = root.join(format!(".probe-{node}"));
    let flag = Arc::clone(in_flight);
    let outcome = call_with_timeout(timeout, move || {
        let result = (|| {
            if !marker.is_file() {
                return Err("marker missing (NAS not mounted or not provisioned)".to_owned());
            }
            std::fs::write(&probe, iso8601(unix_now())).map_err(|e| format!("write: {e}"))?;
            std::fs::remove_file(&probe).map_err(|e| format!("remove: {e}"))?;
            Ok(json!({"writable": true}))
        })();
        flag.store(false, Ordering::SeqCst);
        result
    });
    outcome.unwrap_or_else(|| Err(format!("NAS probe exceeded {}s", timeout.as_secs())))
}

fn gpu_snapshot() -> Result<Value, String> {
    let out = run_with_timeout(
        "nvidia-smi",
        &[
            "--query-gpu=index,name,memory.used,memory.total,utilization.gpu,temperature.gpu",
            "--format=csv,noheader,nounits",
        ],
        Duration::from_secs(10),
    )
    .ok_or("nvidia-smi unavailable or timed out")?;
    if !out.success {
        return Err("nvidia-smi failed".to_owned());
    }
    let gpus: Vec<Value> = out
        .stdout
        .lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split(',').map(str::trim).collect();
            (f.len() == 6).then(|| {
                json!({
                    "index": f[0], "name": f[1],
                    "memory_used_mib": f[2].parse::<u64>().ok(),
                    "memory_total_mib": f[3].parse::<u64>().ok(),
                    "utilization_pct": f[4].parse::<u64>().ok(),
                    "temperature_c": f[5].parse::<u64>().ok(),
                })
            })
        })
        .collect();
    Ok(Value::Array(gpus))
}

/// Persist `current_state/state.json` locally and journal a heartbeat.
pub fn heartbeat(shared: &Shared) {
    let health = shared.health();
    let path = shared.config.paths.current_state().join("state.json");
    let doc = json!({"heartbeat_at": iso8601(unix_now()), "health": health});
    if let Ok(bytes) = serde_json::to_vec_pretty(&doc) {
        let _ = atomic_write(&path, &bytes);
    }
    let _ = shared.spool.append("logs/heartbeat", health);
}

/// Online snapshot of the canonical store into the spool (`VACUUM INTO`).
fn take_snapshot(shared: &Shared, database: &Path, keep: usize) -> Result<String, String> {
    if !database.is_file() {
        return Err(format!("{} does not exist", database.display()));
    }
    let node = &shared.config.node.id;
    let dir = format!("memory/snapshots/{node}");
    let name = format!("{}.sqlite", file_stamp(unix_now()));
    let final_path = shared
        .spool
        .stage_file(&format!("{dir}/{name}"))
        .map_err(|e| e.to_string())?;
    // `.tmp-` keeps the syncer away until the snapshot is complete.
    let tmp = final_path.with_file_name(format!("{name}.tmp-snapshot"));
    let _ = std::fs::remove_file(&tmp);
    let conn = rusqlite::Connection::open_with_flags(
        database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("open: {e}"))?;
    conn.busy_timeout(Duration::from_secs(30))
        .map_err(|e| format!("busy_timeout: {e}"))?;
    conn.execute("VACUUM INTO ?1", [tmp.to_string_lossy().as_ref()])
        .map_err(|e| format!("vacuum into: {e}"))?;
    std::fs::rename(&tmp, &final_path).map_err(|e| format!("rename: {e}"))?;
    shared
        .spool
        .trim_staged(&dir, keep)
        .map_err(|e| format!("trim: {e}"))?;
    Ok(format!("{dir}/{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nas_probe_requires_marker() {
        let dir = tempfile::tempdir().expect("tempdir");
        let flag = Arc::new(AtomicBool::new(false));
        let err = probe_nas(dir.path(), ".m", "pi", Duration::from_secs(2), &flag)
            .expect_err("no marker");
        assert!(err.contains("marker"));
        std::fs::write(dir.path().join(".m"), "").expect("marker");
        probe_nas(dir.path(), ".m", "pi", Duration::from_secs(2), &flag).expect("writable");
        assert!(!dir.path().join(".probe-pi").exists());
    }

    #[test]
    fn stuck_probe_is_reported_not_repeated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let flag = Arc::new(AtomicBool::new(true));
        let err = probe_nas(dir.path(), ".m", "pi", Duration::from_secs(1), &flag)
            .expect_err("in flight");
        assert!(err.contains("blocked"));
    }

    #[test]
    fn missing_credential_is_named_not_leaked() {
        let err = bearer(Some("KAMIMUSUHI_TEST_UNSET_VAR")).expect_err("unset");
        assert!(err.contains("KAMIMUSUHI_TEST_UNSET_VAR"));
    }
}
