//! Model-independent K-CORE v0: a bounded, non-authoritative cognitive loop.
//!
//! Identity, memory and lineage belong to `Runtime` / the Continuity Kernel.
//! This module owns only process-lifetime observations and organ state. It
//! never claims a canonical writer, starts a model, or commits organ output.

use std::collections::{BTreeMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kamimusuhi_core::ids::{BootId, IndividualId};
use kamimusuhi_core::organs::{
    CognitiveOrgan, OrganCycle, OrganDescriptor, OrganInput, OrganSupervisor, PromotionMode,
};
use serde::{Deserialize, Serialize};

use crate::organs::{ProcessOrgan, ProcessOrganConfig, validated_experiment_manifest};
use crate::{InspectReport, Runtime, RuntimeError, RuntimeOptions, inspect};

pub const MAX_FRAME_BYTES: usize = 16 * 1024;
pub const MAX_CONFIG_BYTES: usize = 64 * 1024;
pub const MAX_OBSERVATION_DIM: usize = 256;
const MAX_ORGANS: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum KCoreError {
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("invalid K-CORE configuration: {0}")]
    Config(String),
    #[error("K-CORE loop lock unavailable: {0}")]
    Lock(String),
    #[error("organ registration failed: {0}")]
    Organ(String),
    #[error("tick counter exhausted")]
    CounterExhausted,
}

/// Local operator configuration, not canonical state or a model checkpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KCoreConfig {
    pub interval_ms: u64,
    pub queue_capacity: usize,
    pub max_sources: usize,
    pub max_age_ms: u64,
    pub organs: Vec<ProcessBinding>,
}

impl Default for KCoreConfig {
    fn default() -> Self {
        Self {
            interval_ms: 100,
            queue_capacity: 64,
            max_sources: 64,
            max_age_ms: 5_000,
            organs: Vec::new(),
        }
    }
}

impl KCoreConfig {
    pub fn validate(&self) -> Result<(), KCoreError> {
        if !(1..=60_000).contains(&self.interval_ms)
            || !(1..=4_096).contains(&self.queue_capacity)
            || !(1..=1_024).contains(&self.max_sources)
            || !(1..=60_000).contains(&self.max_age_ms)
            || self.organs.len() > MAX_ORGANS
        {
            return Err(KCoreError::Config("limits out of range".to_owned()));
        }
        let mut keys = std::collections::BTreeSet::new();
        for binding in &self.organs {
            binding.descriptor()?;
            if !keys.insert(&binding.key) {
                return Err(KCoreError::Config("duplicate organ key".to_owned()));
            }
        }
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self, KCoreError> {
        let mut bytes = Vec::new();
        File::open(path)?
            .take((MAX_CONFIG_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_CONFIG_BYTES {
            return Err(KCoreError::Config("config too large".to_owned()));
        }
        let config: Self = serde_json::from_slice(&bytes)?;
        config.validate()?;
        Ok(config)
    }
}

/// A checkpoint-specific wrapper must implement the existing ProcessOrgan
/// wire contract. Nothing is downloaded, trained or promoted automatically.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessBinding {
    pub key: String,
    pub program: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "shadow")]
    pub promotion: PromotionMode,
    #[serde(default = "process_timeout")]
    pub timeout_ms: u64,
}

fn shadow() -> PromotionMode {
    PromotionMode::Shadow
}

fn process_timeout() -> u64 {
    1_000
}

impl ProcessBinding {
    fn descriptor(&self) -> Result<OrganDescriptor, KCoreError> {
        let mut descriptor = validated_experiment_manifest()
            .into_iter()
            .find(|entry| entry.key.as_str() == self.key)
            .ok_or_else(|| KCoreError::Config(format!("unpromoted organ: {}", self.key)))?;
        if self.promotion == PromotionMode::Excluded
            || (self.promotion == PromotionMode::Active
                && descriptor.promotion != PromotionMode::Active)
            || self.program.as_os_str().is_empty()
            || !(1..=10_000).contains(&self.timeout_ms)
        {
            return Err(KCoreError::Config(format!("invalid binding: {}", self.key)));
        }
        descriptor.promotion = self.promotion;
        Ok(descriptor)
    }

    fn build(&self) -> Result<ProcessOrgan, KCoreError> {
        ProcessOrgan::new(
            ProcessOrganConfig::new(self.descriptor()?, &self.program)
                .with_args(self.args.clone())
                .with_timeout_ms(self.timeout_ms),
        )
        .map_err(|error| KCoreError::Organ(error.to_string()))
    }
}

/// Numeric ingress only. `source` and sequence are local routing metadata,
/// NOT authentication or evidence. No caller-supplied evidence IDs are accepted.
/// `age_ms` is the producer's age at ingress; local queue and execution time
/// are added using Instant, never a remote process's monotonic timestamp.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerceptFrame {
    pub source: String,
    pub sequence: u64,
    pub observation: Vec<f64>,
    pub quality_milli: u16,
    #[serde(default)]
    pub age_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngressError {
    Invalid,
    Stale,
    OutOfOrder,
    QueueFull,
    TooManySources,
}

#[derive(Debug)]
struct PendingFrame {
    frame: PerceptFrame,
    deadline: Instant,
}

/// Deliberately conservative v0 policy, NOT a learned K0/CX policy and NOT an
/// actuator or LLM call. Only active, fresh signals may produce Observe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreIntent {
    Wait,
    Observe,
}

#[derive(Debug, Clone, Serialize)]
pub struct TickReport {
    pub individual_id: IndividualId,
    pub boot_id: BootId,
    pub tick: u64,
    pub intent: CoreIntent,
    pub source: Option<String>,
    pub sequence: Option<u64>,
    pub cycle: OrganCycle,
    pub discarded_stale: usize,
    pub queued: usize,
    pub authorizes_mutation: bool,
}

/// Owns the same persistent individual regardless of attached organs. The
/// lock serializes K-CORE instances in this runtime directory; it is NOT a
/// distributed canonical writer lease. Never unlink the lock file while live.
#[derive(Debug)]
pub struct KCore {
    runtime: Runtime,
    config: KCoreConfig,
    supervisor: OrganSupervisor,
    queue: VecDeque<PendingFrame>,
    last_sequence: BTreeMap<String, u64>,
    ticks: u64,
    // OS lock released on drop/process exit, including abnormal termination.
    _loop_lock: File,
}

impl KCore {
    /// Resume only: initialization remains an explicit existing Runtime command.
    pub fn open(
        dir: impl AsRef<Path>,
        options: RuntimeOptions,
        config: KCoreConfig,
    ) -> Result<Self, KCoreError> {
        config.validate()?;
        let runtime = Runtime::open(dir, options)?;
        let loop_lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(runtime.paths().dir.join("kcore.loop.lock"))?;
        loop_lock
            .try_lock()
            .map_err(|error| KCoreError::Lock(error.to_string()))?;
        let mut supervisor = OrganSupervisor::new();
        for binding in &config.organs {
            supervisor
                .register(Box::new(binding.build()?))
                .map_err(|error| KCoreError::Organ(error.to_string()))?;
        }
        Ok(Self {
            runtime,
            config,
            supervisor,
            queue: VecDeque::new(),
            last_sequence: BTreeMap::new(),
            ticks: 0,
            _loop_lock: loop_lock,
        })
    }

    /// Native implementations are trusted host code. Use bounded ProcessOrgan
    /// for untrusted/fallible external backends, not arbitrary in-process code.
    pub fn register(&mut self, organ: Box<dyn CognitiveOrgan>) -> Result<(), KCoreError> {
        if self.supervisor.descriptors().len() >= MAX_ORGANS {
            return Err(KCoreError::Config("too many organs".to_owned()));
        }
        self.supervisor
            .register(organ)
            .map_err(|error| KCoreError::Organ(error.to_string()))
    }

    pub fn inspect(&self) -> Result<InspectReport, KCoreError> {
        Ok(inspect(&self.runtime)?)
    }

    pub fn descriptors(&self) -> Vec<OrganDescriptor> {
        self.supervisor.descriptors()
    }

    pub fn interval(&self) -> Duration {
        Duration::from_millis(self.config.interval_ms)
    }

    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    pub fn ingest(&mut self, frame: PerceptFrame) -> Result<(), IngressError> {
        self.ingest_at(frame, Instant::now())
    }

    fn ingest_at(&mut self, frame: PerceptFrame, now: Instant) -> Result<(), IngressError> {
        if frame.source.is_empty()
            || frame.source.len() > 96
            || !frame
                .source
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            || frame.observation.is_empty()
            || frame.observation.len() > MAX_OBSERVATION_DIM
            || frame.observation.iter().any(|value| !value.is_finite())
            || !(1..=1_000).contains(&frame.quality_milli)
        {
            return Err(IngressError::Invalid);
        }
        if frame.age_ms >= self.config.max_age_ms {
            return Err(IngressError::Stale);
        }
        if self
            .last_sequence
            .get(&frame.source)
            .is_some_and(|previous| frame.sequence <= *previous)
        {
            return Err(IngressError::OutOfOrder);
        }
        if !self.last_sequence.contains_key(&frame.source)
            && self.last_sequence.len() >= self.config.max_sources
        {
            return Err(IngressError::TooManySources);
        }
        if self.queue.len() >= self.config.queue_capacity {
            return Err(IngressError::QueueFull);
        }
        let deadline = now
            .checked_add(Duration::from_millis(self.config.max_age_ms - frame.age_ms))
            .ok_or(IngressError::Invalid)?;
        // Rejection never consumes a sequence number; backpressure is retryable.
        self.last_sequence
            .insert(frame.source.clone(), frame.sequence);
        self.queue.push_back(PendingFrame { frame, deadline });
        Ok(())
    }

    /// At most one fresh observation per tick. Idle ticks never call a backend.
    /// No stale/cached organ signal is fed into the next tick or restored.
    pub fn tick(&mut self) -> Result<TickReport, KCoreError> {
        // Fail closed if the canonical individual can no longer be restored.
        self.runtime.head()?;
        self.ticks = self
            .ticks
            .checked_add(1)
            .ok_or(KCoreError::CounterExhausted)?;
        let mut report = TickReport {
            individual_id: self.runtime.individual_id(),
            boot_id: self.runtime.boot_id(),
            tick: self.ticks,
            intent: CoreIntent::Wait,
            source: None,
            sequence: None,
            cycle: OrganCycle::default(),
            discarded_stale: 0,
            queued: 0,
            authorizes_mutation: false,
        };
        while let Some(pending) = self.queue.pop_front() {
            if Instant::now() >= pending.deadline {
                report.discarded_stale += 1;
                continue;
            }
            report.source = Some(pending.frame.source.clone());
            report.sequence = Some(pending.frame.sequence);
            let input = OrganInput {
                observed_at: self.runtime.now(),
                payload: serde_json::to_value(&pending.frame)?,
                evidence_refs: Vec::new(),
            };
            report.cycle = self.supervisor.run(&input);
            // Even a valid signal is useless if its originating observation
            // expired while a slow organ was being executed.
            if Instant::now() >= pending.deadline {
                report.cycle.active.clear();
                report.discarded_stale += 1;
            } else {
                let active_keys: std::collections::BTreeSet<_> = self
                    .supervisor
                    .descriptors()
                    .into_iter()
                    .filter(|descriptor| descriptor.promotion == PromotionMode::Active)
                    .map(|descriptor| descriptor.key)
                    .collect();
                let active_failed = report
                    .cycle
                    .failures
                    .iter()
                    .any(|failure| active_keys.contains(&failure.key));
                if !report.cycle.active.is_empty() && !active_failed {
                    report.intent = CoreIntent::Observe;
                }
            }
            break;
        }
        report.queued = self.queue.len();
        Ok(report)
    }
}

impl Drop for KCore {
    fn drop(&mut self) {
        self.runtime.stopping();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ResourceImplementation;

    fn fixture(config: KCoreConfig) -> (tempfile::TempDir, KCore) {
        let dir = tempfile::tempdir().unwrap();
        Runtime::init(
            dir.path(),
            RuntimeOptions::deterministic(1),
            ResourceImplementation::FakeUnavailable,
        )
        .unwrap();
        let core = KCore::open(dir.path(), RuntimeOptions::deterministic(2), config).unwrap();
        (dir, core)
    }

    fn frame(source: &str, sequence: u64) -> PerceptFrame {
        PerceptFrame {
            source: source.to_owned(),
            sequence,
            observation: vec![0.1, -0.2],
            quality_milli: 1_000,
            age_ms: 0,
        }
    }

    #[test]
    fn model_free_idle_and_restart_preserve_canonical_state() {
        let (dir, mut core) = fixture(KCoreConfig::default());
        let before = core.inspect().unwrap();
        let first = core.tick().unwrap();
        assert_eq!(first.intent, CoreIntent::Wait);
        assert!(!first.authorizes_mutation);
        assert!(core.descriptors().is_empty());
        for _ in 0..32 {
            core.tick().unwrap();
        }
        assert_eq!(before, core.inspect().unwrap());
        drop(core);
        let mut resumed = KCore::open(
            dir.path(),
            RuntimeOptions::deterministic(3),
            KCoreConfig::default(),
        )
        .unwrap();
        assert_eq!(before, resumed.inspect().unwrap());
        let next = resumed.tick().unwrap();
        assert_eq!(first.individual_id, next.individual_id);
        assert_ne!(first.boot_id, next.boot_id);
        assert_eq!(next.tick, 1);
        assert_eq!(resumed.queued(), 0);
    }

    #[test]
    fn duplicate_loops_are_fenced_and_lock_releases_on_drop() {
        let (dir, core) = fixture(KCoreConfig::default());
        assert!(matches!(
            KCore::open(
                dir.path(),
                RuntimeOptions::default(),
                KCoreConfig::default()
            ),
            Err(KCoreError::Lock(_))
        ));
        drop(core);
        assert!(
            KCore::open(
                dir.path(),
                RuntimeOptions::default(),
                KCoreConfig::default()
            )
            .is_ok()
        );
    }

    #[test]
    fn missing_runtime_never_initializes_an_individual() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            KCore::open(
                dir.path(),
                RuntimeOptions::default(),
                KCoreConfig::default()
            )
            .is_err()
        );
        assert!(!dir.path().join("kamimusuhi.sqlite").exists());
    }

    #[test]
    fn bounded_queue_does_not_consume_rejected_sequence() {
        let config = KCoreConfig {
            queue_capacity: 1,
            ..KCoreConfig::default()
        };
        let (_dir, mut core) = fixture(config);
        core.ingest(frame("sensor", 1)).unwrap();
        assert_eq!(
            core.ingest(frame("sensor", 2)),
            Err(IngressError::QueueFull)
        );
        core.tick().unwrap();
        core.ingest(frame("sensor", 2)).unwrap();
        assert_eq!(
            core.ingest(frame("sensor", 2)),
            Err(IngressError::OutOfOrder)
        );
        assert_eq!(
            core.ingest(frame("sensor", 1)),
            Err(IngressError::OutOfOrder)
        );
    }

    #[test]
    fn source_registry_is_bounded() {
        let config = KCoreConfig {
            max_sources: 1,
            ..KCoreConfig::default()
        };
        let (_dir, mut core) = fixture(config);
        core.ingest(frame("a", 1)).unwrap();
        assert_eq!(
            core.ingest(frame("b", 1)),
            Err(IngressError::TooManySources)
        );
    }

    #[test]
    fn rejects_nan_bad_quality_and_untrusted_evidence_fields() {
        let (_dir, mut core) = fixture(KCoreConfig::default());
        let mut invalid = frame("sensor", 1);
        invalid.observation[0] = f64::NAN;
        assert_eq!(core.ingest(invalid), Err(IngressError::Invalid));
        let mut invalid = frame("sensor", 1);
        invalid.quality_milli = 0;
        assert_eq!(core.ingest(invalid), Err(IngressError::Invalid));
        let mut value = serde_json::to_value(frame("sensor", 1)).unwrap();
        value["evidence_refs"] = serde_json::json!(["forged"]);
        assert!(serde_json::from_value::<PerceptFrame>(value).is_err());
        core.ingest(frame("sensor", 1)).unwrap();
    }

    #[test]
    fn stale_ingress_and_queued_expiry_fail_closed() {
        let (_dir, mut core) = fixture(KCoreConfig::default());
        let mut old = frame("sensor", 1);
        old.age_ms = 5_000;
        assert_eq!(core.ingest(old), Err(IngressError::Stale));
        core.ingest_at(frame("sensor", 1), Instant::now() - Duration::from_secs(6))
            .unwrap();
        let report = core.tick().unwrap();
        assert_eq!(report.discarded_stale, 1);
        assert_eq!(report.intent, CoreIntent::Wait);
        assert!(report.source.is_none());
    }

    #[test]
    fn unknown_and_failed_experiments_cannot_be_promoted() {
        for key in ["g0", "p0", "unknown", "o0-object-state"] {
            let config = KCoreConfig {
                organs: vec![ProcessBinding {
                    key: key.to_owned(),
                    program: "python3".into(),
                    args: Vec::new(),
                    promotion: PromotionMode::Active,
                    timeout_ms: 100,
                }],
                ..KCoreConfig::default()
            };
            assert!(config.validate().is_err());
        }
    }

    #[test]
    fn process_binding_defaults_to_shadow() {
        let binding: ProcessBinding =
            serde_json::from_str(r#"{"key":"h0-regulation","program":"python3"}"#).unwrap();
        assert_eq!(binding.promotion, PromotionMode::Shadow);
        assert!(binding.descriptor().is_ok());
    }
}
