//! The route gate: which provider may serve which kind of turn.
//!
//! This module is policy, not transport. It reads provider declarations
//! ([`crate::provider_bench::ProviderSpec`]) plus live health observations
//! and returns an ordered list of candidates for a lane of work. It never
//! sends a request, never sees a credential, and never sees prompt text —
//! only sizes and privacy flags.
//!
//! The governing rules, in order of precedence:
//!
//! 1. **Privacy is structural, not advisory.** A provider that trains on
//!    free-tier inputs is ineligible for any turn carrying persona memory,
//!    private history, durable self or private references — regardless of
//!    how fast or cheap it is.
//! 2. **Cost minimization.** Billing classes order candidates:
//!    `Local`/`Subscription`/`FreeTier` first, `Metered` last. A metered
//!    provider (Cerebras) is the paid fallback, selected only when no
//!    unmetered candidate can serve the turn.
//! 3. **Eligibility before speed.** Context limits, per-minute token
//!    ceilings, an active 429 cooldown or a high recent failure rate all
//!    remove a candidate before latency is considered.
//! 4. **Latency last.** Among eligible candidates, the observed latency
//!    estimate orders the list; providers that can suppress reasoning win
//!    ties on the fast-chat lane.
//!
//! Agent lanes (CODE_AGENT / RESEARCH_AGENT / BACKGROUND_AGENT) are *not*
//! chat providers and are never mixed into the OpenAI-compatible candidate
//! list. They exist here as declared lanes with an availability probe so
//! future work can attach official CLI harnesses — subscription CLIs used
//! through their documented non-interactive modes only, never through
//! unofficial OAuth reuse or token extraction.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::provider_bench::ProviderSpec;

/// Minimal provider-policy view consumed by the routing gate. Keeping the
/// gate generic lets the benchmark registry and resident daemon share the
/// same eligibility/order rules without duplicating provider policy.
pub trait RouteProvider {
    fn route_id(&self) -> &str;
    fn billing(&self) -> BillingClass;
    fn privacy_ok_for_private_memory(&self) -> bool;
    fn context_limit_tokens(&self) -> Option<u64>;
    fn approx_tpm_limit(&self) -> Option<u64>;
    fn reasoning_suppression(&self) -> bool;
    fn route_available(&self) -> bool;
    /// Operator preference: this provider leads `lane` ahead of the billing
    /// order (e.g. a fast metered cloud for quick everyday chat). Privacy,
    /// context, health and spend limits still apply.
    fn preferred_for(&self, _lane: RouteLane) -> bool {
        false
    }
}

impl RouteProvider for ProviderSpec {
    fn route_id(&self) -> &str {
        self.id
    }
    fn billing(&self) -> BillingClass {
        self.billing
    }
    fn privacy_ok_for_private_memory(&self) -> bool {
        self.privacy_ok_for_private_memory
    }
    fn context_limit_tokens(&self) -> Option<u64> {
        self.context_limit_tokens.map(u64::from)
    }
    fn approx_tpm_limit(&self) -> Option<u64> {
        self.approx_tpm_limit
    }
    fn reasoning_suppression(&self) -> bool {
        self.reasoning_suppression
    }
    fn route_available(&self) -> bool {
        self.skip_reason.is_none() && !self.base_url.is_empty()
    }
}

/// How a provider is paid for. Drives the cost-minimizing order; it is a
/// property of the *plan*, not the vendor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BillingClass {
    /// Self-hosted or peer node; marginal cost is zero.
    Local,
    /// Flat subscription already paid for (e.g. HAI); marginal cost is zero.
    Subscription,
    /// Provider free tier; marginal cost is zero but rate limits and —
    /// depending on the plan's data terms — training use may apply.
    FreeTier,
    /// Per-token billing. Used only when unmetered paths cannot serve.
    Metered,
}

/// A lane of work with its own routing rules. Chat lanes pick an
/// OpenAI-compatible provider; agent lanes are separate harnesses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RouteLane {
    /// Ordinary conversation: latency matters, cheap-first.
    FastChat,
    /// Content that must not leave the local node.
    LocalChat,
    /// Hard reasoning: quality first, latency secondary.
    DeepReasoning,
    /// Turn that will drive tool calls.
    ToolTask,
    /// Turn needing heavy recall/context.
    MemoryHeavy,
    /// Coding work handed to an agent CLI (e.g. Codex/Claude Code).
    CodeAgent,
    /// Long-running research handed to an agent CLI (e.g. Gemini CLI).
    ResearchAgent,
    /// Background/batch work; latency irrelevant, cost still capped.
    BackgroundAgent,
}

/// The two planes of work. The chat plane talks to people under a latency
/// budget; the task plane runs agent harnesses for as long as a task needs
/// (see `agent_exec`). Chat never waits on the task plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkPlane {
    Chat,
    Task,
}

impl RouteLane {
    pub const fn plane(self) -> WorkPlane {
        if self.is_chat() {
            WorkPlane::Chat
        } else {
            WorkPlane::Task
        }
    }

    /// Lanes that select an OpenAI-compatible chat provider.
    pub const CHAT_LANES: [Self; 5] = [
        Self::FastChat,
        Self::LocalChat,
        Self::DeepReasoning,
        Self::ToolTask,
        Self::MemoryHeavy,
    ];

    /// Lanes served by agent harnesses, never by chat providers.
    pub const AGENT_LANES: [Self; 3] =
        [Self::CodeAgent, Self::ResearchAgent, Self::BackgroundAgent];

    pub const fn is_chat(self) -> bool {
        !matches!(
            self,
            Self::CodeAgent | Self::ResearchAgent | Self::BackgroundAgent
        )
    }
}

/// Live health of one provider, fed by observed requests. Kept in-process:
/// this is the routing gate's memory of what recently failed, never the
/// durable store's business.
#[derive(Debug, Clone, Default)]
pub struct ProviderHealth {
    /// Exponentially weighted first-content latency (ms).
    pub ewma_ttft_ms: Option<f64>,
    /// Consecutive failures (any kind).
    pub consecutive_failures: u32,
    /// Requests observed in the current window.
    pub window_requests: u32,
    /// Failures in the current window.
    pub window_failures: u32,
    /// If a 429 was seen, do not retry before this instant.
    pub rate_limited_until: Option<Instant>,
    /// Largest cached_tokens the provider recently reported.
    pub last_cached_tokens: Option<u64>,
    /// Time of the latest live request observation. A stale slow sample must
    /// not bench a recovered provider forever.
    pub last_observed_at: Option<Instant>,
    /// Time of the latest failed observation. The failure-rate bench is
    /// measured from here so it expires into a bounded retry instead of
    /// excluding the provider until restart; a success clears it.
    pub last_failure_at: Option<Instant>,
}

impl ProviderHealth {
    /// Failure rate over the observation window, `None` when no data.
    pub fn failure_rate(&self) -> Option<f64> {
        (self.window_requests > 0)
            .then(|| self.window_failures as f64 / self.window_requests as f64)
    }

    /// Record one outcome. `rate_limited` starts a cooldown; a success
    /// resets the consecutive-failure count and updates the EWMA.
    pub fn observe(
        &mut self,
        ok: bool,
        ttft_ms: Option<u64>,
        rate_limited: bool,
        cached_tokens: Option<u64>,
        cooldown: Duration,
        now: Instant,
    ) {
        self.window_requests += 1;
        self.last_observed_at = Some(now);
        if ok {
            self.consecutive_failures = 0;
            // An answered request ends the failure bench. Keep any 429
            // cooldown: an already in-flight request can finish while
            // the provider still requires new requests to back off.
            self.last_failure_at = None;
            if let Some(ttft) = ttft_ms {
                let ttft = ttft as f64;
                self.ewma_ttft_ms = Some(self.ewma_ttft_ms.map_or(ttft, |e| 0.6 * e + 0.4 * ttft));
            }
            if let Some(cached) = cached_tokens {
                self.last_cached_tokens = Some(self.last_cached_tokens.unwrap_or(0).max(cached));
            }
        } else {
            self.window_failures += 1;
            self.consecutive_failures += 1;
            self.last_failure_at = Some(now);
            if rate_limited {
                self.rate_limited_until = Some(now + cooldown);
            }
        }
    }
}

/// The gate's per-provider health table.
#[derive(Debug, Default)]
pub struct ProviderStateBook {
    states: BTreeMap<String, ProviderHealth>,
}

impl ProviderStateBook {
    pub fn get(&self, provider: &str) -> Option<&ProviderHealth> {
        self.states.get(provider)
    }

    pub fn get_mut(&mut self, provider: &str) -> &mut ProviderHealth {
        self.states.entry(provider.to_owned()).or_default()
    }
}

/// Why a candidate is or is not usable — recorded on the sample so a run
/// can be audited without reproducing the decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteReason {
    /// First choice of its billing class.
    Primary,
    /// Cheaper classes had no eligible provider.
    BillingFallback,
    /// Preceding candidate(s) were over context/token limits.
    ContextFallback,
    /// Preceding candidate(s) excluded by privacy.
    PrivacyFallback,
    /// Preceding candidate(s) in a 429 cooldown or failing.
    HealthFallback,
}

/// One selectable candidate with the audit trail for its position.
#[derive(Debug)]
pub struct RouteCandidate<'a, S: RouteProvider> {
    pub spec: &'a S,
    pub reason: RouteReason,
    /// Estimated first-token latency used for ordering, `None` unmeasured.
    pub est_ttft_ms: Option<u64>,
}

/// The routing gate. Stateless policy: everything mutable lives in
/// [`ProviderStateBook`], so the gate itself can be shared across lanes.
pub struct RouteGate {
    /// Cooldown applied when a provider returns 429.
    pub rate_limit_cooldown: Duration,
    /// Window failure rate above which a provider is benched.
    pub failure_rate_threshold: f64,
    /// Minimum window size before the failure rate counts.
    pub failure_window_min: u32,
    /// A provider benched by the failure rate is retried after this
    /// interval. A renewed failure
    /// re-arms the bench and a success ends it, so a transient outage can
    /// never exclude a provider until process restart.
    pub failure_retry_after: Duration,
    /// EWMA first-token latency above which a provider stops being usable
    /// for [`RouteLane::FastChat`]. A slow-but-free provider is not "usable"
    /// for conversation; beyond this ceiling the turn falls through to the
    /// next class. Other lanes have no ceiling — a background task can wait.
    pub fast_chat_ttft_ceiling_ms: Option<u64>,
    /// A slow provider is temporarily benched, then retried so recovery does
    /// not require a resident restart.
    pub latency_retry_after: Duration,
    /// A latency sample older than this no longer describes the provider.
    /// Unmeasured and stale candidates are explored first within their
    /// billing band; otherwise the first provider to be measured would win
    /// that band forever and its siblings would never get a sample.
    pub latency_explore_after: Duration,
}

impl Default for RouteGate {
    fn default() -> Self {
        Self {
            rate_limit_cooldown: Duration::from_secs(60),
            failure_rate_threshold: 0.5,
            failure_window_min: 3,
            failure_retry_after: Duration::from_secs(60),
            // Thirty seconds to the first token is where "slow" stops being
            // conversation: measured free-tier reasoning upstreams sit at
            // 12-48s, fast clouds at 0.4-2s, HAI at 4-22s.
            fast_chat_ttft_ceiling_ms: Some(30_000),
            latency_retry_after: Duration::from_secs(60),
            latency_explore_after: Duration::from_secs(15 * 60),
        }
    }
}

/// Billing classes in preference order per lane. Metered is always last:
/// it exists to rescue a turn, not to serve it. FastChat leads with the
/// free tier — free fast cloud is both zero-marginal-cost and fastest —
/// then the subscription and local paths, exactly the stated policy
/// "usable free provider → HAI / local → Cerebras paid fallback".
const BILLING_ORDER: [BillingClass; 4] = [
    BillingClass::FreeTier,
    BillingClass::Subscription,
    BillingClass::Local,
    BillingClass::Metered,
];

impl RouteGate {
    /// Ordered candidates for a lane.
    ///
    /// `prompt_tokens_est` is the assembled request size; `private` marks a
    /// turn carrying persona memory / private history / durable self /
    /// private references. `health` holds recent observations. The returned
    /// order is cheapest-first within eligibility; [`RouteReason`] records
    /// *why* each candidate sits where it does.
    pub fn select<'a, S: RouteProvider>(
        &self,
        lane: RouteLane,
        specs: &'a [S],
        prompt_tokens_est: u64,
        private: bool,
        health: &ProviderStateBook,
        now: Instant,
    ) -> Vec<RouteCandidate<'a, S>> {
        debug_assert!(lane.is_chat());
        let mut candidates: Vec<RouteCandidate<'a, S>> = Vec::new();
        let mut skipped_for_privacy = false;
        let mut skipped_for_context = false;
        let mut skipped_for_health = false;

        for spec in specs {
            if !spec.route_available() {
                continue;
            }
            // Privacy is a hard wall: a provider whose plan may train on
            // inputs never sees private material, whatever the speed.
            if private && !spec.privacy_ok_for_private_memory() {
                skipped_for_privacy = true;
                continue;
            }
            // Context ceiling: a provider that cannot hold the prompt is
            // not a candidate at all.
            if let Some(limit) = spec.context_limit_tokens()
                && prompt_tokens_est > limit
            {
                skipped_for_context = true;
                continue;
            }
            // Free-tier token ceilings are approximate; a prompt that would
            // blow through the minute budget is sent elsewhere rather than
            // eaten by a 429.
            if let Some(tpm) = spec.approx_tpm_limit()
                && prompt_tokens_est > tpm
            {
                skipped_for_context = true;
                continue;
            }
            if let Some(state) = health.get(spec.route_id()) {
                if state.rate_limited_until.is_some_and(|until| now < until) {
                    skipped_for_health = true;
                    continue;
                }
                // A sustained failure rate benches the provider, but the
                // bench is bounded: once the last failure is
                // `failure_retry_after` old the provider is eligible for
                // retry. A renewed failure re-arms the bench and
                // a success ends it — recovery never needs a restart.
                if state.window_requests >= self.failure_window_min
                    && state
                        .failure_rate()
                        .is_some_and(|rate| rate > self.failure_rate_threshold)
                    && state
                        .last_failure_at
                        .is_some_and(|at| now < at + self.failure_retry_after)
                {
                    skipped_for_health = true;
                    continue;
                }
                // A provider whose measured latency is past the lane's
                // ceiling is "not usable", the same class of disqualifier
                // as a 429 — the turn is not free if the user has left.
                if lane == RouteLane::FastChat
                    && let Some(ceiling) = self.fast_chat_ttft_ceiling_ms
                    && state.ewma_ttft_ms.is_some_and(|ewma| ewma > ceiling as f64)
                    && state
                        .last_observed_at
                        .is_none_or(|at| now < at + self.latency_retry_after)
                {
                    skipped_for_health = true;
                    continue;
                }
            }
            candidates.push(RouteCandidate {
                spec,
                reason: RouteReason::Primary,
                est_ttft_ms: health
                    .get(spec.route_id())
                    .and_then(|s| s.ewma_ttft_ms)
                    .map(|v| v as u64),
            });
        }

        // Lane-specific class preference: LocalChat wants Local first;
        // DeepReasoning tolerates cost; everything else is cost-minimal.
        let class_rank = |class: BillingClass| -> usize {
            match lane {
                RouteLane::FastChat => match class {
                    BillingClass::FreeTier => 0,
                    // Both are already paid-for / zero-marginal-cost paths.
                    // Keep them in one band so observed latency (and, before
                    // measurements exist, operator config order) decides.
                    BillingClass::Subscription | BillingClass::Local => 1,
                    BillingClass::Metered => 2,
                },
                RouteLane::LocalChat => usize::from(class != BillingClass::Local),
                RouteLane::DeepReasoning | RouteLane::ToolTask | RouteLane::MemoryHeavy => {
                    match class {
                        BillingClass::Subscription | BillingClass::Local => 0,
                        BillingClass::Metered => 1,
                        BillingClass::FreeTier => 2,
                    }
                }
                _ => BILLING_ORDER
                    .iter()
                    .position(|c| *c == class)
                    .unwrap_or(BILLING_ORDER.len()),
            }
        };
        let lane_allows = |class: BillingClass| -> bool {
            match lane {
                RouteLane::LocalChat => class == BillingClass::Local,
                _ => true,
            }
        };
        candidates.retain(|c| lane_allows(c.spec.billing()));

        // Within a billing class: unmeasured/stale candidates are explored
        // first, then measured TTFT, reasoning-capable before reasoning-forced
        // on the fast lane (thinking time is TTFT).
        let ceiling = match lane {
            RouteLane::FastChat => self.fast_chat_ttft_ceiling_ms,
            _ => None,
        };
        let order_ms = |c: &RouteCandidate<'a, S>| -> u64 {
            let Some(state) = health.get(c.spec.route_id()) else {
                return 0;
            };
            let Some(ewma) = state.ewma_ttft_ms else {
                return 0;
            };
            let stale = state
                .last_observed_at
                .is_none_or(|at| now >= at + self.latency_explore_after);
            // A sample past the lane's ceiling is not re-explored on a
            // user-facing lane: it would spend a slow turn to confirm it.
            if stale && ceiling.is_none_or(|limit| ewma <= limit as f64) {
                return 0;
            }
            ewma as u64
        };
        candidates.sort_by_cached_key(|c| {
            let reasoning_penalty = match lane {
                RouteLane::FastChat | RouteLane::LocalChat => {
                    usize::from(!c.spec.reasoning_suppression())
                }
                _ => 0,
            };
            (
                usize::from(!c.spec.preferred_for(lane)),
                class_rank(c.spec.billing()),
                reasoning_penalty,
                order_ms(c),
            )
        });

        // Assign reasons: the first candidate is Primary; later ones carry
        // the strongest applicable skip cause.
        let fallback_reason = if skipped_for_privacy {
            RouteReason::PrivacyFallback
        } else if skipped_for_health {
            RouteReason::HealthFallback
        } else if skipped_for_context {
            RouteReason::ContextFallback
        } else {
            RouteReason::BillingFallback
        };
        let primary_rank = candidates
            .first()
            .map(|c| class_rank(c.spec.billing()))
            .unwrap_or(0);
        for (index, candidate) in candidates.iter_mut().enumerate() {
            candidate.reason = if index == 0 {
                RouteReason::Primary
            } else if class_rank(candidate.spec.billing()) > primary_rank {
                RouteReason::BillingFallback
            } else {
                fallback_reason.clone()
            };
        }
        candidates
    }
}

/// One agent lane's availability, discovered not assumed. Presence of a
/// binary is recorded; nothing is invoked.
#[derive(Debug, Clone, Serialize)]
pub struct AgentLaneStatus {
    pub lane: String,
    /// Official CLI name probed on PATH (e.g. `codex`, `claude`, `gemini`).
    pub cli: String,
    pub available: bool,
    /// Documented non-interactive invocation, e.g. `claude -p`.
    pub headless_mode: &'static str,
}

/// Which CLIs could fill each agent lane, probed by presence on PATH only.
/// The probe never runs the tool: a subscription CLI is used through its
/// documented headless mode or not at all — no OAuth reuse, no token
/// extraction, no pretending a CLI is an API.
pub fn probe_agent_lanes() -> Vec<AgentLaneStatus> {
    const PROBES: [(&str, &str, &str); 3] = [
        ("CODE_AGENT", "codex", "codex exec"),
        ("RESEARCH_AGENT", "gemini", "gemini -p"),
        ("CODE_AGENT", "claude", "claude -p"),
    ];
    PROBES
        .iter()
        .map(|(lane, cli, headless)| {
            let available = std::env::var_os("PATH")
                .map(|paths| {
                    std::env::split_paths(&paths).any(|dir| {
                        let candidate = dir.join(cli);
                        candidate.is_file()
                    })
                })
                .unwrap_or(false);
            AgentLaneStatus {
                lane: (*lane).to_owned(),
                cli: (*cli).to_owned(),
                available,
                headless_mode: headless,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_bench::{AuthSource, ModelSpec};

    /// Minimal spec builder: only the fields the gate reads matter.
    fn spec(
        id: &'static str,
        billing: BillingClass,
        private_ok: bool,
        ctx: Option<u32>,
        tpm: Option<u64>,
        reasoning_suppression: bool,
    ) -> ProviderSpec {
        ProviderSpec {
            id,
            base_url: "https://example.invalid/v1",
            auth: AuthSource::None,
            models: &[ModelSpec {
                id: "m",
                input_usd_per_mtok: None,
                output_usd_per_mtok: None,
            }],
            extra_headers: &[],
            extra_body: "",
            stream_usage: false,
            accepts_chat_template_kwargs: false,
            key_name: id,
            billing,
            privacy_ok_for_private_memory: private_ok,
            context_limit_tokens: ctx,
            approx_tpm_limit: tpm,
            reasoning_suppression,
            skip_reason: None,
        }
    }

    #[test]
    fn fast_chat_orders_cheap_first_metered_last() {
        let gate = RouteGate::default();
        let specs = vec![
            spec("paid", BillingClass::Metered, true, None, None, true),
            spec("free", BillingClass::FreeTier, true, None, None, true),
            spec("sub", BillingClass::Subscription, true, None, None, true),
            spec("local", BillingClass::Local, true, None, None, true),
        ];
        let health = ProviderStateBook::default();
        let order = gate
            .select(
                RouteLane::FastChat,
                &specs,
                2_000,
                false,
                &health,
                Instant::now(),
            )
            .iter()
            .map(|c| c.spec.id)
            .collect::<Vec<_>>();
        // Policy order: usable free provider → HAI / local → paid fallback.
        assert_eq!(order, vec!["free", "sub", "local", "paid"]);
    }

    #[test]
    fn fast_chat_benches_providers_past_the_latency_ceiling() {
        let gate = RouteGate::default();
        let now = Instant::now();
        let mut health = ProviderStateBook::default();
        // A free provider that thinks for ~45s before answering is not a
        // usable fast-chat path — the turn should fall to the next class.
        for _ in 0..3 {
            health.get_mut("slow-free").observe(
                true,
                Some(45_000),
                false,
                None,
                gate.rate_limit_cooldown,
                now,
            );
        }
        let specs = vec![
            spec("slow-free", BillingClass::FreeTier, true, None, None, true),
            spec("sub", BillingClass::Subscription, true, None, None, true),
        ];
        let order = gate.select(RouteLane::FastChat, &specs, 2_000, false, &health, now);
        assert_eq!(order.len(), 1);
        assert_eq!(order[0].spec.id, "sub");
        // Other lanes have no latency ceiling — background work can wait.
        let order = gate.select(RouteLane::MemoryHeavy, &specs, 2_000, false, &health, now);
        assert_eq!(order.len(), 2);

        // A stale slow sample is retried after the cooldown; otherwise one
        // bad turn would bench a recovered provider until process restart.
        let later = now + gate.latency_retry_after + Duration::from_secs(1);
        let order = gate.select(RouteLane::FastChat, &specs, 2_000, false, &health, later);
        assert_eq!(order.len(), 2);
    }

    #[test]
    fn private_turns_exclude_training_on_input_providers() {
        let gate = RouteGate::default();
        let specs = vec![
            spec("free", BillingClass::FreeTier, false, None, None, true),
            spec("paid", BillingClass::Metered, true, None, None, true),
        ];
        let health = ProviderStateBook::default();
        // Public turn: the free provider leads.
        let public = gate.select(
            RouteLane::FastChat,
            &specs,
            2_000,
            false,
            &health,
            Instant::now(),
        );
        assert_eq!(public[0].spec.id, "free");
        // Private turn: the free provider is structurally absent.
        let private = gate.select(
            RouteLane::FastChat,
            &specs,
            2_000,
            true,
            &health,
            Instant::now(),
        );
        assert_eq!(private.len(), 1);
        assert_eq!(private[0].spec.id, "paid");
        assert_eq!(private[0].reason, RouteReason::Primary);
    }

    #[test]
    fn oversized_prompts_skip_small_context_providers() {
        let gate = RouteGate::default();
        let specs = vec![
            spec(
                "small",
                BillingClass::FreeTier,
                true,
                Some(8_192),
                None,
                true,
            ),
            spec(
                "big",
                BillingClass::Metered,
                true,
                Some(131_072),
                None,
                true,
            ),
            spec(
                "tpm-bound",
                BillingClass::FreeTier,
                true,
                None,
                Some(15_000),
                true,
            ),
        ];
        let health = ProviderStateBook::default();
        let order = gate.select(
            RouteLane::FastChat,
            &specs,
            28_000,
            false,
            &health,
            Instant::now(),
        );
        // Only the metered provider can hold 28k prompt tokens.
        assert_eq!(order.len(), 1);
        assert_eq!(order[0].spec.id, "big");
    }

    #[test]
    fn rate_limit_cooldown_and_failure_rate_bench_providers() {
        let gate = RouteGate::default();
        let now = Instant::now();
        let mut health = ProviderStateBook::default();
        // "limited" just took a 429.
        health
            .get_mut("limited")
            .observe(false, None, true, None, gate.rate_limit_cooldown, now);
        // "flaky" has failed 3 of 3 requests in its window.
        for _ in 0..3 {
            health.get_mut("flaky").observe(
                false,
                None,
                false,
                None,
                gate.rate_limit_cooldown,
                now,
            );
        }
        // "steady" answered quickly.
        health.get_mut("steady").observe(
            true,
            Some(400),
            false,
            Some(1_000),
            gate.rate_limit_cooldown,
            now,
        );
        let specs = vec![
            spec("limited", BillingClass::FreeTier, true, None, None, true),
            spec("flaky", BillingClass::FreeTier, true, None, None, true),
            spec("steady", BillingClass::Metered, true, None, None, true),
        ];
        let order = gate.select(RouteLane::FastChat, &specs, 2_000, false, &health, now);
        assert_eq!(order.len(), 1);
        assert_eq!(order[0].spec.id, "steady");
        // After the cooldown the limited provider is eligible again.
        let later = now + gate.rate_limit_cooldown + Duration::from_secs(1);
        let order = gate.select(
            RouteLane::FastChat,
            &specs[..1],
            2_000,
            false,
            &health,
            later,
        );
        assert_eq!(order.len(), 1);
    }

    #[test]
    fn a_failure_bench_expires_into_bounded_retries_and_success_recovers() {
        let gate = RouteGate::default();
        let now = Instant::now();
        let mut health = ProviderStateBook::default();
        // Three straight failures arm the bench.
        for _ in 0..3 {
            health.get_mut("flaky").observe(
                false,
                None,
                false,
                None,
                gate.rate_limit_cooldown,
                now,
            );
        }
        let specs = vec![
            spec("flaky", BillingClass::FreeTier, true, None, None, true),
            spec("steady", BillingClass::Metered, true, None, None, true),
        ];
        let ids = |health: &ProviderStateBook, at: Instant| {
            gate.select(RouteLane::FastChat, &specs, 2_000, false, health, at)
                .iter()
                .map(|c| c.spec.id)
                .collect::<Vec<_>>()
        };
        // Refused while the bench is fresh — a transient outage still
        // shields live requests.
        assert_eq!(ids(&health, now + Duration::from_secs(1)), ["steady"]);
        // The bench expires into a bounded retry: eligible again after
        // the interval, still eligible a day later — never a permanent
        // exclusion.
        let retry = now + gate.failure_retry_after + Duration::from_secs(1);
        assert_eq!(ids(&health, retry), ["flaky", "steady"]);
        assert_eq!(
            ids(&health, now + Duration::from_secs(86_400)),
            ["flaky", "steady"]
        );
        // A renewed failure re-arms the bench instead of retrying every
        // turn.
        health
            .get_mut("flaky")
            .observe(false, None, false, None, gate.rate_limit_cooldown, retry);
        assert_eq!(ids(&health, retry + Duration::from_secs(1)), ["steady"]);
        // An answered request — e.g. a forced-tier call served while the
        // provider was benched — is proof of life and clears the bench at
        // once.
        health.get_mut("flaky").observe(
            true,
            Some(800),
            false,
            None,
            gate.rate_limit_cooldown,
            retry + Duration::from_secs(2),
        );
        assert_eq!(
            ids(&health, retry + Duration::from_secs(3)),
            ["flaky", "steady"]
        );
        // Failing again re-benches for another full interval; the next
        // half-open retry still comes.
        health.get_mut("flaky").observe(
            false,
            None,
            false,
            None,
            gate.rate_limit_cooldown,
            retry + Duration::from_secs(4),
        );
        assert_eq!(ids(&health, retry + Duration::from_secs(5)), ["steady"]);
        let later =
            retry + Duration::from_secs(4) + gate.failure_retry_after + Duration::from_secs(1);
        assert_eq!(ids(&health, later), ["flaky", "steady"]);
    }

    #[test]
    fn in_flight_success_does_not_cancel_a_rate_limit_cooldown() {
        let gate = RouteGate::default();
        let now = Instant::now();
        let mut health = ProviderStateBook::default();
        let specs = [spec(
            "limited",
            BillingClass::FreeTier,
            true,
            None,
            None,
            true,
        )];
        health
            .get_mut("limited")
            .observe(false, None, true, None, gate.rate_limit_cooldown, now);
        health.get_mut("limited").observe(
            true,
            Some(100),
            false,
            None,
            gate.rate_limit_cooldown,
            now + Duration::from_secs(1),
        );
        assert!(
            gate.select(
                RouteLane::FastChat,
                &specs,
                100,
                false,
                &health,
                now + Duration::from_secs(2)
            )
            .is_empty()
        );
        assert_eq!(
            gate.select(
                RouteLane::FastChat,
                &specs,
                100,
                false,
                &health,
                now + gate.rate_limit_cooldown + Duration::from_secs(1)
            )
            .len(),
            1
        );
    }

    #[test]
    fn measured_latency_orders_within_a_billing_class() {
        let gate = RouteGate::default();
        let now = Instant::now();
        let mut health = ProviderStateBook::default();
        health.get_mut("slow-free").observe(
            true,
            Some(5_000),
            false,
            None,
            gate.rate_limit_cooldown,
            now,
        );
        health.get_mut("fast-free").observe(
            true,
            Some(300),
            false,
            None,
            gate.rate_limit_cooldown,
            now,
        );
        let specs = vec![
            spec("slow-free", BillingClass::FreeTier, true, None, None, true),
            spec("fast-free", BillingClass::FreeTier, true, None, None, true),
        ];
        let order = gate.select(RouteLane::FastChat, &specs, 2_000, false, &health, now);
        assert_eq!(order[0].spec.id, "fast-free");
    }

    #[test]
    fn unmeasured_and_stale_siblings_are_explored_within_a_band() {
        let gate = RouteGate::default();
        let now = Instant::now();
        let mut health = ProviderStateBook::default();
        health.get_mut("sub").observe(
            true,
            Some(2_000),
            false,
            None,
            gate.rate_limit_cooldown,
            now,
        );
        let specs = vec![
            spec("sub", BillingClass::Subscription, true, None, None, true),
            spec("local", BillingClass::Local, true, None, None, true),
            spec("paid", BillingClass::Metered, true, None, None, true),
        ];
        // The measured provider must not starve its unmeasured sibling; the
        // billing band still dominates (paid stays last).
        let ids = |order: Vec<RouteCandidate<'_, ProviderSpec>>| {
            order.iter().map(|c| c.spec.id).collect::<Vec<_>>()
        };
        let order = gate.select(RouteLane::FastChat, &specs, 2_000, false, &health, now);
        assert_eq!(ids(order), vec!["local", "sub", "paid"]);

        // Once measured slower, the faster sibling leads again ...
        health.get_mut("local").observe(
            true,
            Some(3_500),
            false,
            None,
            gate.rate_limit_cooldown,
            now,
        );
        let order = gate.select(RouteLane::FastChat, &specs, 2_000, false, &health, now);
        assert_eq!(ids(order), vec!["sub", "local", "paid"]);

        // ... until the loser's sample goes stale and it is sampled again.
        let later = now + gate.latency_explore_after;
        health.get_mut("sub").observe(
            true,
            Some(2_000),
            false,
            None,
            gate.rate_limit_cooldown,
            later,
        );
        let order = gate.select(RouteLane::FastChat, &specs, 2_000, false, &health, later);
        assert_eq!(ids(order), vec!["local", "sub", "paid"]);
    }

    #[test]
    fn stale_samples_past_the_fast_ceiling_are_not_explored_on_fast_chat() {
        let gate = RouteGate::default();
        let now = Instant::now();
        let mut health = ProviderStateBook::default();
        health.get_mut("slow").observe(
            true,
            Some(40_000),
            false,
            None,
            gate.rate_limit_cooldown,
            now,
        );
        health.get_mut("fast").observe(
            true,
            Some(2_000),
            false,
            None,
            gate.rate_limit_cooldown,
            now + gate.latency_explore_after,
        );
        let specs = vec![
            spec("slow", BillingClass::Local, true, None, None, true),
            spec("fast", BillingClass::Subscription, true, None, None, true),
        ];
        let later = now + gate.latency_explore_after;
        let order = gate.select(RouteLane::FastChat, &specs, 2_000, false, &health, later);
        assert_eq!(order[0].spec.id, "fast");
        // Lanes without a latency ceiling do re-sample it.
        let order = gate.select(RouteLane::ToolTask, &specs, 2_000, false, &health, later);
        assert_eq!(order[0].spec.id, "slow");
    }

    struct Preferring(ProviderSpec, RouteLane);
    impl RouteProvider for Preferring {
        fn route_id(&self) -> &str {
            self.0.route_id()
        }
        fn billing(&self) -> BillingClass {
            self.0.billing()
        }
        fn privacy_ok_for_private_memory(&self) -> bool {
            self.0.privacy_ok_for_private_memory()
        }
        fn context_limit_tokens(&self) -> Option<u64> {
            self.0.context_limit_tokens()
        }
        fn approx_tpm_limit(&self) -> Option<u64> {
            self.0.approx_tpm_limit()
        }
        fn reasoning_suppression(&self) -> bool {
            self.0.reasoning_suppression()
        }
        fn route_available(&self) -> bool {
            self.0.route_available()
        }
        fn preferred_for(&self, lane: RouteLane) -> bool {
            lane == self.1
        }
    }

    #[test]
    fn lane_preference_leads_only_its_lane_and_never_breaks_privacy() {
        let gate = RouteGate::default();
        let health = ProviderStateBook::default();
        let now = Instant::now();
        let specs = vec![
            Preferring(
                spec("sub", BillingClass::Subscription, true, None, None, true),
                RouteLane::MemoryHeavy,
            ),
            Preferring(
                spec("fast-paid", BillingClass::Metered, true, None, None, true),
                RouteLane::FastChat,
            ),
            Preferring(
                spec("fast-free", BillingClass::FreeTier, false, None, None, true),
                RouteLane::FastChat,
            ),
        ];
        let ids = |lane, private| {
            gate.select(lane, &specs, 2_000, private, &health, now)
                .iter()
                .map(|c| c.spec.route_id().to_owned())
                .collect::<Vec<_>>()
        };
        // Everyday chat: the preferred fast providers lead the billing order.
        assert_eq!(
            ids(RouteLane::FastChat, false),
            ["fast-free", "fast-paid", "sub"]
        );
        // A private turn still never reaches a training free tier.
        assert_eq!(ids(RouteLane::FastChat, true), ["fast-paid", "sub"]);
        // Other lanes keep the normal order.
        assert_eq!(ids(RouteLane::DeepReasoning, true), ["sub", "fast-paid"]);
    }

    #[test]
    fn reasoning_forced_loses_fast_lane_ties() {
        let gate = RouteGate::default();
        let specs = vec![
            spec("thinker", BillingClass::FreeTier, true, None, None, false),
            spec("quiet", BillingClass::FreeTier, true, None, None, true),
        ];
        let health = ProviderStateBook::default();
        let order = gate.select(
            RouteLane::FastChat,
            &specs,
            2_000,
            false,
            &health,
            Instant::now(),
        );
        assert_eq!(order[0].spec.id, "quiet");
        // On the reasoning lane suppression no longer matters for order.
        let order = gate.select(
            RouteLane::DeepReasoning,
            &specs,
            2_000,
            false,
            &health,
            Instant::now(),
        );
        assert_eq!(order.len(), 2);
    }

    #[test]
    fn local_chat_only_sees_local_providers() {
        let gate = RouteGate::default();
        let specs = vec![
            spec("local", BillingClass::Local, true, None, None, true),
            spec("free", BillingClass::FreeTier, true, None, None, true),
        ];
        let health = ProviderStateBook::default();
        let order = gate.select(
            RouteLane::LocalChat,
            &specs,
            2_000,
            false,
            &health,
            Instant::now(),
        );
        assert_eq!(order.len(), 1);
        assert_eq!(order[0].spec.id, "local");
    }

    #[test]
    fn metered_is_only_reached_when_free_paths_fail() {
        let gate = RouteGate::default();
        let now = Instant::now();
        let mut health = ProviderStateBook::default();
        // Free provider in a 429 cooldown — metered must still answer.
        health
            .get_mut("free")
            .observe(false, None, true, None, gate.rate_limit_cooldown, now);
        let specs = vec![
            spec("free", BillingClass::FreeTier, true, None, None, true),
            spec("paid", BillingClass::Metered, true, None, None, true),
        ];
        let order = gate.select(RouteLane::FastChat, &specs, 2_000, false, &health, now);
        assert_eq!(order.len(), 1);
        assert_eq!(order[0].spec.id, "paid");
        assert_eq!(order[0].reason, RouteReason::Primary);
    }
}
