//! Production routing policy for the resident daemon.
//!
//! The benchmark is useful only if production enforces the same economic and
//! privacy invariants.  This module keeps the hot path self-contained:
//! private-by-default eligibility, billing-class ordering, conservative prompt
//! sizing, and persistent spend caps for metered providers.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::{RoutingConfig, TierBilling, TierConfig};
use crate::util::{atomic_write, unix_now, utc_date};

/// Conservative prompt estimate used before a request is sent.  Counting the
/// complete JSON body includes tool schemas and other context that a
/// messages-only estimate would miss.
pub fn estimated_prompt_tokens(body: &Value) -> u64 {
    u64::try_from(body.to_string().len() / 3).unwrap_or(u64::MAX).max(64)
}

/// Auto-routed requests are private unless an internal caller explicitly
/// marks a request public.  Persona dialogue therefore never enters a
/// privacy-incompatible free tier by accident.
pub fn request_is_private(body: &Value) -> bool {
    body.get("kamimusuhi_private")
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

/// Resident-only routing metadata must never be forwarded to an upstream.
pub fn strip_internal_fields(body: &mut Value) {
    if let Some(map) = body.as_object_mut() {
        map.remove("kamimusuhi_private");
    }
}

fn privacy_ok(tier: &TierConfig) -> bool {
    tier.privacy_ok_for_private_memory
        .unwrap_or(!matches!(tier.billing, Some(TierBilling::FreeTier)))
}

/// Return a stable, operator-readable reason when a tier cannot accept the
/// request.  Unknown billing is not treated as free, but it is otherwise
/// eligible for backwards compatibility.
pub fn ineligible_reason(
    tier: &TierConfig,
    private: bool,
    prompt_tokens: u64,
) -> Option<&'static str> {
    if private && !privacy_ok(tier) {
        return Some("privacy policy");
    }
    if tier
        .context_limit_tokens
        .is_some_and(|limit| prompt_tokens > limit)
    {
        return Some("context limit");
    }
    if tier
        .approx_tpm_limit
        .is_some_and(|limit| prompt_tokens > limit)
    {
        return Some("token/minute limit");
    }
    None
}

fn billing_rank(billing: Option<TierBilling>, prefer_local: bool) -> u8 {
    if prefer_local {
        return match billing {
            Some(TierBilling::Local) => 0,
            Some(TierBilling::FreeTier) => 1,
            Some(TierBilling::Subscription) => 2,
            Some(TierBilling::Metered) => 3,
            None => 4,
        };
    }
    match billing {
        Some(TierBilling::FreeTier) => 0,
        Some(TierBilling::Subscription) => 1,
        Some(TierBilling::Local) => 2,
        Some(TierBilling::Metered) => 3,
        None => 4,
    }
}

/// Apply production eligibility and billing order while preserving config
/// order within a class.
pub fn order_tiers<'a>(
    mut tiers: Vec<&'a TierConfig>,
    private: bool,
    prompt_tokens: u64,
    prefer_local: bool,
) -> Vec<&'a TierConfig> {
    tiers.retain(|tier| ineligible_reason(tier, private, prompt_tokens).is_none());
    tiers.sort_by_key(|tier| billing_rank(tier.billing, prefer_local));
    tiers
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SpendLedger {
    #[serde(default)]
    daily: BTreeMap<String, f64>,
    #[serde(default)]
    monthly: BTreeMap<String, f64>,
}

impl SpendLedger {
    fn load(path: &std::path::Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn record(&mut self, date: &str, usd: f64) {
        *self.daily.entry(date.to_owned()).or_default() += usd;
        *self.monthly.entry(date[..7].to_owned()).or_default() += usd;
    }

    fn day_total(&self, date: &str) -> f64 {
        self.daily.get(date).copied().unwrap_or(0.0)
    }

    fn month_total(&self, month: &str) -> f64 {
        self.monthly.get(month).copied().unwrap_or(0.0)
    }
}

/// Production spend guard.  Metered calls are serialized through the guard's
/// mutex in `Shared`, so concurrent requests cannot race past a cap.
#[derive(Debug)]
pub struct CostGuard {
    request_cap_usd: f64,
    session_cap_usd: f64,
    daily_cap_usd: f64,
    monthly_cap_usd: f64,
    default_completion_tokens: u32,
    session_spent_usd: f64,
    ledger: SpendLedger,
    ledger_path: PathBuf,
}

impl CostGuard {
    pub fn new(config: &RoutingConfig, ledger_path: PathBuf) -> Self {
        Self {
            request_cap_usd: config.max_request_cost_usd,
            session_cap_usd: config.max_session_cost_usd,
            daily_cap_usd: config.max_daily_cost_usd,
            monthly_cap_usd: config.max_monthly_cost_usd,
            default_completion_tokens: config.max_completion_tokens_estimate,
            session_spent_usd: 0.0,
            ledger: SpendLedger::load(&ledger_path),
            ledger_path,
        }
    }

    fn max_completion_tokens(&self, body: &Value) -> u32 {
        body.get("max_completion_tokens")
            .or_else(|| body.get("max_tokens"))
            .and_then(Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(self.default_completion_tokens)
    }

    /// Check a metered request before sending it and return the worst-case
    /// estimate reserved by the caller.  Unknown metered prices fail closed.
    pub fn permit(
        &self,
        tier: &TierConfig,
        body: &Value,
        prompt_tokens: u64,
    ) -> Result<f64, String> {
        let (Some(input), Some(output)) = (
            tier.input_usd_per_mtok,
            tier.output_usd_per_mtok,
        ) else {
            return Err(format!(
                "metered tier {} has no configured input/output price",
                tier.name
            ));
        };
        let estimate = (prompt_tokens as f64 * input
            + f64::from(self.max_completion_tokens(body)) * output)
            / 1_000_000.0;
        if estimate > self.request_cap_usd {
            return Err(format!(
                "estimated ${estimate:.4}/request exceeds ${:.2} cap",
                self.request_cap_usd
            ));
        }
        if self.session_spent_usd + estimate > self.session_cap_usd {
            return Err(format!("session cost cap ${:.2} reached", self.session_cap_usd));
        }
        let today = utc_date(unix_now());
        if self.ledger.day_total(&today) + estimate > self.daily_cap_usd {
            return Err(format!("daily cost cap ${:.2} reached", self.daily_cap_usd));
        }
        if self.ledger.month_total(&today[..7]) + estimate > self.monthly_cap_usd {
            return Err(format!("monthly cost cap ${:.2} reached", self.monthly_cap_usd));
        }
        Ok(estimate)
    }

    pub fn record_cost(&mut self, usd: f64) {
        if !usd.is_finite() || usd <= 0.0 {
            return;
        }
        self.session_spent_usd += usd;
        let today = utc_date(unix_now());
        self.ledger.record(&today, usd);
        if let Ok(body) = serde_json::to_vec_pretty(&self.ledger) {
            let _ = atomic_write(&self.ledger_path, &body);
        }
    }

    pub fn view(&self) -> Value {
        let today = utc_date(unix_now());
        json!({
            "session_usd": self.session_spent_usd,
            "daily_usd": self.ledger.day_total(&today),
            "monthly_usd": self.ledger.month_total(&today[..7]),
            "request_cap_usd": self.request_cap_usd,
            "session_cap_usd": self.session_cap_usd,
            "daily_cap_usd": self.daily_cap_usd,
            "monthly_cap_usd": self.monthly_cap_usd,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{TierCondition, TierConfig};

    fn tier(name: &str, billing: TierBilling) -> TierConfig {
        TierConfig {
            name: name.to_owned(),
            base_url: "https://example.invalid/v1".to_owned(),
            model: "m".to_owned(),
            auth_env: None,
            timeout_secs: 1,
            condition: TierCondition::Always,
            node_local: billing == TierBilling::Local,
            peer_local_only: false,
            probe_interval_secs: 1,
            billing: Some(billing),
            input_usd_per_mtok: None,
            output_usd_per_mtok: None,
            privacy_ok_for_private_memory: None,
            context_limit_tokens: None,
            approx_tpm_limit: None,
            reasoning_suppression: false,
        }
    }

    #[test]
    fn private_defaults_block_free_tier() {
        let free = tier("free", TierBilling::FreeTier);
        assert_eq!(ineligible_reason(&free, true, 100), Some("privacy policy"));
        assert_eq!(ineligible_reason(&free, false, 100), None);
    }

    #[test]
    fn billing_order_matches_fast_chat_policy() {
        let free = tier("free", TierBilling::FreeTier);
        let sub = tier("sub", TierBilling::Subscription);
        let local = tier("local", TierBilling::Local);
        let paid = tier("paid", TierBilling::Metered);
        let order = order_tiers(vec![&paid, &local, &sub, &free], false, 100, false)
            .into_iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(order, ["free", "sub", "local", "paid"]);
    }

    #[test]
    fn prompt_estimate_includes_tools() {
        let small = json!({"messages":[{"role":"user","content":"hi"}]});
        let large = json!({"messages":[{"role":"user","content":"hi"}],
                           "tools":[{"type":"function","function":{"name":"x",
                           "description":"x".repeat(3000)}}]});
        assert!(estimated_prompt_tokens(&large) > estimated_prompt_tokens(&small));
    }
}
