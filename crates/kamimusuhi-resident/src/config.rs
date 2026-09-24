//! Resident node configuration.
//!
//! The file holds topology and policy only. Credentials are never stored in
//! it: every secret is referenced by the *name* of an environment variable,
//! which systemd loads from a mode-600 file outside the repository.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const DEFAULT_CONFIG_PATH: &str = "/srv/kamimusuhi/config/resident.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeRole {
    /// 24h node: continuity, scheduler, authoritative state (Raspberry Pi).
    Continuity,
    /// Daytime node: cognition, reflection, GPU jobs (llm_master).
    Cognition,
}

impl NodeRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Continuity => "continuity",
            Self::Cognition => "cognition",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub node: NodeConfig,
    pub paths: PathsConfig,
    #[serde(default)]
    pub nas: Option<NasConfig>,
    #[serde(default)]
    pub peers: Vec<PeerConfig>,
    #[serde(default)]
    pub tiers: Vec<TierConfig>,
    #[serde(default)]
    pub routing: RoutingConfig,
    #[serde(default)]
    pub hai_account: Option<HaiAccountConfig>,
    #[serde(default)]
    pub kcore: Option<KCoreConfig>,
    #[serde(default)]
    pub snapshot: Option<SnapshotConfig>,
    #[serde(default)]
    pub dialogue: Option<DialogueConfig>,
    #[serde(default)]
    pub libraries: Vec<LibraryConfig>,
    /// MCP servers this node runs (stdio). Their tools join `/v1/tools`.
    #[serde(default)]
    pub mcp_servers: Vec<crate::mcp::McpServerConfig>,
    /// Commands run periodically on this node (e.g. mirror `git pull`).
    #[serde(default)]
    pub jobs: Vec<crate::jobs::JobConfig>,
    #[serde(default)]
    pub intervals: Intervals,
    /// External agent harnesses this node runs tasks on (task plane).
    #[serde(default)]
    pub task_plane: Option<TaskPlaneConfig>,
}

/// Task plane: agent harnesses (Devin, OpenCode, Command Code, …) the
/// individual can delegate work to. Harnesses are listed in `executors`
/// and/or in `executors_file`; the file is re-read whenever it changes, so
/// a harness can be added or removed without restarting the resident
/// (which would interrupt running tasks).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskPlaneConfig {
    #[serde(default)]
    pub executors: Vec<kamimusuhi_runtime::agent_exec::ExecutorConfig>,
    /// `{"executors": [...]}`, merged after `executors` (same names are an
    /// error).
    #[serde(default)]
    pub executors_file: Option<PathBuf>,
    /// Directories tasks may run in. Delegation names one of these; no
    /// other path is ever handed to a harness.
    pub workspaces: Vec<WorkspaceConfig>,
    /// Tasks running at once on this node, over all executors.
    #[serde(default = "default_task_concurrency")]
    pub max_concurrent: usize,
    /// How often catalog freshness, `executors_file` and helper servers
    /// are checked.
    #[serde(default = "default_catalog_check")]
    pub check_interval_secs: u64,
    /// Limits over every executor together (each executor may also set
    /// its own `quota`).
    #[serde(default)]
    pub quota: Option<kamimusuhi_runtime::agent_exec::Quota>,
    /// How `executor: auto` chooses: `static` rules, `shadow` (static,
    /// with the history-based choice recorded for comparison) or `history`
    /// (measured outcomes once `history_min_samples` exist).
    #[serde(default)]
    pub routing_mode: RoutingMode,
    #[serde(default = "default_min_samples")]
    pub history_min_samples: usize,
    /// Evaluation battery (`task-battery.example.json` format); the
    /// built-in Q1–Q7 battery otherwise.
    #[serde(default)]
    pub battery_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RoutingMode {
    Static,
    #[default]
    Shadow,
    History,
}

impl RoutingMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::Shadow => "shadow",
            Self::History => "history",
        }
    }
}

const fn default_min_samples() -> usize {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfig {
    pub name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub description: Option<String>,
    /// Permit write tasks. They still never touch this directory: each
    /// runs in its own git worktree and branch, merged only by an
    /// explicit operator `apply`.
    #[serde(default)]
    pub allow_write: bool,
}

const fn default_task_concurrency() -> usize {
    2
}

const fn default_catalog_check() -> u64 {
    60
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeConfig {
    /// Stable short id used in NAS paths (`logs/<category>/<id>/…`).
    pub id: String,
    pub role: NodeRole,
    /// e.g. `0.0.0.0:7860`.
    pub listen: String,
    /// Environment variable holding the shared node bearer token. Required
    /// for every non-loopback request except `/health`.
    #[serde(default = "default_token_env")]
    pub token_env: String,
}

fn default_token_env() -> String {
    "KAMIMUSUHI_NODE_TOKEN".to_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathsConfig {
    /// Local root holding `runtime/ config/ current_state/ cache/ spool/`.
    pub root: PathBuf,
    /// Warn (status: degraded) once the local spool exceeds this size.
    #[serde(default = "default_spool_warn")]
    pub spool_warn_bytes: u64,
}

const fn default_spool_warn() -> u64 {
    8 * 1024 * 1024 * 1024
}

impl PathsConfig {
    pub fn spool(&self) -> PathBuf {
        self.root.join("spool")
    }
    pub fn current_state(&self) -> PathBuf {
        self.root.join("current_state")
    }
    pub fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }
    pub fn runtime(&self) -> PathBuf {
        self.root.join("runtime")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NasConfig {
    /// Mounted NAS directory (e.g. `/mnt/kamimusuhi`). Only used while the
    /// marker file below exists, so an unmounted mountpoint on the local SSD
    /// is never mistaken for the NAS.
    pub root: PathBuf,
    #[serde(default = "default_marker")]
    pub marker: String,
    #[serde(default = "default_nas_timeout")]
    pub probe_timeout_secs: u64,
}

fn default_marker() -> String {
    ".kamimusuhi-nas".to_owned()
}

const fn default_nas_timeout() -> u64 {
    8
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerConfig {
    pub id: String,
    pub role: NodeRole,
    /// Base URL of the peer resident, e.g. `http://llm-master.example:7860`.
    pub url: String,
    /// Other addresses of the same peer (e.g. LAN and tailnet). The health
    /// probe tries them in order and requests go to whichever answered.
    #[serde(default)]
    pub alt_urls: Vec<String>,
}

impl PeerConfig {
    /// `url` first, then `alt_urls`.
    pub fn urls(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.url.as_str()).chain(self.alt_urls.iter().map(String::as_str))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TierCondition {
    /// Eligible for every request.
    #[default]
    Always,
    /// Only for short requests (K0 / local-class work) or when asked for.
    SmallRequest,
}

/// How a tier is paid for — display and policy metadata, mirroring
/// `BillingClass` in the runtime's route gate. `None` means the plan is
/// undeclared; an undeclared tier's cost is shown as unknown, never as free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TierBilling {
    /// Self-hosted or peer node; marginal cost is zero.
    Local,
    /// Flat subscription already paid for (e.g. HAI).
    Subscription,
    /// Provider free tier.
    FreeTier,
    /// Per-token billing.
    Metered,
}

impl TierBilling {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Subscription => "subscription",
            Self::FreeTier => "free_tier",
            Self::Metered => "metered",
        }
    }
}

/// One OpenAI-compatible backend in the routing order.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TierConfig {
    /// Route name (`local`, `llm_master`, `hai`); also accepted as a model
    /// prefix to force this tier (`hai/glm-5.3`).
    pub name: String,
    /// Base URL ending in `/v1`.
    pub base_url: String,
    /// Model sent to the backend when the client asks for `kamimusuhi`.
    pub model: String,
    #[serde(default)]
    pub auth_env: Option<String>,
    #[serde(default = "default_tier_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub condition: TierCondition,
    /// True when the backend runs on this node. A peer asking with
    /// `X-Kamimusuhi-Route: local` is served from these tiers only, so the
    /// caller keeps control of its own fallback.
    #[serde(default)]
    pub node_local: bool,
    /// Ask the backend itself not to fall back (used when the backend is a
    /// peer resident).
    #[serde(default)]
    pub peer_local_only: bool,
    /// Seconds between health probes for this tier.
    #[serde(default = "default_probe_interval")]
    pub probe_interval_secs: u64,
    /// How this tier is billed. Drives the per-turn cost display; when the
    /// prices below are set, metered turns also get an estimate.
    #[serde(default)]
    pub billing: Option<TierBilling>,
    /// USD per million prompt tokens, for metered tiers.
    #[serde(default)]
    pub input_usd_per_mtok: Option<f64>,
    /// USD per million completion tokens, for metered tiers.
    #[serde(default)]
    pub output_usd_per_mtok: Option<f64>,
    /// Explicit privacy declaration. When omitted, only local/subscription
    /// tiers are trusted with persona memory; free/metered tiers fail closed.
    #[serde(default)]
    pub privacy_ok_for_private_memory: Option<bool>,
    /// Maximum prompt tokens accepted by this tier, when known.
    #[serde(default)]
    pub context_limit_tokens: Option<u64>,
    /// Approximate per-minute token budget used to avoid predictable 429s.
    #[serde(default)]
    pub approx_tpm_limit: Option<u64>,
    /// Whether the tier can suppress reasoning for latency-sensitive chat.
    #[serde(default)]
    pub reasoning_suppression: Option<bool>,
    /// Provider-specific request fields added to every upstream call (e.g.
    /// `{"reasoning_effort": "low"}` for gpt-oss). Fields the client already
    /// set, and the routing-owned `model`/`stream`/`messages`, win.
    #[serde(default)]
    pub extra_body: Option<serde_json::Map<String, serde_json::Value>>,
    /// Request fields removed before calling this tier, for providers that
    /// reject fields they do not know (e.g. `chat_template_kwargs`, which
    /// llama.cpp and HAI accept but strict OpenAI-compatible APIs refuse).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strip_fields: Vec<String>,
    /// Lanes this tier leads ahead of the billing order (e.g. a fast cloud
    /// for `FAST_CHAT`). Privacy, health and spend limits still apply.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prefer_lanes: Vec<kamimusuhi_runtime::route_gate::RouteLane>,
}

const fn default_tier_timeout() -> u64 {
    180
}

const fn default_probe_interval() -> u64 {
    20
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingConfig {
    /// Requests whose messages total at most this many characters are
    /// "small" and may use `small_request` tiers.
    #[serde(default = "default_small_chars")]
    pub small_request_chars: usize,
    /// Record request/response pairs under `conversations/`.
    #[serde(default = "default_true")]
    pub log_conversations: bool,
    /// User-facing latency ceiling for FAST_CHAT. The production resident
    /// uses this as the RouteGate health cutoff; default is the 4s SLA.
    #[serde(default = "default_fast_chat_latency_ceiling_ms")]
    pub fast_chat_latency_ceiling_ms: u64,
    /// Estimated prompt tokens from which an unhinted request is routed on
    /// the MEMORY_HEAVY lane (context-limit aware, no fast-chat ceiling).
    #[serde(default = "default_memory_heavy_prompt_tokens")]
    pub memory_heavy_prompt_tokens: u64,
    /// The last user message counts as everyday quick chat (`FAST_CHAT`)
    /// up to this many characters; longer requests go to `DEEP_REASONING`.
    #[serde(default = "default_fast_chat_max_input_chars")]
    pub fast_chat_max_input_chars: usize,
    /// Persona requests are private unless an internal caller explicitly
    /// marks a request public.
    #[serde(default = "default_true")]
    pub private_by_default: bool,
    /// Completion-token assumption used for metered pre-flight estimates
    /// when the request does not carry an explicit limit.
    #[serde(default = "default_max_completion_tokens_for_cost")]
    pub max_completion_tokens_for_cost: u32,
    #[serde(default = "default_max_request_cost_usd")]
    pub max_request_cost_usd: f64,
    #[serde(default = "default_max_session_cost_usd")]
    pub max_session_cost_usd: f64,
    #[serde(default = "default_max_daily_cost_usd")]
    pub max_daily_cost_usd: Option<f64>,
    #[serde(default = "default_max_monthly_cost_usd")]
    pub max_monthly_cost_usd: Option<f64>,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            small_request_chars: default_small_chars(),
            log_conversations: true,
            fast_chat_latency_ceiling_ms: default_fast_chat_latency_ceiling_ms(),
            memory_heavy_prompt_tokens: default_memory_heavy_prompt_tokens(),
            fast_chat_max_input_chars: default_fast_chat_max_input_chars(),
            private_by_default: true,
            max_completion_tokens_for_cost: default_max_completion_tokens_for_cost(),
            max_request_cost_usd: default_max_request_cost_usd(),
            max_session_cost_usd: default_max_session_cost_usd(),
            max_daily_cost_usd: default_max_daily_cost_usd(),
            max_monthly_cost_usd: default_max_monthly_cost_usd(),
        }
    }
}

const fn default_small_chars() -> usize {
    1200
}

const fn default_fast_chat_latency_ceiling_ms() -> u64 {
    4_000
}

const fn default_fast_chat_max_input_chars() -> usize {
    200
}

const fn default_memory_heavy_prompt_tokens() -> u64 {
    16_000
}

const fn default_max_completion_tokens_for_cost() -> u32 {
    768
}

const fn default_max_request_cost_usd() -> f64 {
    0.02
}

const fn default_max_session_cost_usd() -> f64 {
    0.50
}

fn default_max_daily_cost_usd() -> Option<f64> {
    Some(0.10)
}

fn default_max_monthly_cost_usd() -> Option<f64> {
    Some(2.00)
}

const fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HaiAccountConfig {
    /// e.g. `https://hai-api.hcloud.ltd/api`.
    pub base_url: String,
    pub auth_env: String,
    #[serde(default = "default_account_interval")]
    pub interval_secs: u64,
    /// Balance (JPY) under which status reports a warning.
    #[serde(default)]
    pub low_balance_jpy: f64,
}

const fn default_account_interval() -> u64 {
    900
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KCoreConfig {
    /// Path to the `k-core` binary.
    pub binary: PathBuf,
    /// Initialized runtime directory (canonical `kamimusuhi.sqlite`). The
    /// resident never initializes an individual; see deploy/README.md.
    pub dir: PathBuf,
    #[serde(default = "default_kcore_interval")]
    pub interval_ms: u64,
}

const fn default_kcore_interval() -> u64 {
    60_000
}

/// A read-only reference library held on this node (see `library.rs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryConfig {
    pub name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub description: Option<String>,
    /// Upstream origin, for provenance (e.g. a git URL).
    #[serde(default)]
    pub source: Option<String>,
}

/// Dialogue with the individual itself (Persona Core + memory), served by
/// running `kamimusuhi-runtime talk` against the canonical runtime directory.
/// Only the continuity node, which owns that directory, enables this.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DialogueConfig {
    /// Path to the `kamimusuhi-runtime` binary.
    pub runtime_binary: PathBuf,
    /// Initialized runtime directory (same individual K-CORE resumes).
    pub dir: PathBuf,
    /// `--privacy` for each turn. `unconstrained` is required while the
    /// persona endpoint may route to HAI.
    #[serde(default = "default_privacy")]
    pub privacy: String,
    #[serde(default = "default_dialogue_timeout")]
    pub timeout_secs: u64,
    /// Turns that may run at once across all clients. Turns for the same
    /// subject still run one at a time, in arrival order.
    #[serde(default = "default_dialogue_concurrency")]
    pub max_concurrent: usize,
}

const fn default_dialogue_concurrency() -> usize {
    2
}

fn default_privacy() -> String {
    "unconstrained".to_owned()
}

const fn default_dialogue_timeout() -> u64 {
    600
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotConfig {
    /// SQLite database to snapshot with `VACUUM INTO`.
    pub database: PathBuf,
    #[serde(default = "default_snapshot_interval")]
    pub interval_secs: u64,
    /// Snapshots kept in the local spool while the NAS is unreachable.
    #[serde(default = "default_spool_keep")]
    pub spool_keep: usize,
}

const fn default_snapshot_interval() -> u64 {
    3600
}

const fn default_spool_keep() -> usize {
    3
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Intervals {
    #[serde(default = "default_heartbeat")]
    pub heartbeat_secs: u64,
    #[serde(default = "default_sync")]
    pub sync_secs: u64,
    #[serde(default = "default_peer")]
    pub peer_secs: u64,
    #[serde(default = "default_nas_probe")]
    pub nas_probe_secs: u64,
    #[serde(default = "default_gpu")]
    pub gpu_secs: u64,
}

impl Default for Intervals {
    fn default() -> Self {
        Self {
            heartbeat_secs: default_heartbeat(),
            sync_secs: default_sync(),
            peer_secs: default_peer(),
            nas_probe_secs: default_nas_probe(),
            gpu_secs: default_gpu(),
        }
    }
}

const fn default_heartbeat() -> u64 {
    60
}
const fn default_sync() -> u64 {
    30
}
const fn default_peer() -> u64 {
    15
}
const fn default_nas_probe() -> u64 {
    30
}
const fn default_gpu() -> u64 {
    60
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text =
            std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
        let config: Self =
            serde_json::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), String> {
        let id_ok = |id: &str| {
            !id.is_empty()
                && id.len() <= 64
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        };
        if !id_ok(&self.node.id) {
            return Err("node.id must be 1-64 chars of [A-Za-z0-9_-]".to_owned());
        }
        if !self.paths.root.is_absolute() {
            return Err("paths.root must be absolute".to_owned());
        }
        let mut names = std::collections::HashSet::new();
        for tier in &self.tiers {
            if !id_ok(&tier.name) || !names.insert(tier.name.as_str()) {
                return Err(format!(
                    "tier name {:?} is invalid or duplicated",
                    tier.name
                ));
            }
            kamimusuhi_resource_http::Endpoint::parse(&tier.base_url, "/chat/completions")
                .map_err(|e| format!("tier {}: {e}", tier.name))?;
            if tier.timeout_secs == 0 || tier.probe_interval_secs == 0 {
                return Err(format!("tier {}: intervals must be positive", tier.name));
            }
            let billing = tier
                .billing
                .ok_or_else(|| format!("tier {}: billing must be declared", tier.name))?;
            let valid_price = |value: Option<f64>| value.is_none_or(|v| v.is_finite() && v >= 0.0);
            if !valid_price(tier.input_usd_per_mtok) || !valid_price(tier.output_usd_per_mtok) {
                return Err(format!(
                    "tier {}: prices must be finite and non-negative",
                    tier.name
                ));
            }
            if billing == TierBilling::Metered
                && (tier.input_usd_per_mtok.is_none() || tier.output_usd_per_mtok.is_none())
            {
                return Err(format!(
                    "tier {}: metered tiers require input/output prices for pre-flight cost control",
                    tier.name
                ));
            }
            if billing == TierBilling::FreeTier
                && (tier.input_usd_per_mtok.unwrap_or(0.0) > 0.0
                    || tier.output_usd_per_mtok.unwrap_or(0.0) > 0.0)
            {
                return Err(format!(
                    "tier {}: free_tier must not declare non-zero token prices",
                    tier.name
                ));
            }
        }
        let finite_non_negative = |v: f64| v.is_finite() && v >= 0.0;
        if self.routing.fast_chat_latency_ceiling_ms == 0
            || self.routing.max_completion_tokens_for_cost == 0
            || !finite_non_negative(self.routing.max_request_cost_usd)
            || !finite_non_negative(self.routing.max_session_cost_usd)
            || self.routing.max_request_cost_usd > self.routing.max_session_cost_usd
            || self
                .routing
                .max_daily_cost_usd
                .is_some_and(|v| !finite_non_negative(v))
            || self
                .routing
                .max_monthly_cost_usd
                .is_some_and(|v| !finite_non_negative(v))
        {
            return Err("routing latency/cost limits are invalid".to_owned());
        }
        for peer in &self.peers {
            if !id_ok(&peer.id) {
                return Err(format!("peer id {:?} is invalid", peer.id));
            }
            for url in peer.urls() {
                kamimusuhi_resource_http::Endpoint::parse(url, "/health")
                    .map_err(|e| format!("peer {}: {e}", peer.id))?;
            }
        }
        let mut mcp_names = std::collections::HashSet::new();
        for server in &self.mcp_servers {
            if !id_ok(&server.name) || !mcp_names.insert(server.name.as_str()) {
                return Err(format!(
                    "mcp server name {:?} is invalid or duplicated",
                    server.name
                ));
            }
        }
        for lib in &self.libraries {
            if !id_ok(&lib.name) || !lib.path.is_absolute() {
                return Err(format!("library {:?}: name or path invalid", lib.name));
            }
        }
        if let Some(plane) = &self.task_plane {
            kamimusuhi_runtime::agent_exec::resolve_all(&plane.executors)
                .map_err(|e| format!("task_plane: {e}"))?;
            if plane
                .executors_file
                .iter()
                .chain(&plane.battery_file)
                .any(|p| !p.is_absolute())
            {
                return Err("task_plane.executors_file/battery_file must be absolute".to_owned());
            }
            if let Some(q) = &plane.quota {
                q.validate().map_err(|e| format!("task_plane.quota: {e}"))?;
            }
            if plane.workspaces.is_empty()
                || plane.max_concurrent == 0
                || plane.check_interval_secs == 0
            {
                return Err(
                    "task_plane needs workspaces and positive max_concurrent/check_interval_secs"
                        .to_owned(),
                );
            }
            let mut names = std::collections::HashSet::new();
            for ws in &plane.workspaces {
                if !id_ok(&ws.name) || !names.insert(ws.name.as_str()) || !ws.path.is_absolute() {
                    return Err(format!(
                        "task_plane workspace {:?}: name invalid/duplicated or path not absolute",
                        ws.name
                    ));
                }
            }
        }
        if self
            .dialogue
            .as_ref()
            .is_some_and(|d| d.max_concurrent == 0)
        {
            return Err("dialogue.max_concurrent must be positive".to_owned());
        }
        if let Some(nas) = &self.nas
            && (!nas.root.is_absolute() || nas.marker.contains('/'))
        {
            return Err("nas.root must be absolute and nas.marker a plain file name".to_owned());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deploy_examples_parse() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/resident");
        for name in [
            "pi.resident.example.json",
            "llm_master.resident.example.json",
        ] {
            let config = Config::load(&root.join(name)).expect(name);
            assert!(!config.tiers.is_empty(), "{name} has tiers");
        }
    }

    #[test]
    fn mac_example_parses_after_substitution() {
        let text = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../deploy/resident/mac.resident.example.json"),
        )
        .expect("read")
        .replace("@ROOT@", "/tmp/kamimusuhi-node")
        .replace("@HOME@", "/Users/example");
        let config: Config = serde_json::from_str(&text).expect("parses");
        config.validate().expect("valid");
        let plane = config.task_plane.expect("task plane");
        assert!(plane.workspaces[0].allow_write);
    }

    #[test]
    fn agent_executors_example_resolves() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../deploy/resident/agent-executors.example.json");
        let executors =
            crate::task_orchestrator::parse_executors_file(&std::fs::read(&path).expect("read"))
                .expect("executors");
        let specs = kamimusuhi_runtime::agent_exec::resolve_all(&executors).expect("resolves");
        assert_eq!(specs.len(), 4);
    }

    #[test]
    fn provider_tiers_example_parses_and_keeps_training_tiers_off_private_memory() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../deploy/resident/provider-tiers.example.json");
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
        let tiers: Vec<TierConfig> = serde_json::from_value(value["tiers"].clone()).expect("tiers");
        for tier in &tiers {
            assert!(tier.billing.is_some(), "{} declares billing", tier.name);
            assert!(tier.auth_env.is_some(), "{} names its key", tier.name);
        }
        let private_ok = |name: &str| {
            tiers
                .iter()
                .find(|t| t.name == name)
                .and_then(|t| t.privacy_ok_for_private_memory)
        };
        assert_eq!(private_ok("gemini"), Some(false));
        assert_eq!(private_ok("openrouter"), Some(false));
        assert_eq!(private_ok("groq"), Some(true));
    }

    #[test]
    fn rejects_bad_ids() {
        let text = r#"{"node":{"id":"a/b","role":"continuity","listen":"127.0.0.1:1"},
                       "paths":{"root":"/tmp/x"}}"#;
        let config: Config = serde_json::from_str(text).expect("parses");
        assert!(config.validate().is_err());
    }
}
