//! Read-only, explicitly pinned MIO observation bridge. An experiment genome
//! is not a canonical identity, and completed evaluations are not live senses.

use std::collections::BTreeMap;
use std::time::Duration;

use kamimusuhi_core::digest::json_digest;
use kamimusuhi_core::time::Clock;
use kamimusuhi_resource_http::http::{Endpoint, get_json};
use kamimusuhi_resource_http::tls::TrustAnchors;
use serde::{Deserialize, Serialize};

use crate::RuntimeError;

const MAX_SNAPSHOT_BYTES: usize = 32_768;
const CLOCK_SKEW_MS: i64 = 5_000;

fn default_age_ms() -> u64 {
    300_000
}
fn default_timeout_ms() -> u64 {
    2_000
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MioBinding {
    pub base_url: String,
    pub experiment_id: String,
    pub genome_id: String,
    #[serde(default = "default_age_ms")]
    pub max_age_ms: u64,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn identifier(value: &str) -> bool {
    value
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_:.".contains(&b))
}

impl MioBinding {
    pub fn new(base_url: String, experiment_id: String, genome_id: String) -> Self {
        Self {
            base_url,
            experiment_id,
            genome_id,
            max_age_ms: default_age_ms(),
            timeout_ms: default_timeout_ms(),
        }
    }

    pub fn validate(&self) -> Result<(), RuntimeError> {
        if !identifier(&self.experiment_id)
            || !identifier(&self.genome_id)
            || !(1_000..=86_400_000).contains(&self.max_age_ms)
            || !(1..=30_000).contains(&self.timeout_ms)
        {
            return Err(RuntimeError::Usage(
                "invalid MIO binding IDs, age or timeout".to_owned(),
            ));
        }
        self.endpoint()
            .map_err(|_| RuntimeError::Usage("invalid MIO endpoint".to_owned()))?;
        Ok(())
    }

    fn endpoint(&self) -> Result<Endpoint, String> {
        Endpoint::parse(
            &self.base_url,
            &format!("/api/dialogue/organisms/{}", self.genome_id),
        )
    }

    /// Every turn performs a new read. No cached prior success is presented
    /// as current on transport or validation failure. No model API key or
    /// user conversation is sent to the MIO coordinator.
    pub fn observe(&self, clock: &dyn Clock) -> MioObservation {
        let mut received_at = clock.now_utc();
        let mut observation = MioObservation {
            connection: "unavailable".to_owned(),
            association: "operator_selected_mio".to_owned(),
            experiment_id: self.experiment_id.clone(),
            genome_id: self.genome_id.clone(),
            received_at_unix_ms: received_at.unix_millis(),
            error_code: None,
            snapshot_digest: None,
            snapshot: None,
            evaluation_freshness: None,
            evaluation_age_ms: None,
        };
        let result = (|| {
            self.validate().map_err(|_| "invalid_binding")?;
            let endpoint = self.endpoint().map_err(|_| "invalid_binding")?;
            let response = get_json(
                &endpoint,
                &[],
                Duration::from_millis(self.timeout_ms),
                &TrustAnchors::default(),
            );
            received_at = clock.now_utc();
            let response = response.map_err(|_| "source_unreachable")?;
            if response.status != 200 {
                return Err("source_http_error");
            }
            if response.body.len() > MAX_SNAPSHOT_BYTES {
                return Err("snapshot_too_large");
            }
            let snapshot: MioSnapshot =
                serde_json::from_str(&response.body).map_err(|_| "invalid_snapshot")?;
            snapshot.validate(self, received_at.unix_millis())?;
            Ok(snapshot)
        })();
        observation.received_at_unix_ms = received_at.unix_millis();
        match result {
            Ok(snapshot) => {
                observation.connection = "connected".to_owned();
                observation.snapshot_digest = Some(json_digest(&serde_json::json!(snapshot)));
                if let Some(evaluation) = &snapshot.evaluation {
                    if let Some(finished) = evaluation.finished_at_unix_ms {
                        let age = received_at.unix_millis().saturating_sub(finished).max(0) as u64;
                        observation.evaluation_age_ms = Some(age);
                        observation.evaluation_freshness = Some(
                            if age <= self.max_age_ms {
                                "recent_record"
                            } else {
                                "stale_record"
                            }
                            .to_owned(),
                        );
                    } else {
                        observation.evaluation_freshness = Some("unknown_timestamp".to_owned());
                    }
                } else {
                    // An absent record also covers malformed stored evaluations;
                    // it does not prove the organism has never been evaluated.
                    observation.evaluation_freshness = Some("no_record".to_owned());
                }
                observation.snapshot = Some(snapshot);
            }
            Err(code) => observation.error_code = Some(code.to_owned()),
        }
        observation
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MioObservation {
    pub connection: String,
    pub association: String,
    pub experiment_id: String,
    pub genome_id: String,
    pub received_at_unix_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<MioSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evaluation_freshness: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evaluation_age_ms: Option<u64>,
}

impl MioObservation {
    /// Fixed recall cues from validated state; no external prose is promoted
    /// into a retrieval instruction, and no second network read is needed.
    pub fn research_topics(&self) -> &'static str {
        if self.connection != "connected" {
            " 観測欠損 遮蔽 個体ID"
        } else if matches!(
            self.evaluation_freshness.as_deref(),
            Some("stale_record" | "unknown_timestamp")
        ) {
            " 時間 鮮度 記録"
        } else if self
            .snapshot
            .as_ref()
            .and_then(|s| s.evaluation.as_ref())
            .is_some_and(|e| {
                e.metrics.contains_key("homeostasis_score") || e.metrics.contains_key("peak_debt")
            })
        {
            " 恒常性 負債"
        } else {
            " 評価記録"
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MioSnapshot {
    schema_version: u32,
    kind: String,
    state_scope: String,
    observed_at_unix_ms: i64,
    experiment_id: String,
    genome_id: String,
    experiment_status: String,
    generation: u64,
    structure: MioStructure,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    implementation: Option<MioImplementation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    evaluation: Option<MioEvaluation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MioImplementation {
    action_feedback: String,
    lifetime_plasticity: String,
    neural_state_scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MioStructure {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    n_artificial_neurons: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    n_enabled_organs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    n_enabled_attachments: Option<u64>,
    #[serde(default)]
    counts: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MioEvaluation {
    kind: String,
    evaluation_id: String,
    job_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    finished_at_unix_ms: Option<i64>,
    backend: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scientific_config_hash: Option<String>,
    summary: BTreeMap<String, serde_json::Number>,
    metrics: BTreeMap<String, f64>,
}

impl MioSnapshot {
    fn validate(&self, binding: &MioBinding, now: i64) -> Result<(), &'static str> {
        if self.schema_version != 1
            || self.kind != "DERIVED"
            || self.state_scope != "recorded_evaluation"
        {
            return Err("unsupported_snapshot");
        }
        if self.experiment_id != binding.experiment_id || self.genome_id != binding.genome_id {
            return Err("mio_identity_mismatch");
        }
        if !["created", "running", "paused", "stopping", "stopped"]
            .contains(&self.experiment_status.as_str())
        {
            return Err("invalid_snapshot");
        }
        if self.observed_at_unix_ms <= 0
            || self.observed_at_unix_ms > now.saturating_add(CLOCK_SKEW_MS)
        {
            return Err("source_clock_invalid");
        }
        if now.saturating_sub(self.observed_at_unix_ms) > binding.max_age_ms as i64 {
            return Err("snapshot_stale");
        }
        if self.structure.counts.keys().any(|key| {
            ![
                "functional",
                "neutral_structure",
                "invalid_structure",
                "disabled",
            ]
            .contains(&key.as_str())
        }) {
            return Err("invalid_structure");
        }
        if self.implementation.as_ref().is_some_and(|implementation| {
            implementation.action_feedback != "not_connected"
                || implementation.lifetime_plasticity != "configured_not_applied"
                || implementation.neural_state_scope != "evaluation_local"
        }) {
            return Err("unsupported_implementation");
        }
        if let Some(evaluation) = &self.evaluation {
            if evaluation.kind != "RECORDED"
                || !identifier(&evaluation.evaluation_id)
                || !identifier(&evaluation.job_id)
                || !["mock", "torch", "genn"].contains(&evaluation.backend.as_str())
                || evaluation
                    .scientific_config_hash
                    .as_ref()
                    .is_some_and(|hash| {
                        hash.len() != 64
                            || !hash
                                .bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    })
            {
                return Err("invalid_evaluation");
            }
            if evaluation.finished_at_unix_ms.is_some_and(|time| {
                time <= 0 || time > self.observed_at_unix_ms.saturating_add(CLOCK_SKEW_MS)
            }) {
                return Err("evaluation_clock_invalid");
            }
            for (key, number) in &evaluation.summary {
                let value = number.as_f64().ok_or("invalid_activity")?;
                if !value.is_finite()
                    || value < 0.0
                    || ![
                        "mean_rate_hz",
                        "rate_std_hz",
                        "active_fraction",
                        "spikes_total",
                        "t_ms",
                    ]
                    .contains(&key.as_str())
                    || (key == "active_fraction" && value > 1.0)
                    || (key == "spikes_total" && number.as_u64().is_none())
                {
                    return Err("invalid_activity");
                }
            }
            for (key, value) in &evaluation.metrics {
                if !value.is_finite()
                    || ![
                        "homeostasis_score",
                        "disturbance_recovery_score",
                        "peak_debt",
                        "terminated_fraction",
                        "task_score",
                    ]
                    .contains(&key.as_str())
                {
                    return Err("invalid_metrics");
                }
                if [
                    "homeostasis_score",
                    "disturbance_recovery_score",
                    "terminated_fraction",
                ]
                .contains(&key.as_str())
                    && !(0.0..=1.0).contains(value)
                    || (key == "peak_debt" && *value < 0.0)
                {
                    return Err("invalid_metrics");
                }
            }
        }
        Ok(())
    }
}
