//! Social embodiment supervisor, policy gate, durable outbox, and initiative engine.
//!
//! Enforces the closed loop:
//! `Observe -> Reason -> Propose -> Authorize -> Act -> Verify -> Persist`
//!
//! No generated reply is treated as spoken until the gateway verifies external
//! delivery (`verify`), and no proactive message leaves without a verifiable
//! internal trigger (`InitiativeEngine`) and `SocialPolicyGate` authorization.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::commitments::CommitmentState;
use crate::config::SocialConfig;
use crate::state::Shared;
use crate::util::{atomic_write, iso8601, unix_now, unix_now_ms, utc_date};

const KEEP_OUTBOX: usize = 200;
const KEEP_TRIGGERS: usize = 500;
const RETENTION_INTERVAL_SECS: u64 = 300;
const SECRET_MARKERS: &[&str] = &[
    "xoxb-",
    "xapp-",
    "sk-",
    "ghp_",
    "github_pat_",
    "DISCORD_BOT_TOKEN",
    "KAMIMUSUHI_NODE_TOKEN",
    "/srv/kamimusuhi/config/secrets.env",
    "/srv/kamimusuhi/private/",
];
const PRIVATE_PUBLIC_MARKERS: &[&str] = &[
    "/srv/kamimusuhi/runtime/",
    "secrets.env",
    "[reply to:",
    "private_memory",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SocialKind {
    Reactive,
    Initiative,
}

impl SocialKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reactive => "reactive",
            Self::Initiative => "initiative",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboxState {
    Proposed,
    Authorized,
    Persisted,
    Rejected,
    DeliveryFailed,
}

impl OutboxState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Authorized => "authorized",
            Self::Persisted => "persisted",
            Self::Rejected => "rejected",
            Self::DeliveryFailed => "delivery_failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitiativeTrigger {
    pub trigger_id: String,
    pub trigger_kind: String,
    pub reason: String,
    pub source_ref: String,
    pub evidence_summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocialCandidate {
    pub kind: SocialKind,
    pub guild_id: Option<u64>,
    pub channel_id: u64,
    pub user_id: Option<u64>,
    pub mention_user_id: Option<u64>,
    pub visibility: String,
    pub text: Option<String>,
    pub social_memory: Option<Value>,
    pub initiative: Option<InitiativeTrigger>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizationReceipt {
    pub receipt_id: String,
    pub decision: String,
    pub channel_id: u64,
    pub visibility: String,
    pub authorized_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyRejection {
    pub code: &'static str,
    pub detail: String,
}

impl PolicyRejection {
    fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SocialOutboxItem {
    pub proposal_id: String,
    pub kind: SocialKind,
    pub state: OutboxState,
    pub subject: String,
    #[serde(default)]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub guild_id: Option<u64>,
    pub channel_id: u64,
    #[serde(default)]
    pub reply_to_message_id: Option<u64>,
    #[serde(default)]
    pub mention_user_id: Option<u64>,
    pub visibility: String,
    pub response: String,
    pub response_digest: String,
    #[serde(default)]
    pub background: Value,
    #[serde(default)]
    pub initiative: Option<InitiativeTrigger>,
    #[serde(default)]
    pub receipt: Option<AuthorizationReceipt>,
    #[serde(default)]
    pub discord_message_id: Option<u64>,
    #[serde(default)]
    pub emitted_evidence_id: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SocialPersistedState {
    #[serde(default)]
    pub outbox: Vec<SocialOutboxItem>,
    #[serde(default)]
    pub seen_triggers: VecDeque<String>,
    #[serde(default)]
    pub reactive_last_at: BTreeMap<u64, u64>,
    #[serde(default)]
    pub reactive_hour_history: BTreeMap<u64, Vec<u64>>,
    #[serde(default)]
    pub initiative_last_at: Option<u64>,
    #[serde(default)]
    pub initiative_daily_date: String,
    #[serde(default)]
    pub initiative_daily_count: u64,
    #[serde(default)]
    pub last_retention_run_at: u64,
    #[serde(default)]
    pub last_supervisor_at: u64,
    #[serde(default)]
    pub last_supervisor_error: Option<String>,
}

pub struct SocialPolicyGate;

impl SocialPolicyGate {
    fn check_static(
        config: &SocialConfig,
        state: &SocialPersistedState,
        candidate: &SocialCandidate,
        now: u64,
    ) -> Result<(), PolicyRejection> {
        if !config.enabled {
            return Err(PolicyRejection::new(
                "SOCIAL_POLICY_DISABLED",
                "social embodiment is disabled",
            ));
        }
        // Destination: no DMs, guild must match when configured, channel must be allowlisted.
        let Some(guild_id) = candidate.guild_id else {
            return Err(PolicyRejection::new(
                "SOCIAL_POLICY_DM_FORBIDDEN",
                "direct messages are forbidden",
            ));
        };
        if let Some(expected_guild) = config.guild_id
            && expected_guild != guild_id
        {
            return Err(PolicyRejection::new(
                "SOCIAL_POLICY_GUILD_MISMATCH",
                format!("guild {guild_id} does not match configured guild"),
            ));
        }
        if config.allowed_channel_ids.is_empty()
            || !config.allowed_channel_ids.contains(&candidate.channel_id)
        {
            return Err(PolicyRejection::new(
                "SOCIAL_POLICY_CHANNEL_NOT_ALLOWED",
                format!("channel {} is not allowlisted", candidate.channel_id),
            ));
        }
        if candidate.kind == SocialKind::Initiative
            && !config
                .initiative_channel_ids
                .contains(&candidate.channel_id)
        {
            return Err(PolicyRejection::new(
                "SOCIAL_POLICY_INITIATIVE_CHANNEL_NOT_ALLOWED",
                format!(
                    "channel {} is not enabled for proactive initiative",
                    candidate.channel_id
                ),
            ));
        }

        // Authority & prohibitions.
        match candidate.kind {
            SocialKind::Reactive => {
                let allowed = candidate.user_id.is_some_and(|uid| {
                    !config.allowed_user_ids.is_empty() && config.allowed_user_ids.contains(&uid)
                });
                if !allowed {
                    return Err(PolicyRejection::new(
                        "SOCIAL_POLICY_USER_NOT_ALLOWED",
                        "user is not in allowed_user_ids",
                    ));
                }
            }
            SocialKind::Initiative => {
                let Some(init) = &candidate.initiative else {
                    return Err(PolicyRejection::new(
                        "SOCIAL_POLICY_MISSING_INITIATIVE_BASIS",
                        "initiative trigger is required",
                    ));
                };
                if matches!(
                    init.trigger_kind.as_str(),
                    "activity_padding" | "idle_timer" | "keepalive" | "heartbeat" | "filler"
                ) {
                    return Err(PolicyRejection::new(
                        "SOCIAL_POLICY_ACTIVITY_PADDING_FORBIDDEN",
                        "activity padding triggers are forbidden",
                    ));
                }
                if init.trigger_kind == "relationship_analysis"
                    || contains_relationship_profiling(&init.reason)
                {
                    return Err(PolicyRejection::new(
                        "SOCIAL_POLICY_RELATIONSHIP_PROFILING_FORBIDDEN",
                        "automated human relationship analysis is forbidden",
                    ));
                }
                if !matches!(
                    init.trigger_kind.as_str(),
                    "commitment_outcome"
                        | "delegated_review"
                        | "research_finding"
                        | "self_goal_progress"
                ) || init.trigger_id.trim().is_empty()
                    || init.reason.trim().is_empty()
                    || init.source_ref.trim().is_empty()
                    || init.evidence_summary.trim().is_empty()
                {
                    return Err(PolicyRejection::new(
                        "SOCIAL_POLICY_MISSING_INITIATIVE_BASIS",
                        "initiative requires a valid trigger kind, source_ref, reason, and evidence_summary",
                    ));
                }
                if state.seen_triggers.iter().any(|t| t == &init.trigger_id) {
                    return Err(PolicyRejection::new(
                        "SOCIAL_POLICY_DUPLICATE_INITIATIVE_TRIGGER",
                        format!("trigger {} was already processed", init.trigger_id),
                    ));
                }
            }
        }

        if let Some(mention_uid) = candidate.mention_user_id {
            let allowed_mention = config.allowed_user_ids.contains(&mention_uid)
                || config.operator_user_id == Some(mention_uid);
            if !allowed_mention {
                return Err(PolicyRejection::new(
                    "SOCIAL_POLICY_MENTION_NOT_ALLOWED",
                    format!("mention target {mention_uid} is not permitted"),
                ));
            }
        }

        // Frequency control.
        match candidate.kind {
            SocialKind::Reactive => {
                if let Some(&last) = state.reactive_last_at.get(&candidate.channel_id)
                    && now.saturating_sub(last) < config.reactive_min_interval_secs
                {
                    return Err(PolicyRejection::new(
                        "SOCIAL_POLICY_REACTIVE_RATE_LIMITED",
                        "reactive channel cooldown active",
                    ));
                }
                let cutoff = now.saturating_sub(3600);
                let recent_count = state
                    .reactive_hour_history
                    .get(&candidate.channel_id)
                    .map_or(0, |times| times.iter().filter(|&&t| t >= cutoff).count());
                if recent_count >= config.reactive_max_per_hour as usize {
                    return Err(PolicyRejection::new(
                        "SOCIAL_POLICY_REACTIVE_RATE_LIMITED",
                        "reactive hourly channel limit reached",
                    ));
                }
            }
            SocialKind::Initiative => {
                if let Some(last) = state.initiative_last_at
                    && now.saturating_sub(last) < config.initiative_min_interval_secs
                {
                    return Err(PolicyRejection::new(
                        "SOCIAL_POLICY_INITIATIVE_RATE_LIMITED",
                        "initiative cooldown active",
                    ));
                }
                let today = utc_date(now);
                if state.initiative_daily_date == today
                    && state.initiative_daily_count >= config.initiative_max_per_day
                {
                    return Err(PolicyRejection::new(
                        "SOCIAL_POLICY_INITIATIVE_RATE_LIMITED",
                        "initiative daily budget reached",
                    ));
                }
            }
        }

        // Content & public-information inspection.
        if let Some(text) = &candidate.text {
            if text.trim().is_empty() {
                return Err(PolicyRejection::new(
                    "SOCIAL_POLICY_EMPTY_TEXT",
                    "proposal text cannot be empty",
                ));
            }
            if contains_relationship_profiling(text) {
                return Err(PolicyRejection::new(
                    "SOCIAL_POLICY_RELATIONSHIP_PROFILING_FORBIDDEN",
                    "automated human relationship analysis is forbidden",
                ));
            }
            if SECRET_MARKERS.iter().any(|marker| text.contains(marker)) {
                return Err(PolicyRejection::new(
                    "SOCIAL_POLICY_SECRET_LEAK_BLOCKED",
                    "candidate contains forbidden credential or private path marker",
                ));
            }
            let is_public = config.public_channel_ids.contains(&candidate.channel_id)
                || candidate.visibility == "public";
            if is_public {
                if let Some(sm) = &candidate.social_memory {
                    let vis = sm.get("visibility").and_then(Value::as_str);
                    let rel_count = sm
                        .pointer("/self_history/relationship_memories")
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    if vis != Some("public") || rel_count > 0 {
                        return Err(PolicyRejection::new(
                            "SOCIAL_POLICY_PRIVATE_MEMORY_IN_PUBLIC_CHANNEL",
                            "public channel candidate cannot rely on private or relationship memory",
                        ));
                    }
                }
                if PRIVATE_PUBLIC_MARKERS.iter().any(|m| text.contains(m)) {
                    return Err(PolicyRejection::new(
                        "SOCIAL_POLICY_PRIVATE_MEMORY_IN_PUBLIC_CHANNEL",
                        "candidate text contains private-only markers",
                    ));
                }
            }
        }

        Ok(())
    }

    pub fn authorize_precheck(
        config: &SocialConfig,
        state: &SocialPersistedState,
        candidate: &SocialCandidate,
        now: u64,
    ) -> Result<(), PolicyRejection> {
        Self::check_static(config, state, candidate, now)
    }

    pub fn authorize_proposal(
        config: &SocialConfig,
        state: &mut SocialPersistedState,
        candidate: &SocialCandidate,
        receipt_id: String,
        now: u64,
    ) -> Result<AuthorizationReceipt, PolicyRejection> {
        if candidate.text.is_none() {
            return Err(PolicyRejection::new(
                "SOCIAL_POLICY_EMPTY_TEXT",
                "generated candidate text is required for authorization",
            ));
        }
        Self::check_static(config, state, candidate, now)?;

        match candidate.kind {
            SocialKind::Reactive => {
                state.reactive_last_at.insert(candidate.channel_id, now);
                let cutoff = now.saturating_sub(3600);
                let list = state
                    .reactive_hour_history
                    .entry(candidate.channel_id)
                    .or_default();
                list.retain(|&t| t >= cutoff);
                list.push(now);
            }
            SocialKind::Initiative => {
                let today = utc_date(now);
                if state.initiative_daily_date != today {
                    state.initiative_daily_date = today;
                    state.initiative_daily_count = 0;
                }
                state.initiative_daily_count += 1;
                state.initiative_last_at = Some(now);
                if let Some(init) = &candidate.initiative {
                    state.seen_triggers.push_back(init.trigger_id.clone());
                    while state.seen_triggers.len() > KEEP_TRIGGERS {
                        state.seen_triggers.pop_front();
                    }
                }
            }
        }

        let effective_visibility = if config.public_channel_ids.contains(&candidate.channel_id)
            || candidate.visibility == "public"
        {
            "public".to_owned()
        } else {
            "private".to_owned()
        };
        Ok(AuthorizationReceipt {
            receipt_id,
            decision: "authorized".to_owned(),
            channel_id: candidate.channel_id,
            visibility: effective_visibility,
            authorized_at: now,
        })
    }
}

fn contains_relationship_profiling(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    text.contains("人間関係分析")
        || lower.contains("relationship_profiling")
        || lower.contains("personality_scoring")
}

pub struct SocialEngine {
    path: PathBuf,
    config: Option<SocialConfig>,
    state: Mutex<SocialPersistedState>,
    seq: AtomicU64,
}

impl SocialEngine {
    pub fn load(path: PathBuf, config: Option<SocialConfig>) -> Self {
        let mut state: SocialPersistedState = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let now = unix_now();
        let mut reconciled = false;
        for item in &mut state.outbox {
            if item.state == OutboxState::Proposed {
                item.state = OutboxState::DeliveryFailed;
                item.error = Some("interrupted_before_authorization".to_owned());
                item.updated_at = now;
                reconciled = true;
            }
        }
        if reconciled {
            let _ = save_state(&path, &mut state);
        }
        Self {
            path,
            config,
            state: Mutex::new(state),
            seq: AtomicU64::new(0),
        }
    }

    fn next_id(&self, prefix: &str) -> String {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        format!("{prefix}-{:x}-{seq}", unix_now_ms())
    }

    pub fn summary(&self) -> Value {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
        for item in &state.outbox {
            *counts.entry(item.state.as_str()).or_insert(0) += 1;
        }
        let today = utc_date(unix_now());
        let daily_used = if state.initiative_daily_date == today {
            state.initiative_daily_count
        } else {
            0
        };
        json!({
            "enabled": self.config.as_ref().is_some_and(|c| c.enabled),
            "configured": self.config.is_some(),
            "outbox_counts": counts,
            "authorized_pending": state.outbox.iter().filter(|i| i.state == OutboxState::Authorized).count(),
            "initiative_daily_used": daily_used,
            "initiative_daily_limit": self.config.as_ref().map(|c| c.initiative_max_per_day),
            "last_retention_run_at": (state.last_retention_run_at > 0).then(|| iso8601(state.last_retention_run_at)),
            "last_supervisor_at": (state.last_supervisor_at > 0).then(|| iso8601(state.last_supervisor_at)),
            "last_supervisor_error": state.last_supervisor_error,
        })
    }
}

fn save_state(path: &Path, state: &mut SocialPersistedState) -> Result<(), String> {
    if state.outbox.len() > KEEP_OUTBOX {
        let mut kept_authorized: Vec<SocialOutboxItem> = state
            .outbox
            .iter()
            .filter(|item| item.state == OutboxState::Authorized)
            .cloned()
            .collect();
        let mut others: Vec<SocialOutboxItem> = state
            .outbox
            .iter()
            .filter(|item| item.state != OutboxState::Authorized)
            .cloned()
            .collect();
        let keep_others = KEEP_OUTBOX.saturating_sub(kept_authorized.len());
        if others.len() > keep_others {
            others.drain(..others.len() - keep_others);
        }
        kept_authorized.extend(others);
        kept_authorized.sort_by_key(|item| item.created_at);
        state.outbox = kept_authorized;
    }
    let bytes = serde_json::to_vec(state).map_err(|e| format!("serialize social state: {e}"))?;
    atomic_write(path, &bytes).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Collect verifiable internal triggers from commitments, delegated task reviews,
/// and reviewed experiment findings. Never fabricates triggers from idle time.
pub fn collect_due_triggers(
    shared: &Shared,
    config: &SocialConfig,
    state: &SocialPersistedState,
) -> Vec<InitiativeTrigger> {
    let mut triggers = Vec::new();
    let seen = |id: &str| state.seen_triggers.iter().any(|s| s == id);

    // 1. Commitment outcomes and verified progress milestones.
    for c in shared.commitments.list(true) {
        if matches!(
            c.state,
            CommitmentState::Done | CommitmentState::Abandoned | CommitmentState::Escalated
        ) {
            let trigger_id = format!("commitment:{}:{}", c.id, c.state.as_str());
            if !seen(&trigger_id) {
                triggers.push(InitiativeTrigger {
                    trigger_id,
                    trigger_kind: "commitment_outcome".to_owned(),
                    reason: format!(
                        "目標 {} が {} になったため報告します",
                        c.id,
                        c.state.as_str()
                    ),
                    source_ref: format!("commitment:{}", c.id),
                    evidence_summary: c
                        .outcome
                        .clone()
                        .or_else(|| c.question.clone())
                        .unwrap_or_else(|| c.goal.clone()),
                });
            }
        } else if c.state == CommitmentState::Active && !c.checkpoint.worked.is_empty() {
            let step_count = c.checkpoint.worked.len();
            let trigger_id = format!("self_goal:{}:step{step_count}", c.id);
            if !seen(&trigger_id) {
                let latest = c.checkpoint.worked.last().cloned().unwrap_or_default();
                triggers.push(InitiativeTrigger {
                    trigger_id,
                    trigger_kind: "self_goal_progress".to_owned(),
                    reason: format!("自己目標 {} の検証済み進捗を共有します", c.id),
                    source_ref: format!("commitment:{}", c.id),
                    evidence_summary: latest,
                });
            }
        }
    }

    // 2. Reviewed delegated tasks on the task board.
    for task in shared.tasks.list(false) {
        if task["kind"].as_str() != Some("delegated") {
            continue;
        }
        let status = task["status"].as_str().unwrap_or("");
        if !matches!(status, "done" | "failed") {
            continue;
        }
        let Some(verdict) = task["detail"]["review"]["verdict"].as_str() else {
            continue;
        };
        let Some(id) = task["id"].as_str() else {
            continue;
        };
        let trigger_id = format!("delegated:{id}:{status}");
        if !seen(&trigger_id) {
            triggers.push(InitiativeTrigger {
                trigger_id,
                trigger_kind: "delegated_review".to_owned(),
                reason: format!("委譲タスク {id} の評価結果（{verdict}）を報告します"),
                source_ref: format!("task:{id}"),
                evidence_summary: task["result"]["summary"]
                    .as_str()
                    .unwrap_or(task["title"].as_str().unwrap_or(""))
                    .to_owned(),
            });
        }
    }

    // 3. Reviewed research findings (if configured).
    if let Some(findings_path) = &config.findings_path
        && let Ok(bytes) = std::fs::read(findings_path)
        && let Ok(doc) = serde_json::from_slice::<Value>(&bytes)
    {
        let array = doc
            .as_array()
            .or_else(|| doc.get("findings").and_then(Value::as_array));
        if let Some(items) = array {
            for item in items {
                if item["status"].as_str() != Some("supported") {
                    continue;
                }
                let Some(id) = item["id"].as_str() else {
                    continue;
                };
                let rev = item["revision"].as_u64().unwrap_or(1);
                let trigger_id = format!("finding:{id}:rev{rev}");
                if !seen(&trigger_id) {
                    triggers.push(InitiativeTrigger {
                        trigger_id,
                        trigger_kind: "research_finding".to_owned(),
                        reason: format!("検証済み研究知見 {id}（rev {rev}）を共有します"),
                        source_ref: format!("research:{id}"),
                        evidence_summary: item["claim"]
                            .as_str()
                            .or_else(|| item["title"].as_str())
                            .unwrap_or("")
                            .to_owned(),
                    });
                }
            }
        }
    }

    triggers
}

/// Start the independent social supervisor thread (retention + initiative checks).
pub fn spawn(shared: &Arc<Shared>) {
    let Some(config) = shared.config.social.clone().filter(|c| c.enabled) else {
        return;
    };
    let shared = Arc::clone(shared);
    let _ = thread::Builder::new()
        .name("social-supervisor".to_owned())
        .spawn(move || {
            loop {
                thread::sleep(Duration::from_secs(config.poll_interval_secs));
                run_supervisor_tick(&shared, &config);
            }
        });
}

pub fn run_supervisor_tick(shared: &Shared, config: &SocialConfig) {
    let now = unix_now();
    let should_run_retention = {
        let state = shared
            .social
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        now.saturating_sub(state.last_retention_run_at) >= RETENTION_INTERVAL_SECS
    };
    if should_run_retention {
        let _ = run_retention_now(shared, config);
    }

    // Only attempt proactive LLM generation when foreground dialogue turns are idle.
    let turns_idle = shared
        .turns
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .is_empty();
    if turns_idle {
        let trigger_opt = {
            let state = shared
                .social
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            collect_due_triggers(shared, config, &state)
                .into_iter()
                .next()
        };
        if let Some(trigger) = trigger_opt
            && let Some(&channel_id) = config.initiative_channel_ids.first()
        {
            let _ =
                process_initiative(shared, config, trigger, channel_id, config.operator_user_id);
        }
    }

    let mut state = shared
        .social
        .state
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    state.last_supervisor_at = now;
    let _ = save_state(&shared.social.path, &mut state);
}

fn run_retention_now(shared: &Shared, config: &SocialConfig) -> (u16, Value) {
    let Some(dialogue) = &shared.config.dialogue else {
        return (
            503,
            json!({"error": "dialogue runtime is not configured on this node"}),
        );
    };
    let req = json!({
        "action": "social_retention",
        "max_age_secs": config.user_utterance_ttl_secs,
        "max_utterances_per_source": config.max_user_utterances_per_channel,
    });
    match crate::dialogue::run_social_host_action(shared, dialogue, &config.operator_subject, req) {
        Ok(res) => {
            let now = unix_now();
            let mut state = shared
                .social
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            state.last_retention_run_at = now;
            state.last_supervisor_error = None;
            let _ = save_state(&shared.social.path, &mut state);
            let _ = shared.spool.append_sync(
                "logs/social",
                json!({"event": "retention_enforced", "result": res, "at": iso8601(now)}),
            );
            (200, res)
        }
        Err((status, err)) => {
            let mut state = shared
                .social
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            state.last_supervisor_error = err["error"].as_str().map(str::to_owned);
            let _ = save_state(&shared.social.path, &mut state);
            (status, err)
        }
    }
}

fn effective_visibility(config: &SocialConfig, channel_id: u64, requested: Option<&str>) -> String {
    if config.public_channel_ids.contains(&channel_id) || requested == Some("public") {
        "public".to_owned()
    } else {
        "private".to_owned()
    }
}

pub fn handle(shared: &Shared, request: &Value) -> (u16, Value) {
    let action = request["action"].as_str().unwrap_or("status");
    let Some(config) = &shared.config.social else {
        return (
            404,
            json!({"error": "social embodiment is not configured on this node"}),
        );
    };
    match action {
        "status" => (200, shared.social.summary()),
        "outbox" => {
            let state = shared
                .social
                .state
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let items: Vec<Value> = state
                .outbox
                .iter()
                .filter(|item| item.state == OutboxState::Authorized)
                .map(|item| serde_json::to_value(item).unwrap_or(Value::Null))
                .collect();
            (200, json!({"items": items}))
        }
        "propose" => handle_propose(shared, config, request),
        "initiative" => handle_initiative(shared, config, request),
        "verify" => handle_verify(shared, config, request),
        "delete" => handle_delete(shared, config, request),
        "retention" => run_retention_now(shared, config),
        other => (
            400,
            json!({"error": format!("unknown social action {other:?}")}),
        ),
    }
}

fn handle_propose(shared: &Shared, config: &SocialConfig, request: &Value) -> (u16, Value) {
    let Some(dialogue) = &shared.config.dialogue else {
        return (
            503,
            json!({"error": "dialogue runtime is not configured on this node"}),
        );
    };
    let message = request["message"].as_str().unwrap_or("");
    if message.trim().is_empty() {
        return (400, json!({"error": "message is required"}));
    }
    let Some(channel_id) = request["channel_id"].as_u64() else {
        return (400, json!({"error": "channel_id is required"}));
    };
    let guild_id = request["guild_id"].as_u64();
    let user_id = request["user_id"].as_u64();
    let message_id = request["message_id"].as_u64();
    let mention_user_id = request["mention_user_id"].as_u64();
    let is_operator = request["is_operator"].as_bool().unwrap_or(false);
    let visibility = effective_visibility(config, channel_id, request["visibility"].as_str());
    let subject = request["subject"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(&config.operator_subject)
        .to_owned();

    let now = unix_now();
    let precheck = SocialCandidate {
        kind: SocialKind::Reactive,
        guild_id,
        channel_id,
        user_id,
        mention_user_id,
        visibility: visibility.clone(),
        text: None,
        social_memory: None,
        initiative: None,
    };
    {
        let state = shared
            .social
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Err(rej) = SocialPolicyGate::authorize_precheck(config, &state, &precheck, now) {
            let _ = shared.spool.append_sync(
                "logs/social",
                json!({"event": "policy_rejected_precheck", "kind": "reactive", "code": rej.code, "detail": rej.detail, "channel_id": channel_id, "at": iso8601(now)}),
            );
            return (
                403,
                json!({"authorized": false, "reason_code": rej.code, "error": rej.detail}),
            );
        }
    }

    let social_context = json!({
        "platform": "discord",
        "guild_id": guild_id,
        "channel_id": channel_id,
        "message_id": message_id,
        "user_id": user_id,
        "reply_to_message_id": message_id,
        "visibility": visibility,
        "is_operator": is_operator,
    });
    let (status, reply) = crate::dialogue::run_social_turn(
        shared,
        dialogue,
        &subject,
        message,
        &social_context,
        None,
    );
    if status != 200 {
        return (status, reply);
    }

    let response_text = reply["response"].as_str().unwrap_or("").to_owned();
    let response_digest = reply["response_digest"].as_str().unwrap_or("").to_owned();
    let turn_id = reply["turn_id"].as_str().map(str::to_owned);
    let social_memory = reply.get("social_memory").cloned();

    let candidate = SocialCandidate {
        kind: SocialKind::Reactive,
        guild_id,
        channel_id,
        user_id,
        mention_user_id,
        visibility: visibility.clone(),
        text: Some(response_text.clone()),
        social_memory: social_memory.clone(),
        initiative: None,
    };

    let proposal_id = shared.social.next_id("sp");
    let receipt_id = shared.social.next_id("auth");
    let mut state = shared
        .social
        .state
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    match SocialPolicyGate::authorize_proposal(config, &mut state, &candidate, receipt_id, now) {
        Ok(receipt) => {
            let item = SocialOutboxItem {
                proposal_id: proposal_id.clone(),
                kind: SocialKind::Reactive,
                state: OutboxState::Authorized,
                subject: subject.clone(),
                turn_id: turn_id.clone(),
                guild_id,
                channel_id,
                reply_to_message_id: message_id,
                mention_user_id,
                visibility: receipt.visibility.clone(),
                response: response_text.clone(),
                response_digest: response_digest.clone(),
                background: reply["background"].clone(),
                initiative: None,
                receipt: Some(receipt.clone()),
                discord_message_id: None,
                emitted_evidence_id: None,
                error: None,
                created_at: now,
                updated_at: now,
            };
            state.outbox.push(item);
            let _ = save_state(&shared.social.path, &mut state);
            drop(state);
            let _ = shared.spool.append_sync(
                "logs/social",
                json!({"event": "proposal_authorized", "proposal_id": proposal_id, "turn_id": turn_id, "channel_id": channel_id, "visibility": receipt.visibility, "at": iso8601(now)}),
            );
            (
                200,
                json!({
                    "authorized": true,
                    "proposal_id": proposal_id,
                    "turn_id": turn_id,
                    "subject": subject,
                    "response": response_text,
                    "response_digest": response_digest,
                    "background": reply["background"],
                    "social_memory": social_memory,
                    "receipt": receipt,
                }),
            )
        }
        Err(rej) => {
            if let Some(tid) = &turn_id {
                let _ = crate::dialogue::run_social_host_action(
                    shared,
                    dialogue,
                    &subject,
                    json!({"action": "abort_proposal", "turn_id": tid, "reason": rej.code}),
                );
            }
            state.outbox.push(SocialOutboxItem {
                proposal_id: proposal_id.clone(),
                kind: SocialKind::Reactive,
                state: OutboxState::Rejected,
                subject,
                turn_id,
                guild_id,
                channel_id,
                reply_to_message_id: message_id,
                mention_user_id,
                visibility,
                response: String::new(),
                response_digest,
                background: Value::Null,
                initiative: None,
                receipt: None,
                discord_message_id: None,
                emitted_evidence_id: None,
                error: Some(rej.code.to_owned()),
                created_at: now,
                updated_at: now,
            });
            let _ = save_state(&shared.social.path, &mut state);
            drop(state);
            let _ = shared.spool.append_sync(
                "logs/social",
                json!({"event": "policy_rejected", "proposal_id": proposal_id, "code": rej.code, "detail": rej.detail, "channel_id": channel_id, "at": iso8601(now)}),
            );
            (
                403,
                json!({"authorized": false, "proposal_id": proposal_id, "reason_code": rej.code, "error": rej.detail}),
            )
        }
    }
}

fn handle_initiative(shared: &Shared, config: &SocialConfig, request: &Value) -> (u16, Value) {
    let channel_id = request["channel_id"]
        .as_u64()
        .or_else(|| config.initiative_channel_ids.first().copied());
    let Some(channel_id) = channel_id else {
        return (
            403,
            json!({"authorized": false, "reason_code": "SOCIAL_POLICY_INITIATIVE_CHANNEL_NOT_ALLOWED", "error": "no initiative channel configured"}),
        );
    };
    let mention_user_id = request["mention_user_id"]
        .as_u64()
        .or(config.operator_user_id);

    let trigger = if let Some(raw) = request.get("trigger").filter(|v| !v.is_null()) {
        match serde_json::from_value::<InitiativeTrigger>(raw.clone()) {
            Ok(t) => t,
            Err(e) => {
                return (
                    403,
                    json!({"authorized": false, "reason_code": "SOCIAL_POLICY_MISSING_INITIATIVE_BASIS", "error": format!("invalid initiative trigger: {e}")}),
                );
            }
        }
    } else {
        let state = shared
            .social
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let Some(t) = collect_due_triggers(shared, config, &state)
            .into_iter()
            .next()
        else {
            return (
                200,
                json!({"authorized": false, "status": "no_due_triggers"}),
            );
        };
        t
    };

    process_initiative(shared, config, trigger, channel_id, mention_user_id)
}

pub fn process_initiative(
    shared: &Shared,
    config: &SocialConfig,
    trigger: InitiativeTrigger,
    channel_id: u64,
    mention_user_id: Option<u64>,
) -> (u16, Value) {
    let Some(dialogue) = &shared.config.dialogue else {
        return (
            503,
            json!({"error": "dialogue runtime is not configured on this node"}),
        );
    };
    let now = unix_now();
    let visibility = effective_visibility(config, channel_id, None);
    let precheck = SocialCandidate {
        kind: SocialKind::Initiative,
        guild_id: config.guild_id,
        channel_id,
        user_id: None,
        mention_user_id,
        visibility: visibility.clone(),
        text: None,
        social_memory: None,
        initiative: Some(trigger.clone()),
    };
    {
        let state = shared
            .social
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Err(rej) = SocialPolicyGate::authorize_precheck(config, &state, &precheck, now) {
            return (
                403,
                json!({"authorized": false, "reason_code": rej.code, "error": rej.detail}),
            );
        }
    }

    let prompt = format!(
        "[自発的発信提案] kind={} source={} reason={}\nevidence={}\nこの根拠に基づき、許可されたDiscordチャンネルへ短く率直な報告メッセージを作成してください。",
        trigger.trigger_kind, trigger.source_ref, trigger.reason, trigger.evidence_summary
    );
    let social_context = json!({
        "platform": "discord",
        "guild_id": config.guild_id,
        "channel_id": channel_id,
        "message_id": Value::Null,
        "user_id": Value::Null,
        "reply_to_message_id": Value::Null,
        "visibility": visibility,
        "is_operator": true,
    });
    let init_json = serde_json::to_value(&trigger).unwrap_or(Value::Null);
    let (status, reply) = crate::dialogue::run_social_turn(
        shared,
        dialogue,
        &config.operator_subject,
        &prompt,
        &social_context,
        Some(&init_json),
    );
    if status != 200 {
        return (status, reply);
    }

    let response_text = reply["response"].as_str().unwrap_or("").to_owned();
    let response_digest = reply["response_digest"].as_str().unwrap_or("").to_owned();
    let turn_id = reply["turn_id"].as_str().map(str::to_owned);
    let social_memory = reply.get("social_memory").cloned();

    let candidate = SocialCandidate {
        kind: SocialKind::Initiative,
        guild_id: config.guild_id,
        channel_id,
        user_id: None,
        mention_user_id,
        visibility: visibility.clone(),
        text: Some(response_text.clone()),
        social_memory: social_memory.clone(),
        initiative: Some(trigger.clone()),
    };
    let proposal_id = shared.social.next_id("sp");
    let receipt_id = shared.social.next_id("auth");
    let mut state = shared
        .social
        .state
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    match SocialPolicyGate::authorize_proposal(config, &mut state, &candidate, receipt_id, now) {
        Ok(receipt) => {
            let item = SocialOutboxItem {
                proposal_id: proposal_id.clone(),
                kind: SocialKind::Initiative,
                state: OutboxState::Authorized,
                subject: config.operator_subject.clone(),
                turn_id: turn_id.clone(),
                guild_id: config.guild_id,
                channel_id,
                reply_to_message_id: None,
                mention_user_id,
                visibility: receipt.visibility.clone(),
                response: response_text.clone(),
                response_digest: response_digest.clone(),
                background: Value::Null,
                initiative: Some(trigger),
                receipt: Some(receipt.clone()),
                discord_message_id: None,
                emitted_evidence_id: None,
                error: None,
                created_at: now,
                updated_at: now,
            };
            state.outbox.push(item.clone());
            let _ = save_state(&shared.social.path, &mut state);
            drop(state);
            let _ = shared.spool.append_sync(
                "logs/social",
                json!({"event": "initiative_authorized", "proposal_id": proposal_id, "turn_id": turn_id, "channel_id": channel_id, "at": iso8601(now)}),
            );
            (
                200,
                json!({
                    "authorized": true,
                    "item": item,
                    "receipt": receipt,
                }),
            )
        }
        Err(rej) => {
            if let Some(tid) = &turn_id {
                let _ = crate::dialogue::run_social_host_action(
                    shared,
                    dialogue,
                    &config.operator_subject,
                    json!({"action": "abort_proposal", "turn_id": tid, "reason": rej.code}),
                );
            }
            drop(state);
            (
                403,
                json!({"authorized": false, "proposal_id": proposal_id, "reason_code": rej.code, "error": rej.detail}),
            )
        }
    }
}

fn handle_verify(shared: &Shared, _config: &SocialConfig, request: &Value) -> (u16, Value) {
    let Some(dialogue) = &shared.config.dialogue else {
        return (
            503,
            json!({"error": "dialogue runtime is not configured on this node"}),
        );
    };
    let proposal_id = request["proposal_id"].as_str();
    let turn_id_req = request["turn_id"].as_str();
    let verified = request["verified"].as_bool().unwrap_or(false);
    let discord_message_id = request["discord_message_id"].as_u64();

    let found = {
        let state = shared
            .social
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        state
            .outbox
            .iter()
            .position(|item| {
                proposal_id.is_some_and(|pid| item.proposal_id == pid)
                    || turn_id_req.is_some_and(|tid| item.turn_id.as_deref() == Some(tid))
            })
            .map(|idx| state.outbox[idx].clone())
    };
    let Some(item) = found else {
        return (404, json!({"error": "social proposal not found"}));
    };

    if verified {
        let Some(msg_id) = discord_message_id.filter(|&id| id > 0) else {
            return (
                400,
                json!({"error": "discord_message_id > 0 is required when verified=true"}),
            );
        };
        if item.state == OutboxState::Persisted && item.discord_message_id == Some(msg_id) {
            return (
                200,
                json!({
                    "ok": true,
                    "state": "persisted",
                    "idempotent": true,
                    "proposal_id": item.proposal_id,
                    "turn_id": item.turn_id,
                    "discord_message_id": msg_id,
                    "emitted_evidence_id": item.emitted_evidence_id,
                }),
            );
        }
        let Some(tid) = &item.turn_id else {
            return (400, json!({"error": "proposal has no turn_id"}));
        };
        let delivery_digest = request["delivered_text_digest"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| item.response_digest.clone());
        let host_req = json!({
            "action": "verify_delivery",
            "turn_id": tid,
            "delivery": {
                "discord_message_id": msg_id,
                "channel_id": request["channel_id"].as_u64().unwrap_or(item.channel_id),
                "guild_id": request["guild_id"].as_u64().or(item.guild_id),
                "reply_to_message_id": request["reply_to_message_id"].as_u64().or(item.reply_to_message_id),
                "delivered_text_digest": delivery_digest,
                "visibility": item.visibility,
            }
        });
        match crate::dialogue::run_social_host_action(shared, dialogue, &item.subject, host_req) {
            Ok(res) => {
                let now = unix_now();
                let emitted_id = res["emitted_evidence_id"].as_str().map(str::to_owned);
                let mut state = shared
                    .social
                    .state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                if let Some(entry) = state
                    .outbox
                    .iter_mut()
                    .find(|i| i.proposal_id == item.proposal_id)
                {
                    entry.state = OutboxState::Persisted;
                    entry.discord_message_id = Some(msg_id);
                    entry.emitted_evidence_id = emitted_id.clone();
                    entry.error = None;
                    entry.updated_at = now;
                }
                let _ = save_state(&shared.social.path, &mut state);
                drop(state);
                let _ = shared.spool.append_sync(
                    "conversations/dialogue",
                    json!({
                        "kind": "persisted",
                        "subject": item.subject,
                        "proposal_id": item.proposal_id,
                        "turn_id": tid,
                        "discord_message_id": msg_id,
                        "emitted_evidence_id": emitted_id,
                        "channel_id": item.channel_id,
                    }),
                );
                let _ = shared.spool.append_sync(
                    "logs/social",
                    json!({"event": "delivery_verified_and_persisted", "proposal_id": item.proposal_id, "turn_id": tid, "discord_message_id": msg_id, "at": iso8601(now)}),
                );
                (
                    200,
                    json!({
                        "ok": true,
                        "state": "persisted",
                        "proposal_id": item.proposal_id,
                        "turn_id": tid,
                        "discord_message_id": msg_id,
                        "emitted_evidence_id": emitted_id,
                        "runtime": res,
                    }),
                )
            }
            Err((status, err)) => (status, err),
        }
    } else {
        let reason = request["error"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("discord_delivery_failed");
        if let Some(tid) = &item.turn_id {
            let _ = crate::dialogue::run_social_host_action(
                shared,
                dialogue,
                &item.subject,
                json!({"action": "abort_proposal", "turn_id": tid, "reason": reason}),
            );
        }
        let now = unix_now();
        let mut state = shared
            .social
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some(entry) = state
            .outbox
            .iter_mut()
            .find(|i| i.proposal_id == item.proposal_id)
        {
            entry.state = OutboxState::DeliveryFailed;
            entry.error = Some(reason.to_owned());
            entry.updated_at = now;
        }
        let _ = save_state(&shared.social.path, &mut state);
        drop(state);
        let _ = shared.spool.append_sync(
            "logs/social",
            json!({"event": "delivery_aborted", "proposal_id": item.proposal_id, "turn_id": item.turn_id, "reason": reason, "at": iso8601(now)}),
        );
        (
            200,
            json!({
                "ok": true,
                "state": "delivery_failed",
                "proposal_id": item.proposal_id,
                "turn_id": item.turn_id,
                "reason": reason,
            }),
        )
    }
}

fn handle_delete(shared: &Shared, config: &SocialConfig, request: &Value) -> (u16, Value) {
    let Some(dialogue) = &shared.config.dialogue else {
        return (
            503,
            json!({"error": "dialogue runtime is not configured on this node"}),
        );
    };
    let discord_message_id = request["discord_message_id"].as_u64();
    let evidence_id = request["evidence_id"].as_str();
    if discord_message_id.is_none() && evidence_id.is_none() {
        return (
            400,
            json!({"error": "discord_message_id or evidence_id is required"}),
        );
    }
    let is_operator = request["is_operator"].as_bool().unwrap_or(false);
    let user_id = request["user_id"].as_u64();
    if !is_operator && user_id.is_none() {
        return (
            403,
            json!({"error": "non-operator delete requests must include requester user_id"}),
        );
    }
    let reason = request["reason"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("user_delete_request");
    let subject = request["subject"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(&config.operator_subject);

    let host_req = json!({
        "action": "social_redact",
        "discord_message_id": discord_message_id,
        "evidence_id": evidence_id,
        "user_id": if is_operator { None } else { user_id },
        "reason": reason,
    });
    match crate::dialogue::run_social_host_action(shared, dialogue, subject, host_req) {
        Ok(res) => {
            // Abort any pending authorized outbox replies targeting the deleted message.
            if let Some(mid) = discord_message_id {
                let pending_aborts: Vec<(String, String, String)> = {
                    let mut state = shared
                        .social
                        .state
                        .lock()
                        .unwrap_or_else(|p| p.into_inner());
                    let now = unix_now();
                    let mut to_abort = Vec::new();
                    for item in &mut state.outbox {
                        if item.state == OutboxState::Authorized
                            && item.reply_to_message_id == Some(mid)
                        {
                            item.state = OutboxState::DeliveryFailed;
                            item.error = Some("target_message_deleted".to_owned());
                            item.updated_at = now;
                            if let Some(tid) = &item.turn_id {
                                to_abort.push((
                                    item.subject.clone(),
                                    tid.clone(),
                                    item.proposal_id.clone(),
                                ));
                            }
                        }
                    }
                    if !to_abort.is_empty() {
                        let _ = save_state(&shared.social.path, &mut state);
                    }
                    to_abort
                };
                for (subj, tid, _) in pending_aborts {
                    let _ = crate::dialogue::run_social_host_action(
                        shared,
                        dialogue,
                        &subj,
                        json!({"action": "abort_proposal", "turn_id": tid, "reason": "target_message_deleted"}),
                    );
                }
            }
            let _ = shared.spool.append_sync(
                "logs/social",
                json!({"event": "social_redacted", "discord_message_id": discord_message_id, "evidence_id": evidence_id, "result": res, "at": iso8601(unix_now())}),
            );
            (
                200,
                json!({"ok": true, "redacted": res["redacted_evidence_ids"]}),
            )
        }
        Err((status, err)) => (status, err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spool::Spool;
    use crate::tasks::{NewTask, TaskStatus};

    fn test_social_config(findings_path: Option<PathBuf>) -> SocialConfig {
        SocialConfig {
            enabled: true,
            guild_id: Some(100),
            allowed_channel_ids: vec![200, 201],
            public_channel_ids: vec![201],
            initiative_channel_ids: vec![200],
            allowed_user_ids: vec![300],
            operator_user_id: Some(300),
            operator_subject: "operator".to_owned(),
            reactive_min_interval_secs: 60,
            reactive_max_per_hour: 10,
            initiative_min_interval_secs: 600,
            initiative_max_per_day: 2,
            poll_interval_secs: 30,
            user_utterance_ttl_secs: 3600,
            max_user_utterances_per_channel: 50,
            findings_path,
        }
    }

    fn reactive_candidate(channel_id: u64, visibility: &str, text: &str) -> SocialCandidate {
        SocialCandidate {
            kind: SocialKind::Reactive,
            guild_id: Some(100),
            channel_id,
            user_id: Some(300),
            mention_user_id: None,
            visibility: visibility.to_owned(),
            text: Some(text.to_owned()),
            social_memory: Some(json!({
                "visibility": visibility,
                "self_history": {"relationship_memories": 0}
            })),
            initiative: None,
        }
    }

    fn initiative_candidate(
        trigger_id: &str,
        trigger_kind: &str,
        reason: &str,
        text: &str,
    ) -> SocialCandidate {
        SocialCandidate {
            kind: SocialKind::Initiative,
            guild_id: Some(100),
            channel_id: 200,
            user_id: None,
            mention_user_id: None,
            visibility: "private".to_owned(),
            text: Some(text.to_owned()),
            social_memory: None,
            initiative: Some(InitiativeTrigger {
                trigger_id: trigger_id.to_owned(),
                trigger_kind: trigger_kind.to_owned(),
                reason: reason.to_owned(),
                source_ref: "commitment:c1".to_owned(),
                evidence_summary: "verified milestone".to_owned(),
            }),
        }
    }

    #[test]
    fn policy_gate_enforces_all_guardrails() {
        let cfg = test_social_config(None);
        let mut state = SocialPersistedState::default();
        let now = 1_700_000_000;

        // Destination checks: DM forbidden, channel allowlist, user allowlist.
        let mut dm = reactive_candidate(200, "private", "hello");
        dm.guild_id = None;
        assert_eq!(
            SocialPolicyGate::authorize_proposal(&cfg, &mut state, &dm, "r1".into(), now)
                .unwrap_err()
                .code,
            "SOCIAL_POLICY_DM_FORBIDDEN"
        );

        let bad_chan = reactive_candidate(999, "private", "hello");
        assert_eq!(
            SocialPolicyGate::authorize_proposal(&cfg, &mut state, &bad_chan, "r2".into(), now)
                .unwrap_err()
                .code,
            "SOCIAL_POLICY_CHANNEL_NOT_ALLOWED"
        );

        let mut bad_user = reactive_candidate(200, "private", "hello");
        bad_user.user_id = Some(999);
        assert_eq!(
            SocialPolicyGate::authorize_proposal(&cfg, &mut state, &bad_user, "r3".into(), now)
                .unwrap_err()
                .code,
            "SOCIAL_POLICY_USER_NOT_ALLOWED"
        );

        // Secret leak & public/private isolation checks.
        let secret = reactive_candidate(200, "private", "token=DISCORD_BOT_TOKEN");
        assert_eq!(
            SocialPolicyGate::authorize_proposal(&cfg, &mut state, &secret, "r4".into(), now)
                .unwrap_err()
                .code,
            "SOCIAL_POLICY_SECRET_LEAK_BLOCKED"
        );

        let mut leak_pub = reactive_candidate(201, "public", "public reply");
        leak_pub.social_memory = Some(json!({
            "visibility": "private",
            "self_history": {"relationship_memories": 1}
        }));
        assert_eq!(
            SocialPolicyGate::authorize_proposal(&cfg, &mut state, &leak_pub, "r5".into(), now)
                .unwrap_err()
                .code,
            "SOCIAL_POLICY_PRIVATE_MEMORY_IN_PUBLIC_CHANNEL"
        );

        // Reactive success + immediate rate limit.
        let ok_react = reactive_candidate(200, "private", "了解しました。");
        let receipt =
            SocialPolicyGate::authorize_proposal(&cfg, &mut state, &ok_react, "r6".into(), now)
                .expect("authorized");
        assert_eq!(receipt.visibility, "private");
        assert_eq!(
            SocialPolicyGate::authorize_proposal(&cfg, &mut state, &ok_react, "r7".into(), now + 5)
                .unwrap_err()
                .code,
            "SOCIAL_POLICY_REACTIVE_RATE_LIMITED"
        );

        // Initiative basis, padding prohibition, relationship profiling, cooldown, dedup.
        let mut missing_basis = initiative_candidate("t0", "commitment_outcome", "ok", "hi");
        missing_basis.initiative = None;
        assert_eq!(
            SocialPolicyGate::authorize_proposal(
                &cfg,
                &mut state,
                &missing_basis,
                "i1".into(),
                now
            )
            .unwrap_err()
            .code,
            "SOCIAL_POLICY_MISSING_INITIATIVE_BASIS"
        );

        let padding = initiative_candidate("t1", "activity_padding", "ping", "hi");
        assert_eq!(
            SocialPolicyGate::authorize_proposal(&cfg, &mut state, &padding, "i2".into(), now)
                .unwrap_err()
                .code,
            "SOCIAL_POLICY_ACTIVITY_PADDING_FORBIDDEN"
        );

        let profiling = initiative_candidate(
            "t2",
            "commitment_outcome",
            "人間関係分析をまとめる",
            "result",
        );
        assert_eq!(
            SocialPolicyGate::authorize_proposal(&cfg, &mut state, &profiling, "i3".into(), now)
                .unwrap_err()
                .code,
            "SOCIAL_POLICY_RELATIONSHIP_PROFILING_FORBIDDEN"
        );

        let ok_init = initiative_candidate(
            "commitment:c1:done",
            "commitment_outcome",
            "goal complete",
            "c1が完了しました。",
        );
        SocialPolicyGate::authorize_proposal(&cfg, &mut state, &ok_init, "i4".into(), now)
            .expect("initiative authorized");

        // Cooldown active immediately after initiative.
        let second_init = initiative_candidate(
            "commitment:c2:done",
            "commitment_outcome",
            "goal complete",
            "c2が完了しました。",
        );
        assert_eq!(
            SocialPolicyGate::authorize_proposal(
                &cfg,
                &mut state,
                &second_init,
                "i5".into(),
                now + 10
            )
            .unwrap_err()
            .code,
            "SOCIAL_POLICY_INITIATIVE_RATE_LIMITED"
        );

        // Duplicate trigger rejected even after cooldown elapses.
        assert_eq!(
            SocialPolicyGate::authorize_proposal(
                &cfg,
                &mut state,
                &ok_init,
                "i6".into(),
                now + 1000
            )
            .unwrap_err()
            .code,
            "SOCIAL_POLICY_DUPLICATE_INITIATIVE_TRIGGER"
        );
    }

    #[test]
    fn collect_due_triggers_finds_commitments_delegations_and_findings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let findings_path = dir.path().join("findings.json");
        std::fs::write(
            &findings_path,
            serde_json::to_vec(&json!({
                "findings": [
                    {"id": "f1", "revision": 2, "status": "supported", "claim": "two-phase verify works"},
                    {"id": "f2", "revision": 1, "status": "draft", "claim": "not ready"}
                ]
            }))
            .expect("json"),
        )
        .expect("findings");

        let mut config: crate::config::Config = serde_json::from_value(json!({
            "node": {"id": "pi", "role": "continuity", "listen": "127.0.0.1:0"},
            "paths": {"root": dir.path()},
        }))
        .expect("config");
        config.social = Some(test_social_config(Some(findings_path)));
        let spool = Spool::new(dir.path().join("spool"), "pi").expect("spool");
        let shared = Shared::new(config.clone(), spool, 1, None);

        // Commitment 1: Active with checkpoint progress (self_goal_progress).
        let (st1, c1_res) = crate::commitments::handle(
            &shared,
            &json!({"action": "create", "goal": "Research goal alpha"}),
            "operator",
        );
        assert_eq!(st1, 200, "{c1_res}");
        let c1_id = c1_res["commitment"]["id"]
            .as_str()
            .expect("c1 id")
            .to_owned();
        let (st1u, _) = crate::commitments::handle(
            &shared,
            &json!({"action": "update", "id": c1_id, "worked": "step 1 verified"}),
            "operator",
        );
        assert_eq!(st1u, 200);

        // Commitment 2: Create via handle, then mark Done in persisted commitments.json and reload.
        let (st2, c2_res) = crate::commitments::handle(
            &shared,
            &json!({"action": "create", "goal": "Finish closed loop"}),
            "operator",
        );
        assert_eq!(st2, 200);
        let c2_id = c2_res["commitment"]["id"]
            .as_str()
            .expect("c2 id")
            .to_owned();
        let commitments_file = config.paths.current_state().join("commitments.json");
        let mut stored: Vec<Value> =
            serde_json::from_slice(&std::fs::read(&commitments_file).expect("read commitments"))
                .expect("json");
        for entry in &mut stored {
            if entry["id"].as_str() == Some(c2_id.as_str()) {
                entry["state"] = json!("done");
                entry["outcome"] = json!("verified end-to-end");
            }
        }
        std::fs::write(
            &commitments_file,
            serde_json::to_vec(&stored).expect("serialize"),
        )
        .expect("write");

        let spool = Spool::new(dir.path().join("spool"), "pi").expect("spool");
        let shared = Shared::new(config, spool, 1, None);

        // Reviewed delegated task.
        shared.tasks.create(
            &shared.spool,
            NewTask {
                title: "Delegate experiment",
                kind: "delegated",
                node: "llm_master",
                owner: "mio",
                status: TaskStatus::Done,
                depends_on: vec![],
                detail: json!({"review": {"verdict": "approved"}}),
            },
        );

        let social_cfg = shared.config.social.as_ref().expect("social cfg");
        let triggers = collect_due_triggers(&shared, social_cfg, &SocialPersistedState::default());
        let kinds: Vec<&str> = triggers.iter().map(|t| t.trigger_kind.as_str()).collect();
        assert!(kinds.contains(&"self_goal_progress"), "{triggers:?}");
        assert!(kinds.contains(&"commitment_outcome"), "{triggers:?}");
        assert!(kinds.contains(&"delegated_review"), "{triggers:?}");
        assert!(kinds.contains(&"research_finding"), "{triggers:?}");
        assert!(triggers.iter().any(|t| t.trigger_id == "finding:f1:rev2"));
        assert!(
            !triggers
                .iter()
                .any(|t| t.trigger_id.starts_with("finding:f2"))
        );
    }

    #[test]
    fn load_reconciles_proposed_but_preserves_authorized() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("social.json");
        let initial = SocialPersistedState {
            outbox: vec![
                SocialOutboxItem {
                    proposal_id: "sp-proposed".into(),
                    kind: SocialKind::Reactive,
                    state: OutboxState::Proposed,
                    subject: "operator".into(),
                    turn_id: Some("t1".into()),
                    guild_id: Some(100),
                    channel_id: 200,
                    reply_to_message_id: Some(1),
                    mention_user_id: None,
                    visibility: "private".into(),
                    response: "unsent".into(),
                    response_digest: "sha256:1".into(),
                    background: Value::Null,
                    initiative: None,
                    receipt: None,
                    discord_message_id: None,
                    emitted_evidence_id: None,
                    error: None,
                    created_at: 10,
                    updated_at: 10,
                },
                SocialOutboxItem {
                    proposal_id: "sp-auth".into(),
                    kind: SocialKind::Initiative,
                    state: OutboxState::Authorized,
                    subject: "operator".into(),
                    turn_id: Some("t2".into()),
                    guild_id: Some(100),
                    channel_id: 200,
                    reply_to_message_id: None,
                    mention_user_id: None,
                    visibility: "private".into(),
                    response: "ready".into(),
                    response_digest: "sha256:2".into(),
                    background: Value::Null,
                    initiative: None,
                    receipt: None,
                    discord_message_id: None,
                    emitted_evidence_id: None,
                    error: None,
                    created_at: 11,
                    updated_at: 11,
                },
            ],
            ..Default::default()
        };
        std::fs::write(&path, serde_json::to_vec(&initial).expect("json")).expect("write");

        let engine = SocialEngine::load(path.clone(), Some(test_social_config(None)));
        let state = engine.state.lock().expect("lock");
        assert_eq!(state.outbox[0].state, OutboxState::DeliveryFailed);
        assert_eq!(
            state.outbox[0].error.as_deref(),
            Some("interrupted_before_authorization")
        );
        assert_eq!(state.outbox[1].state, OutboxState::Authorized);
        drop(state);

        // Reloading reads reconciled persisted file cleanly.
        let reloaded = SocialEngine::load(path, Some(test_social_config(None)));
        let state2 = reloaded.state.lock().expect("lock");
        assert_eq!(state2.outbox[0].state, OutboxState::DeliveryFailed);
        assert_eq!(state2.outbox[1].state, OutboxState::Authorized);
    }

    #[cfg(unix)]
    fn shared_with_social_runtime(dir: &Path, log_path: &Path) -> Shared {
        let rt = dir.join("rt");
        std::fs::create_dir_all(&rt).expect("rt dir");
        std::fs::write(rt.join("kamimusuhi.sqlite"), b"").expect("db marker");
        let bin = dir.join("fake-social-runtime");
        let log_str = log_path.display();
        let script = format!(
            "#!/bin/sh\n\
while IFS= read -r line; do\n\
  printf '%s\\n' \"$line\" >> '{log_str}'\n\
  case \"$line\" in\n\
    *'\"action\":\"verify_delivery\"'*)\n\
      printf '%s\\n' '{{\"ok\":true,\"emitted_evidence_id\":\"ev_agent_1\"}}'\n\
      ;;\n\
    *'\"action\":\"abort_proposal\"'*)\n\
      printf '%s\\n' '{{\"ok\":true,\"aborted\":true}}'\n\
      ;;\n\
    *'\"action\":\"social_redact\"'*)\n\
      printf '%s\\n' '{{\"ok\":true,\"redacted_evidence_ids\":[\"ev_user_1\",\"ev_agent_1\"]}}'\n\
      ;;\n\
    *'\"action\":\"social_retention\"'*)\n\
      printf '%s\\n' '{{\"ok\":true,\"redacted\":0}}'\n\
      ;;\n\
    *'\"visibility\":\"public\"'*)\n\
      printf '%s\\n' '{{\"response\":\"公開チャンネルの返答です。\",\"turn_id\":\"turn-pub\",\"session_id\":\"s1\",\"individual_id\":\"mio\",\"response_digest\":\"sha256:pub\",\"background\":{{\"warmed_memories\":0,\"memory_writes\":[]}},\"social_memory\":{{\"visibility\":\"public\",\"self_history\":{{\"relationship_memories\":0}}}}}}'\n\
      ;;\n\
    *)\n\
      printf '%s\\n' '{{\"response\":\"確認しました。\",\"turn_id\":\"turn-priv\",\"session_id\":\"s1\",\"individual_id\":\"mio\",\"response_digest\":\"sha256:priv\",\"background\":{{\"warmed_memories\":0,\"memory_writes\":[]}},\"social_memory\":{{\"visibility\":\"private\",\"self_history\":{{\"relationship_memories\":0}}}}}}'\n\
      ;;\n\
  esac\n\
done\n"
        );
        std::fs::write(&bin, script).expect("write script");
        let mut perms = std::fs::metadata(&bin).expect("meta").permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&bin, perms).expect("chmod");

        let mut social = test_social_config(None);
        social.reactive_min_interval_secs = 0;
        let config: crate::config::Config = serde_json::from_value(json!({
            "node": {"id": "pi", "role": "continuity", "listen": "127.0.0.1:0"},
            "paths": {"root": dir},
            "dialogue": {"runtime_binary": bin, "dir": rt, "timeout_secs": 5},
            "social": social,
        }))
        .expect("config");
        config.validate().expect("valid");
        let spool = Spool::new(dir.join("spool"), "pi").expect("spool");
        Shared::new(config, spool, 1, None)
    }

    #[cfg(unix)]
    #[test]
    fn end_to_end_propose_outbox_verify_abort_and_delete() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log_path = dir.path().join("host-calls.log");
        let shared = shared_with_social_runtime(dir.path(), &log_path);

        // 1. Propose reactive reply on private channel 200.
        let (st, prop) = handle(
            &shared,
            &json!({
                "action": "propose",
                "guild_id": 100,
                "channel_id": 200,
                "user_id": 300,
                "message_id": 42,
                "message": "進捗を教えてください",
                "is_operator": false,
            }),
        );
        assert_eq!(st, 200, "{prop}");
        assert_eq!(prop["authorized"], json!(true));
        let pid1 = prop["proposal_id"]
            .as_str()
            .expect("proposal_id")
            .to_owned();

        // Outbox lists authorized proposal until verified.
        let (ost, outbox) = handle(&shared, &json!({"action": "outbox"}));
        assert_eq!(ost, 200);
        assert_eq!(outbox["items"].as_array().expect("items").len(), 1);
        assert_eq!(outbox["items"][0]["proposal_id"], json!(pid1));

        // Verify delivery persists utterance.
        let (vst, ver) = handle(
            &shared,
            &json!({
                "action": "verify",
                "proposal_id": pid1,
                "verified": true,
                "discord_message_id": 555,
            }),
        );
        assert_eq!(vst, 200, "{ver}");
        assert_eq!(ver["state"], json!("persisted"));
        assert_eq!(ver["emitted_evidence_id"], json!("ev_agent_1"));

        let calls_before = std::fs::read_to_string(&log_path)
            .expect("log")
            .lines()
            .count();
        // Idempotent retry does not hit the runtime host again.
        let (vst2, ver2) = handle(
            &shared,
            &json!({
                "action": "verify",
                "proposal_id": pid1,
                "verified": true,
                "discord_message_id": 555,
            }),
        );
        assert_eq!(vst2, 200, "{ver2}");
        assert_eq!(ver2["idempotent"], json!(true));
        let calls_after = std::fs::read_to_string(&log_path)
            .expect("log")
            .lines()
            .count();
        assert_eq!(calls_before, calls_after);

        // 2. Propose on public channel 201 and abort on failed Discord delivery.
        let (st2, prop2) = handle(
            &shared,
            &json!({
                "action": "propose",
                "guild_id": 100,
                "channel_id": 201,
                "user_id": 300,
                "message_id": 43,
                "message": "公開で挨拶して",
            }),
        );
        assert_eq!(st2, 200, "{prop2}");
        let pid2 = prop2["proposal_id"].as_str().expect("pid2").to_owned();
        let (ast, ab) = handle(
            &shared,
            &json!({
                "action": "verify",
                "proposal_id": pid2,
                "verified": false,
                "error": "discord_503",
            }),
        );
        assert_eq!(ast, 200, "{ab}");
        assert_eq!(ab["state"], json!("delivery_failed"));
        let log = std::fs::read_to_string(&log_path).expect("log");
        assert!(log.contains("\"action\":\"abort_proposal\""), "{log}");

        // 3. Delete forwards redaction for user and rejects anonymous non-operator delete.
        let (bad_del_st, _) = handle(
            &shared,
            &json!({
                "action": "delete",
                "discord_message_id": 555,
                "is_operator": false,
            }),
        );
        assert_eq!(bad_del_st, 403);

        let (del_st, del) = handle(
            &shared,
            &json!({
                "action": "delete",
                "discord_message_id": 555,
                "user_id": 300,
                "is_operator": false,
            }),
        );
        assert_eq!(del_st, 200, "{del}");
        assert_eq!(del["ok"], json!(true));
        let log2 = std::fs::read_to_string(&log_path).expect("log");
        assert!(log2.contains("\"action\":\"social_redact\""), "{log2}");
    }
}
