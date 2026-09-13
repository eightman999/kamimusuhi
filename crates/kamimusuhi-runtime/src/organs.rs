//! Runtime promotion and adapter layer for experimentally validated organs.
//!
//! The core contract says what an organ is allowed to emit. This module fixes
//! which completed experiments are eligible for promotion and provides a
//! bounded process adapter so the existing Python experiment implementations
//! can be wrapped without teaching `kamimusuhi-core` about Python or PyTorch.
//!
//! A child process receives one [`OrganInput`] JSON document on stdin and must
//! return one small JSON object on stdout. It cannot choose its descriptor,
//! evidence references, promotion mode or authority: those are supplied by the
//! runtime. A process therefore cannot turn model output into canonical state
//! simply by printing fields with authoritative-sounding names.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use kamimusuhi_core::organs::{
    CognitiveOrgan, ExperimentVerdict, OrganDescriptor, OrganError, OrganEvidence, OrganInput,
    OrganKey, OrganRole, OrganSignal, PromotionMode,
};
use serde::Deserialize;

fn descriptor(
    key: &str,
    role: OrganRole,
    experiment: &str,
    verdict: ExperimentVerdict,
    promotion: PromotionMode,
    revision: &str,
    report_path: &str,
) -> OrganDescriptor {
    OrganDescriptor {
        key: OrganKey::new(key).expect("static organ key is valid"),
        role,
        implementation: "process-adapter".to_owned(),
        version: "1".to_owned(),
        promotion,
        evidence: OrganEvidence {
            experiment: experiment.to_owned(),
            verdict,
            source_revision: Some(revision.to_owned()),
            report_path: Some(report_path.to_owned()),
        },
    }
}

/// Promotion decisions backed by the completed experiment battery.
///
/// Active means "eligible to influence cognition once a conforming process and
/// pinned checkpoint are supplied". It does not mean a checkpoint is bundled
/// in the repository. O0 stays shadow-only because its comparative criterion
/// remained PARTIAL. G0 and P0 are intentionally absent: their negative
/// results are controls/design evidence, not deployable cognitive organs.
pub fn validated_experiment_manifest() -> Vec<OrganDescriptor> {
    vec![
        descriptor(
            "h0-regulation",
            OrganRole::Regulation,
            "H0",
            ExperimentVerdict::Pass,
            PromotionMode::Active,
            "c50c294fd843fa5613fdcacdc741dd2e123175f5",
            "experiments/h0/reports/H0_RESULTS.md",
        ),
        descriptor(
            "r0-memory-gate",
            OrganRole::MemoryGate,
            "R0",
            ExperimentVerdict::Pass,
            PromotionMode::Active,
            "4f95df640c5fb0dba42b1c8bd5f664094b976e47",
            "experiments/r0/reports/R0_RESULTS.md",
        ),
        descriptor(
            "s0-agency-attribution",
            OrganRole::AgencyAttribution,
            "S0",
            ExperimentVerdict::Pass,
            PromotionMode::Active,
            "f15aea6bbd8b1a53f020a5924e3ffcb153a96d37",
            "experiments/s0/reports/S0_RESULTS.md",
        ),
        descriptor(
            "t0-temporal-state",
            OrganRole::TemporalState,
            "T0",
            ExperimentVerdict::Pass,
            PromotionMode::Active,
            "72b5011bb629a2992e876e5fc9ec3fd8a430b04c",
            "experiments/t0/reports/T0_RESULTS.md",
        ),
        descriptor(
            "o0-object-state",
            OrganRole::ObjectState,
            "O0",
            ExperimentVerdict::Partial,
            PromotionMode::Shadow,
            "d18ee4316196f474ff9274d864b0d21ecfdf073c",
            "experiments/o0/reports/O0_RESULTS.md",
        ),
    ]
}

/// Non-secret configuration of one local process-backed organ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessOrganConfig {
    pub descriptor: OrganDescriptor,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub timeout_ms: u64,
    pub max_stdout_bytes: usize,
}

impl ProcessOrganConfig {
    pub fn new(descriptor: OrganDescriptor, program: impl Into<PathBuf>) -> Self {
        Self {
            descriptor,
            program: program.into(),
            args: Vec::new(),
            timeout_ms: 1_000,
            max_stdout_bytes: 64 * 1024,
        }
    }

    #[must_use]
    pub fn with_args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    #[must_use]
    pub const fn with_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    #[must_use]
    pub const fn with_max_stdout_bytes(mut self, max_stdout_bytes: usize) -> Self {
        self.max_stdout_bytes = max_stdout_bytes;
        self
    }

    pub fn validate(&self) -> Result<(), OrganError> {
        self.descriptor.validate()?;
        if self.descriptor.promotion == PromotionMode::Excluded {
            return Err(OrganError::Excluded {
                key: self.descriptor.key.clone(),
            });
        }
        if self.program.as_os_str().is_empty() {
            return Err(OrganError::InvalidDescriptor {
                reason: "organ process program is empty".to_owned(),
            });
        }
        if self.timeout_ms == 0 {
            return Err(OrganError::InvalidDescriptor {
                reason: "organ process timeout_ms is zero".to_owned(),
            });
        }
        if self.max_stdout_bytes == 0 {
            return Err(OrganError::InvalidDescriptor {
                reason: "organ process max_stdout_bytes is zero".to_owned(),
            });
        }
        Ok(())
    }
}

/// One-shot local-process adapter for a promoted experiment implementation.
#[derive(Debug, Clone)]
pub struct ProcessOrgan {
    config: ProcessOrganConfig,
}

impl ProcessOrgan {
    pub fn new(config: ProcessOrganConfig) -> Result<Self, OrganError> {
        config.validate()?;
        Ok(Self { config })
    }

    pub const fn config(&self) -> &ProcessOrganConfig {
        &self.config
    }
}

/// The only fields a child process is allowed to choose.
///
/// Evidence references are copied from the runtime-owned input, and the
/// descriptor is copied from configuration. This is deliberately narrower
/// than [`OrganSignal`].
#[derive(Debug, Deserialize)]
struct ProcessReply {
    payload: serde_json::Value,
    ttl_ms: u64,
    #[serde(default)]
    confidence_milli: Option<u16>,
}

impl CognitiveOrgan for ProcessOrgan {
    fn descriptor(&self) -> OrganDescriptor {
        self.config.descriptor.clone()
    }

    fn process(&mut self, input: &OrganInput) -> Result<OrganSignal, OrganError> {
        self.config.validate()?;
        let request = serde_json::to_vec(input).map_err(|_| OrganError::Backend {
            code: "request_serialize".to_owned(),
        })?;

        let mut child = Command::new(&self.config.program)
            .args(&self.config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Child diagnostics are intentionally not copied into runtime
            // errors. Operators may capture them separately if desired.
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| OrganError::Backend {
                code: "process_spawn".to_owned(),
            })?;

        let mut stdin = child.stdin.take().ok_or_else(|| OrganError::Backend {
            code: "process_stdin".to_owned(),
        })?;
        stdin
            .write_all(&request)
            .and_then(|()| stdin.write_all(b"\n"))
            .map_err(|_| OrganError::Backend {
                code: "process_stdin_write".to_owned(),
            })?;
        drop(stdin);

        let stdout = child.stdout.take().ok_or_else(|| OrganError::Backend {
            code: "process_stdout".to_owned(),
        })?;
        let max_stdout_bytes = self.config.max_stdout_bytes;
        let reader = thread::spawn(move || {
            let mut bytes = Vec::new();
            let limit = u64::try_from(max_stdout_bytes)
                .unwrap_or(u64::MAX)
                .saturating_add(1);
            let mut bounded = stdout.take(limit);
            bounded.read_to_end(&mut bytes).map(|_| bytes)
        });

        let started = Instant::now();
        let timeout = Duration::from_millis(self.config.timeout_ms);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() >= timeout => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = reader.join();
                    return Err(OrganError::Backend {
                        code: "process_timeout".to_owned(),
                    });
                }
                Ok(None) => thread::sleep(Duration::from_millis(5)),
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = reader.join();
                    return Err(OrganError::Backend {
                        code: "process_wait".to_owned(),
                    });
                }
            }
        };

        let bytes = reader
            .join()
            .map_err(|_| OrganError::Backend {
                code: "process_reader_panic".to_owned(),
            })?
            .map_err(|_| OrganError::Backend {
                code: "process_stdout_read".to_owned(),
            })?;
        if !status.success() {
            return Err(OrganError::Backend {
                code: "process_exit".to_owned(),
            });
        }
        if bytes.len() > self.config.max_stdout_bytes {
            return Err(OrganError::InvalidOutput {
                reason: "organ process stdout exceeds configured bound".to_owned(),
            });
        }

        let reply: ProcessReply =
            serde_json::from_slice(&bytes).map_err(|_| OrganError::InvalidOutput {
                reason: "organ process stdout is not a valid reply".to_owned(),
            })?;

        let signal = OrganSignal {
            descriptor: self.config.descriptor.clone(),
            // The runtime owns the time domain. Until the runtime scheduler
            // supplies a separate completion clock, use the attributed input
            // observation time rather than trusting a child-supplied time.
            produced_at: input.observed_at,
            ttl_ms: reply.ttl_ms,
            confidence_milli: reply.confidence_milli,
            payload: reply.payload,
            evidence_refs: input.evidence_refs.clone(),
        };
        signal.validate()?;
        Ok(signal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pass_results_are_active_and_o0_is_shadow() {
        let manifest = validated_experiment_manifest();
        assert_eq!(manifest.len(), 5);
        for descriptor in &manifest {
            descriptor.validate().unwrap();
        }
        assert_eq!(
            manifest
                .iter()
                .filter(|d| d.promotion == PromotionMode::Active)
                .count(),
            4
        );
        let o0 = manifest
            .iter()
            .find(|d| d.evidence.experiment == "O0")
            .unwrap();
        assert_eq!(o0.promotion, PromotionMode::Shadow);
        assert_eq!(o0.evidence.verdict, ExperimentVerdict::Partial);
    }

    #[test]
    fn negative_results_are_not_promoted() {
        let manifest = validated_experiment_manifest();
        let experiments: Vec<&str> = manifest
            .iter()
            .map(|d| d.evidence.experiment.as_str())
            .collect();
        assert!(!experiments.contains(&"G0"));
        assert!(!experiments.contains(&"P0"));
    }

    #[test]
    fn process_adapter_rejects_zero_bounds() {
        let descriptor = validated_experiment_manifest().remove(0);
        let error = ProcessOrganConfig::new(descriptor, "python3")
            .with_timeout_ms(0)
            .validate()
            .unwrap_err();
        assert_eq!(error.code(), "INVALID_DESCRIPTOR");
    }
}
