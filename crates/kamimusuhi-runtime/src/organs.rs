//! Runtime promotion and adapter layer for experimentally validated organs.
//!
//! The core contract says what an organ is allowed to emit. This module fixes
//! which completed experiments are eligible for promotion and provides a
//! bounded process adapter so the existing Python experiment implementations
//! can be wrapped without teaching `kamimusuhi-core` about Python or PyTorch.
//!
//! A child process receives one JSON document on stdin and must return one
//! small JSON object on stdout. It cannot choose its descriptor, evidence
//! references, promotion mode or authority: those are supplied by the runtime.
//! A process therefore cannot turn model output into canonical state simply by
//! printing fields with authoritative-sounding names.
//!
//! Stateful experimental policies are supported without making their hidden
//! state canonical. The process reply may carry an opaque JSON `state`, which
//! the adapter holds only in memory and sends back on the next invocation. A
//! runtime restart loses that hidden state unless a higher-level organ-specific
//! recovery mechanism deliberately reconstructs it from canonical evidence.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use kamimusuhi_core::organs::{
    CognitiveOrgan, ExperimentVerdict, OrganCycle, OrganDescriptor, OrganError, OrganEvidence,
    OrganInput, OrganKey, OrganRole, OrganSignal, OrganSupervisor, PromotionMode,
};
use kamimusuhi_core::persona::PersonaEnvelope;
use serde::{Deserialize, Serialize};

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
    pub max_request_bytes: usize,
    pub max_stdout_bytes: usize,
}

impl ProcessOrganConfig {
    pub fn new(descriptor: OrganDescriptor, program: impl Into<PathBuf>) -> Self {
        Self {
            descriptor,
            program: program.into(),
            args: Vec::new(),
            timeout_ms: 1_000,
            max_request_bytes: 64 * 1024,
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
    pub const fn with_max_request_bytes(mut self, max_request_bytes: usize) -> Self {
        self.max_request_bytes = max_request_bytes;
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
        if self.max_request_bytes == 0 {
            return Err(OrganError::InvalidDescriptor {
                reason: "organ process max_request_bytes is zero".to_owned(),
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

/// Local-process adapter for a promoted experiment implementation.
#[derive(Debug, Clone)]
pub struct ProcessOrgan {
    config: ProcessOrganConfig,
    /// Opaque recurrent/tracker state. Operational only, never canonical.
    state: Option<serde_json::Value>,
}

impl ProcessOrgan {
    pub fn new(config: ProcessOrganConfig) -> Result<Self, OrganError> {
        config.validate()?;
        Ok(Self {
            config,
            state: None,
        })
    }

    pub const fn config(&self) -> &ProcessOrganConfig {
        &self.config
    }

    pub fn state(&self) -> Option<&serde_json::Value> {
        self.state.as_ref()
    }

    /// Drop transient recurrent state without changing descriptor or evidence.
    pub fn reset_state(&mut self) {
        self.state = None;
    }
}

/// What the runtime sends to a process-backed organ.
#[derive(Debug, Serialize)]
struct ProcessRequest<'a> {
    input: &'a OrganInput,
    /// Previous opaque state, if the organ returned one. This state is not a
    /// memory record and has no continuity authority.
    state: Option<&'a serde_json::Value>,
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
    /// Opaque next recurrent/tracker state. It is retained only after the
    /// signal itself passes validation.
    #[serde(default)]
    state: Option<serde_json::Value>,
}

#[cfg(unix)]
fn configure_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_group(_command: &mut Command) {}

fn process_timeout_error() -> OrganError {
    OrganError::Backend {
        code: "process_timeout".to_owned(),
    }
}

fn terminate_process_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        let _ = Command::new("/bin/kill")
            .arg("-KILL")
            .arg(format!("-{}", child.id()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

impl CognitiveOrgan for ProcessOrgan {
    fn descriptor(&self) -> OrganDescriptor {
        self.config.descriptor.clone()
    }

    fn process(&mut self, input: &OrganInput) -> Result<OrganSignal, OrganError> {
        self.config.validate()?;
        let started = Instant::now();
        let timeout = Duration::from_millis(self.config.timeout_ms);
        let request = serde_json::to_vec(&ProcessRequest {
            input,
            state: self.state.as_ref(),
        })
        .map_err(|_| OrganError::Backend {
            code: "request_serialize".to_owned(),
        })?;
        if request.len().saturating_add(1) > self.config.max_request_bytes {
            return Err(OrganError::InvalidInput {
                reason: "organ process request exceeds configured bound".to_owned(),
            });
        }
        if started.elapsed() >= timeout {
            return Err(process_timeout_error());
        }

        let mut command = Command::new(&self.config.program);
        command
            .args(&self.config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        configure_process_group(&mut command);
        let mut child = command.spawn().map_err(|_| OrganError::Backend {
            code: "process_spawn".to_owned(),
        })?;

        let mut stdin = child.stdin.take().ok_or_else(|| OrganError::Backend {
            code: "process_stdin".to_owned(),
        })?;
        let writer = thread::spawn(move || {
            stdin
                .write_all(&request)
                .and_then(|()| stdin.write_all(b"\n"))
        });

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

        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if started.elapsed() >= timeout => {
                    terminate_process_tree(&mut child);
                    return Err(process_timeout_error());
                }
                Ok(None) => thread::sleep(Duration::from_millis(5)),
                Err(_) => {
                    terminate_process_tree(&mut child);
                    return Err(OrganError::Backend {
                        code: "process_wait".to_owned(),
                    });
                }
            }
        };

        while !writer.is_finished() || !reader.is_finished() {
            if started.elapsed() >= timeout {
                terminate_process_tree(&mut child);
                return Err(process_timeout_error());
            }
            thread::sleep(Duration::from_millis(5));
        }

        let write_result = writer.join().map_err(|_| OrganError::Backend {
            code: "process_writer_panic".to_owned(),
        })?;
        let read_result = reader.join().map_err(|_| OrganError::Backend {
            code: "process_reader_panic".to_owned(),
        })?;
        if !status.success() {
            return Err(OrganError::Backend {
                code: "process_exit".to_owned(),
            });
        }
        write_result.map_err(|_| OrganError::Backend {
            code: "process_stdin_write".to_owned(),
        })?;
        let bytes = read_result.map_err(|_| OrganError::Backend {
            code: "process_stdout_read".to_owned(),
        })?;
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
            produced_at: input.observed_at,
            ttl_ms: reply.ttl_ms,
            confidence_milli: reply.confidence_milli,
            payload: reply.payload,
            evidence_refs: input.evidence_refs.clone(),
        };
        signal.validate()?;
        self.state = reply.state;
        Ok(signal)
    }
}

/// Run one organ cycle and attach only admitted active signals to the Persona
/// envelope. Shadow outputs and failures remain available in the returned
/// cycle for trace/evaluation, but they cannot influence the Persona turn.
pub fn run_organs_for_persona(
    supervisor: &mut OrganSupervisor,
    input: &OrganInput,
    envelope: PersonaEnvelope,
) -> (PersonaEnvelope, OrganCycle) {
    let cycle = supervisor.run(input);
    let envelope = envelope.with_active_organ_signals(cycle.active.clone());
    (envelope, cycle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kamimusuhi_core::time::UtcTimestamp;

    struct FixtureOrgan {
        descriptor: OrganDescriptor,
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
                payload: serde_json::json!({"key": self.descriptor.key.as_str()}),
                evidence_refs: input.evidence_refs.clone(),
            })
        }
    }

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

    #[test]
    fn process_adapter_rejects_oversized_request_before_spawn() {
        let descriptor = validated_experiment_manifest().remove(0);
        let mut organ = ProcessOrgan::new(
            ProcessOrganConfig::new(descriptor, "definitely-not-a-program")
                .with_max_request_bytes(64),
        )
        .unwrap();
        let error = organ
            .process(&OrganInput {
                observed_at: UtcTimestamp::from_unix_millis(1),
                payload: serde_json::json!({"blob": "x".repeat(1024)}),
                evidence_refs: Vec::new(),
            })
            .unwrap_err();
        assert_eq!(error.code(), "INVALID_INPUT");
    }

    #[cfg(unix)]
    #[test]
    fn process_timeout_covers_a_child_that_never_reads_stdin() {
        let descriptor = validated_experiment_manifest().remove(0);
        let mut organ = ProcessOrgan::new(
            ProcessOrganConfig::new(descriptor, "sh")
                .with_args(vec!["-c".to_owned(), "sleep 5".to_owned()])
                .with_timeout_ms(100)
                .with_max_request_bytes(2 * 1024 * 1024),
        )
        .unwrap();
        let error = organ
            .process(&OrganInput {
                observed_at: UtcTimestamp::from_unix_millis(1),
                payload: serde_json::json!({"blob": "x".repeat(1024 * 1024)}),
                evidence_refs: Vec::new(),
            })
            .unwrap_err();
        assert!(error.to_string().contains("process_timeout"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn process_timeout_does_not_block_on_descendant_inherited_stdout() {
        let descriptor = validated_experiment_manifest().remove(0);
        let script = r#"(sleep 5) & printf '%s\n' '{\"payload\":{},\"ttl_ms\":100}'"#;
        let mut organ = ProcessOrgan::new(
            ProcessOrganConfig::new(descriptor, "sh")
                .with_args(vec!["-c".to_owned(), script.to_owned()])
                .with_timeout_ms(100),
        )
        .unwrap();
        let error = organ
            .process(&OrganInput {
                observed_at: UtcTimestamp::from_unix_millis(1),
                payload: serde_json::json!({}),
                evidence_refs: Vec::new(),
            })
            .unwrap_err();
        assert!(error.to_string().contains("process_timeout"), "{error}");
    }

    #[test]
    fn persona_integration_keeps_shadow_outputs_out_of_context() {
        let manifest = validated_experiment_manifest();
        let active = manifest
            .iter()
            .find(|d| d.promotion == PromotionMode::Active)
            .unwrap()
            .clone();
        let shadow = manifest
            .iter()
            .find(|d| d.promotion == PromotionMode::Shadow)
            .unwrap()
            .clone();
        let mut supervisor = OrganSupervisor::new();
        supervisor
            .register(Box::new(FixtureOrgan { descriptor: shadow }))
            .unwrap();
        supervisor
            .register(Box::new(FixtureOrgan { descriptor: active }))
            .unwrap();

        let input = OrganInput {
            observed_at: UtcTimestamp::from_unix_millis(1),
            payload: serde_json::json!({"observation": [0.0, 1.0]}),
            evidence_refs: Vec::new(),
        };
        let (envelope, cycle) =
            run_organs_for_persona(&mut supervisor, &input, PersonaEnvelope::default());
        assert_eq!(cycle.active.len(), 1);
        assert_eq!(cycle.shadow.len(), 1);
        assert_eq!(envelope.organ_signals.len(), 1);
        assert_eq!(envelope.organ_signals[0], cycle.active[0]);
    }
}
