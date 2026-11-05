//! Vocabulary shared by every task executor: model references, billing,
//! task kinds, permissions, the task envelope and what a run reports.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::route_gate::RouteLane;

/// A model *as served by one harness*. `devin:opus` and
/// `opencode:anthropic/claude-opus-5-5` are different candidates even when
/// the weights are the same: tool use, prompts and caching differ per
/// harness, and that difference is what gets measured.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelRef {
    /// Executor name from configuration (`devin`, `opencode`, …).
    pub executor: String,
    /// Upstream provider when the model id names one (`anthropic` in
    /// `anthropic/claude-opus-5-5`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Model id exactly as the harness accepts it (`opus`,
    /// `anthropic/claude-opus-5-5`, `deepseek/deepseek-v4-flash`).
    pub model: String,
}

impl ModelRef {
    pub fn new(executor: &str, model: &str) -> Self {
        Self {
            executor: executor.to_owned(),
            provider: model
                .rsplit_once('/')
                .map(|(provider, _)| provider.to_owned()),
            model: model.to_owned(),
        }
    }

    /// `executor:model`. The model part may itself contain `/` (and `:`).
    pub fn parse(text: &str) -> Option<Self> {
        let (executor, model) = text.trim().split_once(':')?;
        (!executor.is_empty() && !model.is_empty()).then(|| Self::new(executor, model))
    }
}

impl fmt::Display for ModelRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.executor, self.model)
    }
}

/// How using a model is paid for. Unlike chat tiers, agent harnesses
/// often sit on top of other providers (`opencode` is free, the provider
/// underneath may not be), so the class belongs to the model, not the
/// harness, and an unknown class is never treated as free.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum AgentBilling {
    /// Runs on hardware Kamimusuhi owns.
    Local,
    /// Provider free tier (rate limits may apply).
    FreeTier,
    /// Flat subscription already paid for.
    Subscription,
    /// Credits bundled with a plan: finite, not unlimited.
    IncludedCredit,
    /// Per-token billing.
    Metered,
    /// Not declared and not discoverable. Never shown as zero cost.
    Unknown,
}

impl AgentBilling {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::FreeTier => "free_tier",
            Self::Subscription => "subscription",
            Self::IncludedCredit => "included_credit",
            Self::Metered => "metered",
            Self::Unknown => "unknown",
        }
    }

    /// Whether automatic routing (and, later, fan-out) may pick it. Metered
    /// and unknown models are only used when asked for by name.
    pub const fn auto_eligible(self) -> bool {
        matches!(
            self,
            Self::Local | Self::FreeTier | Self::Subscription | Self::IncludedCredit
        )
    }

    /// Automatic routing order: cheapest marginal cost first.
    pub const fn rank(self) -> u8 {
        match self {
            Self::Local => 0,
            Self::FreeTier => 1,
            Self::Subscription | Self::IncludedCredit => 2,
            Self::Metered => 3,
            Self::Unknown => 4,
        }
    }
}

/// Capability hints, taken from discovery metadata where a harness reports
/// it and from the model id otherwise. Hints steer routing; they are never
/// a guarantee.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub coding: bool,
    pub reasoning: bool,
    pub vision: bool,
    pub tools: bool,
    pub long_context: bool,
    pub fast: bool,
}

/// One model an executor can run, as discovered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelDescriptor {
    pub id: ModelRef,
    /// Other names the harness accepts for the same model (`opus`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub capabilities: ModelCapabilities,
    pub billing: AgentBilling,
    /// Why `billing` has its value: `rule:<pattern>`, `discovered_price`,
    /// `executor_default`.
    pub billing_source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<u64>,
    /// Price or cost note as the harness reported it, verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_note: Option<String>,
    /// USD per million (input, output) tokens when the harness lists what
    /// this account pays; used to estimate unreported cost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usd_per_mtok: Option<(f64, f64)>,
    pub available: bool,
    pub discovered_at: u64,
}

impl ModelDescriptor {
    /// Whether `name` (model id or alias) refers to this model.
    pub fn answers_to(&self, name: &str) -> bool {
        self.id.model == name || self.aliases.iter().any(|a| a == name)
    }
}

/// What a delegated task is for. Maps onto the existing agent lanes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Research,
    Coding,
    Review,
    Debug,
    Planning,
    Background,
    General,
}

impl TaskKind {
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "research" => Self::Research,
            "coding" => Self::Coding,
            "review" => Self::Review,
            "debug" => Self::Debug,
            "planning" => Self::Planning,
            "background" => Self::Background,
            "general" => Self::General,
            _ => return None,
        })
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Research => "research",
            Self::Coding => "coding",
            Self::Review => "review",
            Self::Debug => "debug",
            Self::Planning => "planning",
            Self::Background => "background",
            Self::General => "general",
        }
    }

    /// The agent lane this kind of task belongs to.
    pub const fn lane(self) -> RouteLane {
        match self {
            Self::Coding | Self::Review | Self::Debug => RouteLane::CodeAgent,
            Self::Research | Self::Planning => RouteLane::ResearchAgent,
            Self::Background | Self::General => RouteLane::BackgroundAgent,
        }
    }
}

/// What an external agent may do in its workspace. Read-only is the
/// default; anything that writes is granted per task, explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskPermissions {
    /// Read, search, analyze.
    #[default]
    ReadOnly,
    /// Also edit files, run shell commands and tests in the workspace.
    WorkspaceWrite,
    /// Everything the harness allows without prompting (`--yolo` and
    /// friends). Never chosen by routing; only an explicit request.
    AutonomousWorkspace,
}

impl TaskPermissions {
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "read_only" => Self::ReadOnly,
            "workspace_write" => Self::WorkspaceWrite,
            "autonomous_workspace" => Self::AutonomousWorkspace,
            _ => return None,
        })
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::WorkspaceWrite => "workspace_write",
            Self::AutonomousWorkspace => "autonomous_workspace",
        }
    }
}

/// The task envelope: everything an external agent is given. It carries
/// the objective and the material needed for it — never the individual's
/// self model, private conversation, persona seed or any credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRequest {
    pub task_id: String,
    pub kind: TaskKind,
    pub objective: String,
    #[serde(default)]
    pub success_criteria: Vec<String>,
    /// Working directory the harness runs in.
    pub workspace: PathBuf,
    /// Excerpts the caller chose to pass along (already bounded).
    #[serde(default)]
    pub context: Vec<String>,
    #[serde(default)]
    pub constraints: Vec<String>,
    pub permissions: TaskPermissions,
    /// `None` lets the harness use its own default model.
    #[serde(default)]
    pub model: Option<String>,
    /// Continue this harness session instead of starting a new one.
    #[serde(default)]
    pub resume_session: Option<String>,
}

/// Usage limits. Absent limits do not apply; all limits count tasks from
/// the moment they start.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quota {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tasks_per_day: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tasks_per_month: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_usd_per_day: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_usd_per_month: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens_per_day: Option<u64>,
}

impl Quota {
    pub fn validate(&self) -> Result<(), String> {
        let ok = |v: Option<f64>| v.is_none_or(|v| v.is_finite() && v >= 0.0);
        if ok(self.max_usd_per_day) && ok(self.max_usd_per_month) {
            Ok(())
        } else {
            Err("usd limits must be finite and non-negative".to_owned())
        }
    }

    /// A metered model needs an explicit spending budget.
    pub const fn has_usd_budget(&self) -> bool {
        self.max_usd_per_day.is_some() || self.max_usd_per_month.is_some()
    }

    pub const fn is_empty(&self) -> bool {
        self.max_tasks_per_day.is_none()
            && self.max_tasks_per_month.is_none()
            && self.max_usd_per_day.is_none()
            && self.max_usd_per_month.is_none()
            && self.max_tokens_per_day.is_none()
    }
}

/// Token usage as reported by a harness. Absent fields were not reported.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
}

impl TaskUsage {
    pub const fn is_empty(&self) -> bool {
        self.input_tokens.is_none()
            && self.output_tokens.is_none()
            && self.cache_read_tokens.is_none()
            && self.cache_write_tokens.is_none()
    }

    pub fn add(&mut self, other: Self) {
        fn sum(a: Option<u64>, b: Option<u64>) -> Option<u64> {
            match (a, b) {
                (None, None) => None,
                (a, b) => Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0))),
            }
        }
        self.input_tokens = sum(self.input_tokens, other.input_tokens);
        self.output_tokens = sum(self.output_tokens, other.output_tokens);
        self.cache_read_tokens = sum(self.cache_read_tokens, other.cache_read_tokens);
        self.cache_write_tokens = sum(self.cache_write_tokens, other.cache_write_tokens);
    }
}

/// Something that happened during a run, for the task board.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum TaskEvent {
    /// The harness reported its session id.
    Session { id: String },
    /// A tool started or finished inside the harness.
    Tool { name: String, finished: bool },
    /// Free-form progress line (step boundaries, notices).
    Note { text: String },
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunEnd {
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
}

/// What a finished run produced. This is external content: it becomes
/// evidence, never a belief, and never touches canonical memory directly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskOutcome {
    pub end: RunEnd,
    /// Final answer text (bounded).
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub usage: TaskUsage,
    /// USD as reported by the harness. `None` = not reported, which is
    /// not the same as free.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_cost_usd: Option<f64>,
    /// Tool names the harness used, in order (bounded).
    #[serde(default)]
    pub tool_activity: Vec<String>,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
}

/// Whether an executor can take work right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutorHealth {
    pub executor: String,
    pub adapter: String,
    pub enabled: bool,
    /// Resolved binary, when found.
    pub binary: Option<String>,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_refs_keep_the_harness_and_the_provider() {
        let r = ModelRef::parse("opencode:anthropic/claude-opus-5-5").expect("parses");
        assert_eq!(r.executor, "opencode");
        assert_eq!(r.provider.as_deref(), Some("anthropic"));
        assert_eq!(r.model, "anthropic/claude-opus-5-5");
        assert_eq!(r.to_string(), "opencode:anthropic/claude-opus-5-5");
        let alias = ModelRef::parse("devin:opus").expect("parses");
        assert_eq!(alias.provider, None);
        assert!(ModelRef::parse("opus").is_none());
        assert!(ModelRef::parse(":opus").is_none());
        // Same weights through another harness are another candidate.
        assert_ne!(
            ModelRef::new("devin", "opus"),
            ModelRef::new("opencode", "opus")
        );
    }

    #[test]
    fn unknown_and_metered_are_never_auto_eligible() {
        assert!(AgentBilling::FreeTier.auto_eligible());
        assert!(AgentBilling::IncludedCredit.auto_eligible());
        assert!(!AgentBilling::Metered.auto_eligible());
        assert!(!AgentBilling::Unknown.auto_eligible());
    }

    #[test]
    fn task_kinds_map_onto_agent_lanes() {
        for kind in [
            "research",
            "coding",
            "review",
            "debug",
            "planning",
            "background",
            "general",
        ] {
            let k = TaskKind::parse(kind).expect(kind);
            assert_eq!(k.as_str(), kind);
            assert_eq!(
                k.lane().plane(),
                crate::route_gate::WorkPlane::Task,
                "{kind}"
            );
        }
    }
}
