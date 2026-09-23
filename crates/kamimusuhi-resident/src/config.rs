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
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            small_request_chars: default_small_chars(),
            log_conversations: true,
        }
    }
}

const fn default_small_chars() -> usize {
    1200
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
        }
        for peer in &self.peers {
            if !id_ok(&peer.id) {
                return Err(format!("peer id {:?} is invalid", peer.id));
            }
            kamimusuhi_resource_http::Endpoint::parse(&peer.url, "/health")
                .map_err(|e| format!("peer {}: {e}", peer.id))?;
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
    fn rejects_bad_ids() {
        let text = r#"{"node":{"id":"a/b","role":"continuity","listen":"127.0.0.1:1"},
                       "paths":{"root":"/tmp/x"}}"#;
        let config: Config = serde_json::from_str(text).expect("parses");
        assert!(config.validate().is_err());
    }
}
