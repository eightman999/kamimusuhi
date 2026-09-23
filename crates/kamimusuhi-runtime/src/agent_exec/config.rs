//! Executor configuration.
//!
//! Every executor is data: a command line to discover models, a command
//! line to run a task, and the parsers for their output. The named
//! adapters (`devin_cli`, `opencode`, `command_code`) are presets that fill
//! those fields for a known harness; `generic` starts empty so a new
//! harness can be added from configuration alone. Any preset field can be
//! overridden, and removing an executor is removing its entry (or setting
//! `enabled: false`) — no code change either way.

use serde::{Deserialize, Serialize};

use super::types::{AgentBilling, Quota, TaskKind, TaskPermissions};

/// Which preset an executor starts from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterKind {
    /// Devin CLI (`devin -p`, `devin models list --format json`).
    DevinCli,
    /// OpenCode (`opencode run --format json`, `opencode models --verbose`).
    Opencode,
    /// Command Code CLI (`cmd -p --output-format json`, `cmd --list-models`).
    CommandCode,
    /// No preset: everything comes from configuration.
    Generic,
}

impl AdapterKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DevinCli => "devin_cli",
            Self::Opencode => "opencode",
            Self::CommandCode => "command_code",
            Self::Generic => "generic",
        }
    }
}

/// How to read a model listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverFormat {
    /// One model id per line (first whitespace-separated token); blank
    /// lines and lines without an id-like token are skipped.
    Lines,
    /// JSON: an array of ids or of objects with `id`/`model`/`name`, or an
    /// OpenAI-style `{"data": [...]}`.
    Json,
    /// `devin models list --format json`.
    DevinJson,
    /// `opencode models --verbose`: `provider/model` lines each followed
    /// by a JSON object.
    OpencodeVerbose,
    /// `cmd --list-models` text table.
    CommandCodeText,
}

/// How to read a run's stdout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    /// Whole stdout is the answer.
    Text,
    /// `opencode run --format json` event stream.
    OpencodeJson,
    /// `cmd -p --output-format json` NDJSON stream.
    CommandCodeNdjson,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoverSpec {
    pub args: Vec<String>,
    pub format: DiscoverFormat,
}

/// Run command line: `prefix ++ permission args ++ model args ++
/// extra_args ++ args`. `{prompt}` and `{workspace}` are substituted in
/// `args`; `{model}` in `model_args`, which are left out when no model is
/// requested.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSpec {
    #[serde(default)]
    pub prefix: Vec<String>,
    pub args: Vec<String>,
    #[serde(default)]
    pub model_args: Vec<String>,
    /// Arguments that continue an earlier session (`{session}`). Empty =
    /// the harness cannot continue from the command line.
    #[serde(default)]
    pub resume_args: Vec<String>,
    pub output: OutputFormat,
}

/// How the resident talks to a harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// One process per task (`run`).
    #[default]
    Cli,
    /// Long-lived Agent Client Protocol server (JSON-RPC over stdio), one
    /// session per task.
    Acp,
}

impl Protocol {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Acp => "acp",
        }
    }
}

/// Session mode per permission (an ACP `mode` config option value). A
/// permission without a mode is refused.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionModes {
    #[serde(default)]
    pub read_only: Option<String>,
    #[serde(default)]
    pub workspace_write: Option<String>,
    #[serde(default)]
    pub autonomous_workspace: Option<String>,
}

impl PermissionModes {
    pub fn for_mode(&self, mode: TaskPermissions) -> Option<&str> {
        match mode {
            TaskPermissions::ReadOnly => self.read_only.as_deref(),
            TaskPermissions::WorkspaceWrite => self.workspace_write.as_deref(),
            TaskPermissions::AutonomousWorkspace => self.autonomous_workspace.as_deref(),
        }
    }
}

/// ACP server command and how models and permissions map onto sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcpSpec {
    /// e.g. `["acp"]`.
    pub args: Vec<String>,
    /// Process-level model selection (`["--model", "{model}"]`): one
    /// server per model. Leave empty when `model_option` is used.
    #[serde(default)]
    pub model_args: Vec<String>,
    /// Session config option that selects the model (`"model"`): one
    /// server for every model.
    #[serde(default)]
    pub model_option: Option<String>,
    /// Session config option that selects the mode.
    #[serde(default = "default_mode_option")]
    pub mode_option: String,
    pub modes: PermissionModes,
    /// A server idle this long is stopped.
    #[serde(default = "default_idle")]
    pub idle_secs: u64,
}

fn default_mode_option() -> String {
    "mode".to_owned()
}

const fn default_idle() -> u64 {
    900
}

/// A long-lived local server the CLI attaches to (`opencode serve`), so a
/// task does not cold-boot the harness. `{port}` is substituted in `args`
/// and `{url}` in `attach_args`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerSpec {
    pub args: Vec<String>,
    pub port: u16,
    pub attach_args: Vec<String>,
    /// Environment variable carrying a per-start random password to both
    /// the server and each attached run.
    #[serde(default)]
    pub password_env: Option<String>,
}

/// Arguments that put the harness in each permission mode. A mode with no
/// entry is refused rather than run with the harness's default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionArgs {
    #[serde(default)]
    pub read_only: Option<Vec<String>>,
    #[serde(default)]
    pub workspace_write: Option<Vec<String>>,
    #[serde(default)]
    pub autonomous_workspace: Option<Vec<String>>,
}

impl PermissionArgs {
    pub fn for_mode(&self, mode: TaskPermissions) -> Option<&[String]> {
        match mode {
            TaskPermissions::ReadOnly => self.read_only.as_deref(),
            TaskPermissions::WorkspaceWrite => self.workspace_write.as_deref(),
            TaskPermissions::AutonomousWorkspace => self.autonomous_workspace.as_deref(),
        }
    }
}

/// Billing for models whose id matches `models` (glob, `*` = any run of
/// characters). The first matching rule wins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BillingRule {
    pub models: String,
    pub billing: AgentBilling,
}

/// One executor as written in configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutorConfig {
    /// Name used in model refs (`devin:opus`). `[A-Za-z0-9_-]`, 1–32.
    pub name: String,
    pub adapter: AdapterKind,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub description: Option<String>,
    /// Binary (name on PATH or absolute path). Preset default otherwise.
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub discover: Option<DiscoverSpec>,
    #[serde(default)]
    pub run: Option<RunSpec>,
    #[serde(default)]
    pub permission_args: Option<PermissionArgs>,
    /// Appended to every run (e.g. `["--attach", "http://127.0.0.1:4096"]`).
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// Environment variables passed through to the harness *by name*. The
    /// harness otherwise sees a minimal environment, so resident secrets
    /// (node token, provider keys) never reach it.
    #[serde(default)]
    pub env_passthrough: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_discover_timeout")]
    pub discover_timeout_secs: u64,
    /// How long a discovered model list is trusted.
    #[serde(default = "default_catalog_ttl")]
    pub catalog_ttl_secs: u64,
    #[serde(default)]
    pub billing: Vec<BillingRule>,
    /// Billing for models no rule matches and whose price is not
    /// discoverable. `unknown` unless declared.
    #[serde(default)]
    pub default_billing: Option<AgentBilling>,
    /// Permit metered models when asked for by name. Routing never picks
    /// them either way.
    #[serde(default)]
    pub allow_metered: bool,
    #[serde(default = "default_concurrency")]
    pub max_concurrent: usize,
    /// Task kinds routing sends here first. Preset default otherwise.
    #[serde(default)]
    pub prefer_for: Option<Vec<TaskKind>>,
    /// Model used when routing picks this executor. Without it, routing
    /// uses the harness default model only if `default_billing` is
    /// auto-eligible.
    #[serde(default)]
    pub default_model: Option<String>,
    /// `cli` (default) or `acp`.
    #[serde(default)]
    pub protocol: Option<Protocol>,
    /// Overrides the preset's ACP settings.
    #[serde(default)]
    pub acp: Option<AcpSpec>,
    /// Keep the preset's (or this) server running and attach runs to it.
    #[serde(default)]
    pub serve: bool,
    #[serde(default)]
    pub server: Option<ServerSpec>,
    /// Usage limits for this executor (see the resident's cost ledger).
    #[serde(default)]
    pub quota: Option<Quota>,
}

const fn default_true() -> bool {
    true
}
const fn default_timeout() -> u64 {
    1800
}
const fn default_discover_timeout() -> u64 {
    90
}
const fn default_catalog_ttl() -> u64 {
    6 * 3600
}
const fn default_concurrency() -> usize {
    1
}

/// A configuration with its preset applied: every field the executor
/// needs, resolved.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExecutorSpec {
    pub name: String,
    pub adapter: AdapterKind,
    pub enabled: bool,
    pub description: Option<String>,
    pub command: String,
    pub discover: Option<DiscoverSpec>,
    pub run: RunSpec,
    pub permission_args: PermissionArgs,
    pub extra_args: Vec<String>,
    pub env_passthrough: Vec<String>,
    pub timeout_secs: u64,
    pub discover_timeout_secs: u64,
    pub catalog_ttl_secs: u64,
    pub billing: Vec<BillingRule>,
    pub default_billing: AgentBilling,
    pub allow_metered: bool,
    pub max_concurrent: usize,
    pub prefer_for: Vec<TaskKind>,
    pub default_model: Option<String>,
    pub protocol: Protocol,
    /// Present when `protocol` is `acp`.
    pub acp: Option<AcpSpec>,
    /// Present when `serve` is on.
    pub server: Option<ServerSpec>,
    pub quota: Quota,
}

/// What a preset supplies.
pub struct Preset {
    pub command: &'static str,
    pub discover: Option<DiscoverSpec>,
    pub run: Option<RunSpec>,
    pub permission_args: PermissionArgs,
    pub prefer_for: Vec<TaskKind>,
    pub acp: Option<AcpSpec>,
    pub server: Option<ServerSpec>,
}

pub(crate) fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

fn preset(adapter: AdapterKind) -> Preset {
    match adapter {
        AdapterKind::DevinCli => super::devin::preset(),
        AdapterKind::Opencode => super::opencode::preset(),
        AdapterKind::CommandCode => super::commandcode::preset(),
        AdapterKind::Generic => Preset {
            command: "",
            discover: None,
            run: None,
            permission_args: PermissionArgs::default(),
            prefer_for: Vec::new(),
            acp: None,
            server: None,
        },
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
}

impl ExecutorConfig {
    /// Apply the preset and validate.
    pub fn resolve(&self) -> Result<ExecutorSpec, String> {
        let name = &self.name;
        if !valid_name(name) {
            return Err(format!(
                "executor name {name:?} must be 1-32 chars of [A-Za-z0-9_-]"
            ));
        }
        let preset = preset(self.adapter);
        let command = self
            .command
            .clone()
            .unwrap_or_else(|| preset.command.to_owned());
        if command.trim().is_empty() {
            return Err(format!("executor {name}: command is required"));
        }
        let run = self
            .run
            .clone()
            .or(preset.run)
            .ok_or_else(|| format!("executor {name}: run is required for a generic adapter"))?;
        if !run.args.iter().any(|a| a.contains("{prompt}")) {
            return Err(format!("executor {name}: run.args must contain {{prompt}}"));
        }
        if !run.model_args.is_empty() && !run.model_args.iter().any(|a| a.contains("{model}")) {
            return Err(format!(
                "executor {name}: run.model_args must contain {{model}}"
            ));
        }
        let permission_args = self
            .permission_args
            .clone()
            .unwrap_or(preset.permission_args);
        if permission_args.read_only.is_none() {
            return Err(format!(
                "executor {name}: permission_args.read_only must be declared (use [] if the harness is read-only by default)"
            ));
        }
        if self.timeout_secs == 0
            || self.discover_timeout_secs == 0
            || self.catalog_ttl_secs == 0
            || self.max_concurrent == 0
        {
            return Err(format!(
                "executor {name}: timeouts, ttl and max_concurrent must be positive"
            ));
        }
        for rule in &self.billing {
            if rule.models.trim().is_empty() {
                return Err(format!("executor {name}: empty billing pattern"));
            }
        }
        for var in &self.env_passthrough {
            if var.is_empty() || !var.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return Err(format!("executor {name}: bad env_passthrough {var:?}"));
            }
        }
        let protocol = self.protocol.unwrap_or_default();
        let acp = match protocol {
            Protocol::Cli => None,
            Protocol::Acp => {
                let acp =
                    self.acp.clone().or(preset.acp).ok_or_else(|| {
                        format!("executor {name}: protocol acp needs an acp section")
                    })?;
                if acp.modes.read_only.is_none() || acp.idle_secs == 0 {
                    return Err(format!(
                        "executor {name}: acp.modes.read_only and a positive idle_secs are required"
                    ));
                }
                Some(acp)
            }
        };
        let server = if self.serve {
            let server = self
                .server
                .clone()
                .or(preset.server)
                .ok_or_else(|| format!("executor {name}: serve needs a server section"))?;
            if server.port == 0 || !server.attach_args.iter().any(|a| a.contains("{url}")) {
                return Err(format!(
                    "executor {name}: server.port must be set and attach_args must contain {{url}}"
                ));
            }
            Some(server)
        } else {
            None
        };
        let quota = self.quota.clone().unwrap_or_default();
        quota
            .validate()
            .map_err(|e| format!("executor {name}: quota {e}"))?;
        Ok(ExecutorSpec {
            name: name.clone(),
            adapter: self.adapter,
            enabled: self.enabled,
            description: self.description.clone(),
            command,
            discover: self.discover.clone().or(preset.discover),
            run,
            permission_args,
            extra_args: self.extra_args.clone(),
            env_passthrough: self.env_passthrough.clone(),
            timeout_secs: self.timeout_secs,
            discover_timeout_secs: self.discover_timeout_secs,
            catalog_ttl_secs: self.catalog_ttl_secs,
            billing: self.billing.clone(),
            default_billing: self.default_billing.unwrap_or(AgentBilling::Unknown),
            allow_metered: self.allow_metered,
            max_concurrent: self.max_concurrent,
            prefer_for: self.prefer_for.clone().unwrap_or(preset.prefer_for),
            default_model: self.default_model.clone(),
            protocol,
            acp,
            server,
            quota,
        })
    }
}

/// Resolve a whole executor list; names must be unique.
pub fn resolve_all(configs: &[ExecutorConfig]) -> Result<Vec<ExecutorSpec>, String> {
    let mut seen = std::collections::HashSet::new();
    configs
        .iter()
        .map(|c| {
            if !seen.insert(c.name.as_str()) {
                return Err(format!("executor {:?} is declared twice", c.name));
            }
            c.resolve()
        })
        .collect()
}

/// `*`-glob match over the whole string.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !text.starts_with(first) || !text[first.len()..].ends_with(last) {
        return false;
    }
    let mut rest = &text[first.len()..text.len() - last.len()];
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(json: serde_json::Value) -> ExecutorConfig {
        serde_json::from_value(json).expect("config parses")
    }

    #[test]
    fn presets_resolve_without_further_configuration() {
        for (name, adapter) in [
            ("devin", "devin_cli"),
            ("opencode", "opencode"),
            ("commandcode", "command_code"),
        ] {
            let spec = config(serde_json::json!({"name": name, "adapter": adapter}))
                .resolve()
                .expect(name);
            assert!(spec.discover.is_some(), "{name} discovers models");
            assert!(spec.permission_args.read_only.is_some());
            assert_eq!(spec.default_billing, AgentBilling::Unknown);
        }
    }

    #[test]
    fn a_generic_executor_is_configuration_only() {
        let spec = config(serde_json::json!({
            "name": "aider", "adapter": "generic", "command": "aider",
            "discover": {"args": ["--list-models", "*"], "format": "lines"},
            "run": {"args": ["--message", "{prompt}"], "model_args": ["--model", "{model}"],
                    "output": "text"},
            "permission_args": {"read_only": ["--dry-run"]},
            "billing": [{"models": "ollama/*", "billing": "local"}]
        }))
        .resolve()
        .expect("generic resolves");
        assert_eq!(spec.command, "aider");
        // A generic executor without a run line or read-only mode is refused.
        assert!(
            config(serde_json::json!({"name": "x", "adapter": "generic", "command": "x"}))
                .resolve()
                .is_err()
        );
        assert!(
            config(
                serde_json::json!({"name": "x", "adapter": "generic", "command": "x",
                "run": {"args": ["{prompt}"], "output": "text"}})
            )
            .resolve()
            .is_err()
        );
    }

    #[test]
    fn names_are_validated_and_unique() {
        let a = config(serde_json::json!({"name": "a", "adapter": "opencode"}));
        assert!(resolve_all(&[a.clone(), a.clone()]).is_err());
        let mut bad = a;
        bad.name = "a:b".to_owned();
        assert!(bad.resolve().is_err());
    }

    #[test]
    fn globs() {
        assert!(glob_match("opencode/*-free", "opencode/mimo-v2.5-free"));
        assert!(!glob_match("opencode/*-free", "opencode/claude-opus-5"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("lmstudio/*", "lmstudio/qwen"));
        assert!(glob_match("a*b*c", "a-x-b-y-c"));
        assert!(!glob_match("a*b*c", "a-x-c"));
        assert!(glob_match("exact", "exact"));
        assert!(!glob_match("ab*ba", "aba"));
    }
}
