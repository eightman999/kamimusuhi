//! Shared, in-memory view of the node and its surroundings.
//!
//! Probe threads write here; the HTTP server and router only read. Nothing
//! in this module performs I/O.

use std::collections::BTreeMap;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use serde_json::{Value, json};

use crate::config::Config;
use crate::spool::{Spool, SyncReport};
use crate::util::{iso8601, unix_now};

#[derive(Debug, Clone, Default, Serialize)]
pub struct ProbeResult {
    pub healthy: bool,
    pub checked_at: u64,
    pub last_ok: u64,
    pub latency_ms: u64,
    pub consecutive_failures: u32,
    pub error: Option<String>,
    /// Probe-specific payload (peer health document, balance, GPU list…).
    #[serde(skip_serializing_if = "Value::is_null")]
    pub detail: Value,
}

impl ProbeResult {
    pub fn record(&mut self, outcome: Result<Value, String>, latency_ms: u64) {
        let now = unix_now();
        self.checked_at = now;
        self.latency_ms = latency_ms;
        match outcome {
            Ok(detail) => {
                self.healthy = true;
                self.last_ok = now;
                self.consecutive_failures = 0;
                self.error = None;
                self.detail = detail;
            }
            Err(error) => {
                self.healthy = false;
                self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                self.error = Some(error);
            }
        }
    }

    fn view(&self) -> Value {
        let mut value = serde_json::to_value(self).unwrap_or(Value::Null);
        if let Value::Object(map) = &mut value {
            let stamp = |t: u64| {
                if t == 0 {
                    Value::Null
                } else {
                    Value::String(iso8601(t))
                }
            };
            map.insert("checked_at".into(), stamp(self.checked_at));
            map.insert("last_ok".into(), stamp(self.last_ok));
        }
        value
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SyncState {
    pub last_run: u64,
    pub last_success: u64,
    pub in_progress_since: u64,
    pub last: SyncReport,
    pub total_delivered_files: u64,
}

/// What one routed request did: which tier answered, at what cost basis.
/// Token/cost fields are `None` when the upstream did not report them —
/// an unknown cost is never rendered as zero.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RouteEvent {
    pub at: u64,
    pub tier: Option<String>,
    /// Upstream model that served the request.
    pub model: Option<String>,
    /// The tier's declared billing class (`local`/`subscription`/
    /// `free_tier`/`metered`); `None` = undeclared.
    pub billing: Option<String>,
    pub attempts: Vec<String>,
    pub latency_ms: u64,
    pub ok: bool,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    /// Prompt tokens served from an upstream prefix cache, when reported.
    pub cached_tokens: Option<u64>,
    /// USD this request cost — actual when the provider billed and said
    /// so, estimate when computed from configured prices.
    pub cost_usd: Option<f64>,
    /// `actual` | `estimate`; absent when `cost_usd` is.
    pub cost_kind: Option<String>,
}

pub struct Shared {
    pub config: Config,
    pub started: u64,
    pub boot_epoch: u64,
    pub spool: Spool,
    pub token: Option<String>,
    pub tiers: RwLock<BTreeMap<String, ProbeResult>>,
    pub peers: RwLock<BTreeMap<String, ProbeResult>>,
    pub nas: RwLock<ProbeResult>,
    pub sync: RwLock<SyncState>,
    pub account: RwLock<Option<ProbeResult>>,
    pub gpu: RwLock<Option<ProbeResult>>,
    pub kcore: RwLock<Value>,
    pub snapshot: RwLock<Value>,
    pub last_route: RwLock<Option<RouteEvent>>,
    pub requests: AtomicU64,
    pub mcp: Vec<std::sync::Arc<crate::mcp::McpServer>>,
    pub approvals: crate::approvals::ApprovalQueue,
    pub tasks: crate::tasks::TaskBoard,
    /// Task id of the dialogue turn in progress, so tool calls made during
    /// it are recorded as its successors.
    pub current_turn: std::sync::Mutex<Option<String>>,
    pub jobs: crate::jobs::JobStatus,
}

fn read<T: Clone>(lock: &RwLock<T>) -> T {
    lock.read().unwrap_or_else(|p| p.into_inner()).clone()
}

impl Shared {
    pub fn new(config: Config, spool: Spool, boot_epoch: u64, token: Option<String>) -> Self {
        let tiers = config
            .tiers
            .iter()
            .map(|t| (t.name.clone(), ProbeResult::default()))
            .collect();
        let peers = config
            .peers
            .iter()
            .map(|p| (p.id.clone(), ProbeResult::default()))
            .collect();
        let mcp = config
            .mcp_servers
            .iter()
            .map(|c| {
                std::sync::Arc::new(crate::mcp::McpServer::new(
                    c.clone(),
                    config.paths.cache().join("mcp"),
                ))
            })
            .collect();
        let approvals = crate::approvals::ApprovalQueue::load(
            config.paths.current_state().join("approvals.json"),
        );
        Self {
            mcp,
            approvals,
            tasks: crate::tasks::TaskBoard::load(config.paths.current_state().join("tasks.json")),
            current_turn: std::sync::Mutex::new(None),
            jobs: crate::jobs::JobStatus::default(),
            config,
            started: unix_now(),
            boot_epoch,
            spool,
            token,
            tiers: RwLock::new(tiers),
            peers: RwLock::new(peers),
            nas: RwLock::new(ProbeResult::default()),
            sync: RwLock::new(SyncState::default()),
            account: RwLock::new(None),
            gpu: RwLock::new(None),
            kcore: RwLock::new(json!({"state": "disabled"})),
            snapshot: RwLock::new(Value::Null),
            last_route: RwLock::new(None),
            requests: AtomicU64::new(0),
        }
    }

    pub fn tier_healthy(&self, name: &str) -> bool {
        self.tiers
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(name)
            .is_some_and(|r| r.healthy)
    }

    pub fn set_tier(&self, name: &str, outcome: Result<Value, String>, latency_ms: u64) {
        let mut tiers = self.tiers.write().unwrap_or_else(|p| p.into_inner());
        tiers
            .entry(name.to_owned())
            .or_default()
            .record(outcome, latency_ms);
    }

    pub fn nas_healthy(&self) -> bool {
        self.nas.read().unwrap_or_else(|p| p.into_inner()).healthy
    }

    /// Minimal unauthenticated document for peers and uptime monitors.
    pub fn health(&self) -> Value {
        let tiers: BTreeMap<String, bool> = read(&self.tiers)
            .into_iter()
            .map(|(k, v)| (k, v.healthy))
            .collect();
        let local_ok = self
            .config
            .tiers
            .iter()
            .filter(|t| t.node_local)
            .any(|t| tiers.get(&t.name).copied().unwrap_or(false));
        json!({
            "service": "kamimusuhi-resident",
            "version": env!("CARGO_PKG_VERSION"),
            "node": self.config.node.id,
            "role": self.config.node.role.as_str(),
            "authoritative_state": self.config.node.role == crate::config::NodeRole::Continuity,
            "started_at": iso8601(self.started),
            "uptime_secs": unix_now().saturating_sub(self.started),
            "boot_epoch": self.boot_epoch,
            "tiers": tiers,
            "local_llm": local_ok,
            "nas": self.nas_healthy(),
            "kcore": read(&self.kcore).get("state").cloned().unwrap_or(Value::Null),
            "gpu": read(&self.gpu).map(|g| g.detail),
        })
    }

    /// Full status for `kamimusuhi status`.
    pub fn status(&self) -> Value {
        let (spool_bytes, spool_files) = self.spool.backlog();
        let sync = read(&self.sync);
        let nas = read(&self.nas);
        let tiers: BTreeMap<String, Value> = read(&self.tiers)
            .into_iter()
            .map(|(k, v)| (k, v.view()))
            .collect();
        let peers: BTreeMap<String, Value> = read(&self.peers)
            .into_iter()
            .map(|(k, v)| (k, v.view()))
            .collect();
        let order: Vec<&str> = self.config.tiers.iter().map(|t| t.name.as_str()).collect();
        let active = self
            .config
            .tiers
            .iter()
            .find(|t| {
                t.condition == crate::config::TierCondition::Always && self.tier_healthy(&t.name)
            })
            .map(|t| t.name.clone());
        json!({
            "node": self.health(),
            "peers": peers,
            "routing": {
                "order": order,
                "active_primary": active,
                "tiers": tiers,
                "last_route": read(&self.last_route),
                "requests": self.requests.load(Ordering::Relaxed),
            },
            "nas": {
                "configured": self.config.nas.is_some(),
                "root": self.config.nas.as_ref().map(|n| n.root.display().to_string()),
                "probe": nas.view(),
                "spool_backlog_bytes": spool_bytes,
                "spool_backlog_files": spool_files,
                "spool_warn": spool_bytes > self.config.paths.spool_warn_bytes,
                "sync": {
                    "last_run": (sync.last_run > 0).then(|| iso8601(sync.last_run)),
                    "last_success": (sync.last_success > 0).then(|| iso8601(sync.last_success)),
                    "in_progress_since": (sync.in_progress_since > 0).then(|| iso8601(sync.in_progress_since)),
                    "last": sync.last,
                    "total_delivered_files": sync.total_delivered_files,
                },
            },
            "hai_account": read(&self.account).map(|a| a.view()),
            "kcore": read(&self.kcore),
            "snapshot": read(&self.snapshot),
            "mcp": self.mcp.iter().map(|m| m.status()).collect::<Vec<_>>(),
            "jobs": read(&*self.jobs),
            "approvals_pending": self.approvals.list(true).iter().map(|a| a.view()).collect::<Vec<_>>(),
            "generated_at": iso8601(unix_now()),
        })
    }
}
