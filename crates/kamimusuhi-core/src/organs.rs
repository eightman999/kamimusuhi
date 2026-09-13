//! Contracts for promoting validated experiments into runtime cognitive organs.
//!
//! Experiments are not allowed to become identity owners by being useful.
//! A promoted organ produces transient, attributed [`OrganSignal`] values.
//! Those signals may influence cognition when explicitly activated, but they
//! are neither canonical evidence nor durable self-state and never authorize a
//! mutation by themselves.
//!
//! The contract deliberately separates three states of promotion:
//! - [`PromotionMode::Active`]: a replicated PASS result may influence cognition;
//! - [`PromotionMode::Shadow`]: run and observe, but do not influence cognition;
//! - [`PromotionMode::Excluded`]: negative/invalid results are not runnable organs.
//!
//! This keeps experimental provenance attached to the deployed mechanism and
//! makes "we had a promising notebook" insufficient grounds for silently
//! changing runtime behavior.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::EvidenceId;
use crate::time::UtcTimestamp;

/// Stable operator-facing key of an organ implementation.
///
/// Unlike runtime record IDs, this is a deployment key (for example
/// `h0-homeostasis-gru64`) and therefore intentionally survives restarts and
/// re-instantiation of the same implementation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OrganKey(String);

impl OrganKey {
    pub fn new(value: impl Into<String>) -> Result<Self, OrganError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 96
            && value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
        if !valid {
            return Err(OrganError::InvalidDescriptor {
                reason: "organ key must be 1..=96 lowercase ASCII letters/digits/-/_".to_owned(),
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for OrganKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Functional role exposed to the runtime. These are behavioral contracts,
/// not claims about biological equivalence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrganRole {
    Regulation,
    MemoryGate,
    AgencyAttribution,
    TemporalState,
    ObjectState,
    FastControl,
}

/// Verdict of the experiment that justifies (or blocks) promotion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentVerdict {
    Pass,
    Partial,
    Fail,
}

/// How a validated mechanism is allowed to participate in live cognition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionMode {
    /// Output may enter the live cognitive path. Requires a PASS verdict.
    Active,
    /// Execute and trace the output, but do not feed it into live cognition.
    Shadow,
    /// Explicitly not a runtime organ (e.g. a negative result/control).
    Excluded,
}

/// Experimental evidence carried with a deployed organ.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrganEvidence {
    /// Repository experiment name, e.g. `H0` or `R0`.
    pub experiment: String,
    pub verdict: ExperimentVerdict,
    /// Commit/tree/revision used for the evidence when known.
    pub source_revision: Option<String>,
    /// Human-inspectable report path, never interpreted as authority.
    pub report_path: Option<String>,
}

/// Description of one deployable cognitive organ.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrganDescriptor {
    pub key: OrganKey,
    pub role: OrganRole,
    /// Implementation family (`gru64`, `native-rust`, `process-adapter`, ...).
    pub implementation: String,
    pub version: String,
    pub promotion: PromotionMode,
    pub evidence: OrganEvidence,
}

impl OrganDescriptor {
    pub fn validate(&self) -> Result<(), OrganError> {
        if self.implementation.trim().is_empty() {
            return Err(OrganError::InvalidDescriptor {
                reason: "implementation is empty".to_owned(),
            });
        }
        if self.version.trim().is_empty() {
            return Err(OrganError::InvalidDescriptor {
                reason: "version is empty".to_owned(),
            });
        }
        if self.evidence.experiment.trim().is_empty() {
            return Err(OrganError::InvalidDescriptor {
                reason: "experiment is empty".to_owned(),
            });
        }
        if self.promotion == PromotionMode::Active
            && self.evidence.verdict != ExperimentVerdict::Pass
        {
            return Err(OrganError::InvalidDescriptor {
                reason: "active promotion requires a PASS experiment verdict".to_owned(),
            });
        }
        Ok(())
    }

    pub const fn influences_cognition(&self) -> bool {
        matches!(self.promotion, PromotionMode::Active)
    }
}

/// Input supplied to one organ invocation.
///
/// `payload` is intentionally organ-specific. The stable contract is the
/// attribution around it: observation time and the canonical evidence records
/// (if any) from which it was derived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrganInput {
    pub observed_at: UtcTimestamp,
    pub payload: serde_json::Value,
    #[serde(default)]
    pub evidence_refs: Vec<EvidenceId>,
}

/// One transient derived signal emitted by an organ.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrganSignal {
    pub descriptor: OrganDescriptor,
    pub produced_at: UtcTimestamp,
    /// Bounded lifetime declared by the organ. Consumers still use their own
    /// monotonic freshness deadline; this field documents intended lifetime.
    pub ttl_ms: u64,
    /// Optional calibrated confidence in thousandths, 0..=1000.
    pub confidence_milli: Option<u16>,
    pub payload: serde_json::Value,
    #[serde(default)]
    pub evidence_refs: Vec<EvidenceId>,
}

impl OrganSignal {
    pub fn validate(&self) -> Result<(), OrganError> {
        self.descriptor.validate()?;
        if self.descriptor.promotion == PromotionMode::Excluded {
            return Err(OrganError::Excluded {
                key: self.descriptor.key.clone(),
            });
        }
        if self.ttl_ms == 0 {
            return Err(OrganError::InvalidOutput {
                reason: "ttl_ms must be non-zero".to_owned(),
            });
        }
        if self.confidence_milli.is_some_and(|value| value > 1000) {
            return Err(OrganError::InvalidOutput {
                reason: "confidence_milli must be <= 1000".to_owned(),
            });
        }
        Ok(())
    }

    /// A derived signal is never mutation authority. If it should produce a
    /// durable conclusion, some later component must cite real evidence and go
    /// through the normal proposal/policy/Continuity Kernel path.
    pub const fn authorizes_mutation(&self) -> bool {
        false
    }

    pub const fn influences_cognition(&self) -> bool {
        self.descriptor.influences_cognition()
    }
}

/// A replaceable runtime organ. Stateful implementations (GRU hidden state,
/// trackers, timers) keep that state inside the implementation; none of it is
/// canonical merely because it is resident here.
pub trait CognitiveOrgan: Send {
    fn descriptor(&self) -> OrganDescriptor;
    fn process(&mut self, input: &OrganInput) -> Result<OrganSignal, OrganError>;
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OrganError {
    #[error("invalid organ descriptor: {reason}")]
    InvalidDescriptor { reason: String },
    #[error("invalid organ input: {reason}")]
    InvalidInput { reason: String },
    #[error("invalid organ output: {reason}")]
    InvalidOutput { reason: String },
    #[error("organ {key} is excluded from runtime promotion")]
    Excluded { key: OrganKey },
    #[error("duplicate organ key {key}")]
    Duplicate { key: OrganKey },
    /// Adapter-specific failures carry a stable code, not backend prose.
    #[error("organ backend failure: {code}")]
    Backend { code: String },
}

impl OrganError {
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidDescriptor { .. } => "INVALID_DESCRIPTOR",
            Self::InvalidInput { .. } => "INVALID_INPUT",
            Self::InvalidOutput { .. } => "INVALID_OUTPUT",
            Self::Excluded { .. } => "EXCLUDED",
            Self::Duplicate { .. } => "DUPLICATE",
            Self::Backend { .. } => "BACKEND",
        }
    }
}

/// One failed invocation. Failures are operational observations and do not
/// create canonical evidence by themselves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrganFailure {
    pub key: OrganKey,
    pub code: String,
}

/// Deterministic result of one supervisor cycle.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct OrganCycle {
    /// Signals allowed to influence live cognition.
    pub active: Vec<OrganSignal>,
    /// Signals retained for comparison/telemetry only.
    pub shadow: Vec<OrganSignal>,
    pub failures: Vec<OrganFailure>,
}

/// Runtime registry/supervisor for promoted organs.
///
/// Registration and execution order are sorted by [`OrganKey`], not insertion
/// order, so a configuration rewrite cannot accidentally change cognition by
/// reordering otherwise identical organs.
#[derive(Default)]
pub struct OrganSupervisor {
    organs: Vec<Box<dyn CognitiveOrgan>>,
}

impl fmt::Debug for OrganSupervisor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let keys: Vec<OrganKey> = self.organs.iter().map(|o| o.descriptor().key).collect();
        f.debug_struct("OrganSupervisor").field("organs", &keys).finish()
    }
}

impl OrganSupervisor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, organ: Box<dyn CognitiveOrgan>) -> Result<(), OrganError> {
        let descriptor = organ.descriptor();
        descriptor.validate()?;
        if descriptor.promotion == PromotionMode::Excluded {
            return Err(OrganError::Excluded {
                key: descriptor.key,
            });
        }
        if self
            .organs
            .iter()
            .any(|registered| registered.descriptor().key == descriptor.key)
        {
            return Err(OrganError::Duplicate {
                key: descriptor.key,
            });
        }
        self.organs.push(organ);
        self.organs.sort_by_key(|organ| organ.descriptor().key);
        Ok(())
    }

    pub fn descriptors(&self) -> Vec<OrganDescriptor> {
        self.organs.iter().map(|organ| organ.descriptor()).collect()
    }

    pub fn run(&mut self, input: &OrganInput) -> OrganCycle {
        let mut cycle = OrganCycle::default();
        for organ in &mut self.organs {
            let expected = organ.descriptor();
            let result = organ.process(input).and_then(|signal| {
                if signal.descriptor != expected {
                    return Err(OrganError::InvalidOutput {
                        reason: "output descriptor differs from registered descriptor".to_owned(),
                    });
                }
                signal.validate()?;
                Ok(signal)
            });
            match result {
                Ok(signal) if signal.influences_cognition() => cycle.active.push(signal),
                Ok(signal) => cycle.shadow.push(signal),
                Err(error) => cycle.failures.push(OrganFailure {
                    key: expected.key,
                    code: error.code().to_owned(),
                }),
            }
        }
        cycle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: UtcTimestamp = UtcTimestamp::from_unix_millis(1_788_825_600_000);

    fn descriptor(key: &str, verdict: ExperimentVerdict, promotion: PromotionMode) -> OrganDescriptor {
        OrganDescriptor {
            key: OrganKey::new(key).unwrap(),
            role: OrganRole::Regulation,
            implementation: "fixture".to_owned(),
            version: "1".to_owned(),
            promotion,
            evidence: OrganEvidence {
                experiment: "H0".to_owned(),
                verdict,
                source_revision: Some("fixture".to_owned()),
                report_path: Some("experiments/h0/reports/H0_RESULTS.md".to_owned()),
            },
        }
    }

    struct FixtureOrgan {
        descriptor: OrganDescriptor,
        value: i64,
    }

    impl CognitiveOrgan for FixtureOrgan {
        fn descriptor(&self) -> OrganDescriptor {
            self.descriptor.clone()
        }

        fn process(&mut self, input: &OrganInput) -> Result<OrganSignal, OrganError> {
            Ok(OrganSignal {
                descriptor: self.descriptor.clone(),
                produced_at: input.observed_at,
                ttl_ms: 100,
                confidence_milli: Some(900),
                payload: serde_json::json!({"value": self.value}),
                evidence_refs: input.evidence_refs.clone(),
            })
        }
    }

    #[test]
    fn active_requires_pass() {
        let partial = descriptor("o0-object-state", ExperimentVerdict::Partial, PromotionMode::Active);
        assert_eq!(partial.validate().unwrap_err().code(), "INVALID_DESCRIPTOR");
    }

    #[test]
    fn signal_never_authorizes_mutation() {
        let signal = OrganSignal {
            descriptor: descriptor("h0-regulation", ExperimentVerdict::Pass, PromotionMode::Active),
            produced_at: AT,
            ttl_ms: 100,
            confidence_milli: Some(1000),
            payload: serde_json::json!({"energy": 0.7}),
            evidence_refs: Vec::new(),
        };
        assert!(!signal.authorizes_mutation());
        assert!(signal.influences_cognition());
    }

    #[test]
    fn supervisor_separates_active_and_shadow_and_is_key_ordered() {
        let mut supervisor = OrganSupervisor::new();
        supervisor
            .register(Box::new(FixtureOrgan {
                descriptor: descriptor(
                    "z-shadow",
                    ExperimentVerdict::Partial,
                    PromotionMode::Shadow,
                ),
                value: 2,
            }))
            .unwrap();
        supervisor
            .register(Box::new(FixtureOrgan {
                descriptor: descriptor(
                    "a-active",
                    ExperimentVerdict::Pass,
                    PromotionMode::Active,
                ),
                value: 1,
            }))
            .unwrap();

        assert_eq!(supervisor.descriptors()[0].key.as_str(), "a-active");
        let cycle = supervisor.run(&OrganInput {
            observed_at: AT,
            payload: serde_json::json!({}),
            evidence_refs: Vec::new(),
        });
        assert_eq!(cycle.active.len(), 1);
        assert_eq!(cycle.shadow.len(), 1);
        assert!(cycle.failures.is_empty());
        assert_eq!(cycle.active[0].descriptor.key.as_str(), "a-active");
        assert_eq!(cycle.shadow[0].descriptor.key.as_str(), "z-shadow");
    }

    #[test]
    fn excluded_negative_result_cannot_be_registered() {
        let mut supervisor = OrganSupervisor::new();
        let result = supervisor.register(Box::new(FixtureOrgan {
            descriptor: descriptor(
                "p0-surprise",
                ExperimentVerdict::Fail,
                PromotionMode::Excluded,
            ),
            value: 0,
        }));
        assert_eq!(result.unwrap_err().code(), "EXCLUDED");
    }
}
