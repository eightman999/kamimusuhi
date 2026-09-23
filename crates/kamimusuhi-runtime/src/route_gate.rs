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

use serde::Serialize;

use crate::provider_bench::ProviderSpec;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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

impl RouteLane {
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
        if ok {
            self.consecutive_failures = 0;
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
pub struct RouteCandidate<'a> {
    pub spec: &'a ProviderSpec,
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
    /// EWMA first-token latency above which a provider stops being usable
    /// for [`RouteLane::FastChat`]. A slow-but-free provider is not "usable"
    /// for conversation; beyond this ceiling the turn falls through to the
    /// next class. Other lanes have no ceiling — a background task can wait.
    pub fast_chat_ttft_ceiling_ms: Option<u64>,
}

impl Default for RouteGate {
    fn default() -> Self {
        Self {
            rate_limit_cooldown: Duration::from_secs(60),
            failure_rate_threshold: 0.5,
            failure_window_min: 3,
            // Thirty seconds to the first token is where "slow" stops being
            // conversation: measured free-tier reasoning upstreams sit at
            // 12-48s, fast clouds at 0.4-2s, HAI at 4-22s.
            fast_chat_ttft_ceiling_ms: Some(30_000),
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
    pub fn select<'a>(
        &self,
        lane: RouteLane,
        specs: &'a [ProviderSpec],
        prompt_tokens_est: u64,
        private: bool,
        health: &ProviderStateBook,
        now: Instant,
    ) -> Vec<RouteCandidate<'a>> {
        debug_assert!(lane.is_chat());
        let mut candidates: Vec<RouteCandidate<'a>> = Vec::new();
        let mut skipped_for_privacy = false;
        let mut skipped_for_context = false;
        let mut skipped_for_health = false;

        for spec in specs {
            if spec.skip_reason.is_some() || spec.base_url.is_empty() {
                continue;
            }
            // Privacy is a hard wall: a provider whose plan may train on
            // inputs never sees private material, whatever the speed.
            if private && !spec.privacy_ok_for_private_memory {
                skipped_for_privacy = true;
                continue;
            }
            // Context ceiling: a provider that cannot hold the prompt is
            // not a candidate at all.
            if let Some(limit) = spec.context_limit_tokens
                && prompt_tokens_est > u64::from(limit)
            {
                skipped_for_context = true;
                continue;
            }
            // Free-tier token ceilings are approximate; a prompt that would
            // blow through the minute budget is sent elsewhere rather than
            // eaten by a 429.
            if let Some(tpm) = spec.approx_tpm_limit
                && prompt_tokens_est > tpm
            {
                skipped_for_context = true;
                continue;
            }
            if let Some(state) = health.get(spec.id) {
                if state.rate_limited_until.is_some_and(|until| now < until) {
                    skipped_for_health = true;
                    continue;
                }
                if state.window_requests >= self.failure_window_min
                    && state
                        .failure_rate()
                        .is_some_and(|rate| rate > self.failure_rate_threshold)
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
                {
                    skipped_for_health = true;
                    continue;
                }
            }
            candidates.push(RouteCandidate {
                spec,
                reason: RouteReason::Primary,
                est_ttft_ms: health
                    .get(spec.id)
                    .and_then(|s| s.ewma_ttft_ms)
                    .map(|v| v as u64),
            });
        }

        // Lane-specific class preference: LocalChat wants Local first;
        // DeepReasoning tolerates cost; everything else is cost-minimal.
        let class_rank = |class: BillingClass| -> usize {
            let order: &[BillingClass] = match lane {
                RouteLane::LocalChat => &[BillingClass::Local],
                RouteLane::DeepReasoning | RouteLane::ToolTask | RouteLane::MemoryHeavy => &[
                    BillingClass::Subscription,
                    BillingClass::Local,
                    BillingClass::Metered,
                    BillingClass::FreeTier,
                ],
                _ => &BILLING_ORDER,
            };
            order
                .iter()
                .position(|c| *c == class)
                .unwrap_or(order.len())
        };
        let lane_allows = |class: BillingClass| -> bool {
            match lane {
                RouteLane::LocalChat => class == BillingClass::Local,
                _ => true,
            }
        };
        candidates.retain(|c| lane_allows(c.spec.billing));

        // Within a billing class: measured TTFT first, reasoning-capable
        // before reasoning-forced on the fast lane (thinking time is TTFT).
        candidates.sort_by_key(|c| {
            let reasoning_penalty = match lane {
                RouteLane::FastChat | RouteLane::LocalChat => {
                    usize::from(!c.spec.reasoning_suppression)
                }
                _ => 0,
            };
            (
                class_rank(c.spec.billing),
                reasoning_penalty,
                c.est_ttft_ms.unwrap_or(u64::MAX),
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
            .map(|c| class_rank(c.spec.billing))
            .unwrap_or(0);
        for (index, candidate) in candidates.iter_mut().enumerate() {
            candidate.reason = if index == 0 {
                RouteReason::Primary
            } else if class_rank(candidate.spec.billing) > primary_rank {
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
