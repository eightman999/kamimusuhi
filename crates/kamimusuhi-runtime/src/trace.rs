//! Append-only JSONL trace sink.
//!
//! One event per line, opened in append mode and flushed per line, so a trace
//! survives a process that ends abruptly and two processes writing the same
//! file interleave whole lines rather than corrupting each other's.
//!
//! This file is observability. It is not canonical state, it is never read
//! back as evidence or authority, and a failure to write it is reported as a
//! lost observation — never as a failed or successful canonical transition
//! (see [`kamimusuhi_core::trace`]).
//!
//! **Rotation.** Once the runtime makes network calls it emits events for as
//! long as it runs, so an unbounded file is a disk-full waiting to happen. At
//! [`JsonlTraceSink::DEFAULT_MAX_BYTES`] the current file is renamed to
//! `trace.jsonl.1` and a fresh one started, keeping one previous generation.
//! Rotation happens between whole lines, never inside one. This is a size cap,
//! not a log subsystem: it is deliberately the smallest thing that stops the
//! file growing forever, and it is exactly why nothing may treat the trace as
//! a durable record — anything that must survive belongs in the database.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use kamimusuhi_core::ids::{IdGenerator, TraceId};
use kamimusuhi_core::time::Clock;
use kamimusuhi_core::trace::{TraceCorrelation, TraceError, TraceEvent, TraceEventKind, TraceSink};

/// JSONL trace file.
pub struct JsonlTraceSink {
    path: PathBuf,
    file: Mutex<File>,
    /// Correlates every event from this one runtime invocation.
    trace_id: TraceId,
    sequence: AtomicU64,
    max_bytes: u64,
}

impl std::fmt::Debug for JsonlTraceSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonlTraceSink")
            .field("path", &self.path)
            .field("trace_id", &self.trace_id)
            .finish_non_exhaustive()
    }
}

impl JsonlTraceSink {
    pub const FILE_NAME: &'static str = "trace.jsonl";
    /// Rotate at 8 MiB. Big enough that a normal session is one file, small
    /// enough that a long-running runtime cannot fill a disk unnoticed.
    pub const DEFAULT_MAX_BYTES: u64 = 8 * 1024 * 1024;

    /// Open (creating if needed) the trace file in append mode.
    pub fn open(path: impl AsRef<Path>, ids: &dyn IdGenerator) -> Result<Self, TraceError> {
        Self::open_with_limit(path, ids, Self::DEFAULT_MAX_BYTES)
    }

    pub fn open_with_limit(
        path: impl AsRef<Path>,
        ids: &dyn IdGenerator,
        max_bytes: u64,
    ) -> Result<Self, TraceError> {
        let path = path.as_ref().to_path_buf();
        let file = open_append(&path)?;
        Ok(Self {
            path,
            file: Mutex::new(file),
            trace_id: TraceId::generate(ids),
            sequence: AtomicU64::new(0),
            max_bytes,
        })
    }

    /// Path of the one retained previous generation.
    pub fn rotated_path(&self) -> PathBuf {
        rotated_path(&self.path)
    }

    pub const fn trace_id(&self) -> TraceId {
        self.trace_id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Next sequence number for this invocation. Ordering inside a trace does
    /// not rely on timestamps, which repeat under a fixed clock.
    pub fn next_sequence(&self) -> u64 {
        self.sequence.fetch_add(1, Ordering::SeqCst)
    }
}

impl TraceSink for JsonlTraceSink {
    fn emit(&self, event: &TraceEvent) -> Result<(), TraceError> {
        let mut line = serde_json::to_string(event).map_err(|source| TraceError::Encode {
            message: source.to_string(),
        })?;
        line.push('\n');
        let mut file = self.file.lock().map_err(|_| TraceError::Backend {
            message: "trace file mutex poisoned by an earlier panic".to_owned(),
        })?;
        // Rotate between lines, never inside one.
        if self.max_bytes > 0
            && file
                .metadata()
                .map(|meta| meta.len() >= self.max_bytes)
                .unwrap_or(false)
        {
            // A rename the OS refuses is not worth failing the call over; the
            // next event will try again and the file keeps its properties.
            if std::fs::rename(&self.path, rotated_path(&self.path)).is_ok() {
                *file = open_append(&self.path)?;
            }
        }
        // One write for the whole line: a partial line would be a corrupt
        // record, and two appending processes must not interleave mid-event.
        file.write_all(line.as_bytes())
            .map_err(|source| TraceError::Backend {
                message: format!("write {}: {source}", self.path.display()),
            })?;
        file.flush().map_err(|source| TraceError::Backend {
            message: format!("flush {}: {source}", self.path.display()),
        })
    }
}

/// Emits events with the ambient correlation of one runtime invocation
/// already filled in.
///
/// A trace failure is written to stderr and otherwise ignored: the caller is
/// in the middle of real work, and losing an observation must not change what
/// the runtime does or claims.
pub struct TraceRecorder {
    sink: JsonlTraceSink,
    clock: std::sync::Arc<dyn Clock>,
    base: TraceCorrelation,
}

impl std::fmt::Debug for TraceRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TraceRecorder")
            .field("sink", &self.sink)
            .finish_non_exhaustive()
    }
}

impl TraceRecorder {
    pub fn new(
        sink: JsonlTraceSink,
        clock: std::sync::Arc<dyn Clock>,
        base: TraceCorrelation,
    ) -> Self {
        Self { sink, clock, base }
    }

    pub const fn trace_id(&self) -> TraceId {
        self.sink.trace_id()
    }

    pub fn path(&self) -> &Path {
        self.sink.path()
    }

    /// Replace the ambient correlation, e.g. once a session and turn exist.
    pub fn set_base(&mut self, base: TraceCorrelation) {
        self.base = base;
    }

    pub fn base(&self) -> TraceCorrelation {
        self.base.clone()
    }

    pub fn record(&self, kind: TraceEventKind, correlation: TraceCorrelation) {
        self.record_with(kind, correlation, serde_json::Value::Null);
    }

    /// `detail` must carry only counts, digests, kinds and status codes.
    /// Raw utterances and candidate payloads stay in the stores that own them.
    pub fn record_with(
        &self,
        kind: TraceEventKind,
        correlation: TraceCorrelation,
        detail: serde_json::Value,
    ) {
        let event = TraceEvent::new(
            self.sink.trace_id(),
            self.sink.next_sequence(),
            kind,
            self.clock.now_utc(),
        )
        .with_correlation(merge(&self.base, correlation))
        .with_detail(detail);
        if let Err(error) = self.sink.emit(&event) {
            // Losing the observation is bad; pretending the runtime failed,
            // or that a canonical record exists, would be worse.
            eprintln!("kamimusuhi-runtime: trace event {kind} was not recorded: {error}");
        }
    }
}

fn open_append(path: &Path) -> Result<File, TraceError> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|source| TraceError::Backend {
            message: format!("open {}: {source}", path.display()),
        })
}

fn rotated_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".1");
    PathBuf::from(name)
}

/// Overlay per-event IDs on the invocation's ambient correlation.
fn merge(base: &TraceCorrelation, event: TraceCorrelation) -> TraceCorrelation {
    TraceCorrelation {
        individual_id: event.individual_id.or(base.individual_id),
        node_id: event.node_id.or(base.node_id),
        boot_id: event.boot_id.or(base.boot_id),
        session_id: event.session_id.or(base.session_id),
        turn_id: event.turn_id.or(base.turn_id),
        episode_id: event.episode_id.or(base.episode_id),
        evidence_id: event.evidence_id,
        proposal_id: event.proposal_id,
        commit_id: event.commit_id,
        receipt_id: event.receipt_id,
        artifact_id: event.artifact_id,
        chunk_id: event.chunk_id,
        resource_id: event.resource_id,
        resource_call_id: event.resource_call_id,
        workspace_digest: event.workspace_digest,
        process_id: event.process_id.or(base.process_id),
    }
}

/// Read a JSONL trace back, for inspection and tests.
pub fn read_trace(path: impl AsRef<Path>) -> Result<Vec<TraceEvent>, TraceError> {
    let text = std::fs::read_to_string(path.as_ref()).map_err(|source| TraceError::Backend {
        message: format!("read {}: {source}", path.as_ref().display()),
    })?;
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|source| TraceError::Encode {
                message: format!("line {line:?}: {source}"),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use kamimusuhi_core::ids::{EvidenceId, IndividualId};
    use kamimusuhi_testkit::{FixedClock, FixedIdGenerator};

    use super::*;

    fn recorder(dir: &Path, seed: u64) -> TraceRecorder {
        let sink = JsonlTraceSink::open(
            dir.join(JsonlTraceSink::FILE_NAME),
            &FixedIdGenerator::new(seed),
        )
        .unwrap();
        TraceRecorder::new(
            sink,
            Arc::new(FixedClock::baseline()),
            TraceCorrelation {
                individual_id: Some(IndividualId::from_u128(0xA1)),
                process_id: Some(std::process::id()),
                ..TraceCorrelation::default()
            },
        )
    }

    #[test]
    fn events_are_appended_one_per_line_in_sequence() {
        let dir = tempfile::tempdir().unwrap();
        let recorder = recorder(dir.path(), 1);
        recorder.record(TraceEventKind::RuntimeBoot, TraceCorrelation::default());
        recorder.record(
            TraceEventKind::EvidenceRecorded,
            TraceCorrelation {
                evidence_id: Some(EvidenceId::from_u128(0xE1)),
                ..TraceCorrelation::default()
            },
        );

        let events = read_trace(recorder.path()).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].sequence, 0);
        assert_eq!(events[1].sequence, 1);
        assert_eq!(events[0].event_kind, TraceEventKind::RuntimeBoot);
        // Ambient correlation is filled in without the caller repeating it.
        assert_eq!(
            events[1].correlation.individual_id,
            Some(IndividualId::from_u128(0xA1))
        );
        assert_eq!(
            events[1].correlation.evidence_id,
            Some(EvidenceId::from_u128(0xE1))
        );
        // Per-event IDs do not leak onto later events.
        assert_eq!(events[0].correlation.evidence_id, None);
    }

    #[test]
    fn the_trace_rotates_instead_of_growing_without_end() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(JsonlTraceSink::FILE_NAME);
        // A cap small enough that the second event triggers rotation.
        let sink = JsonlTraceSink::open_with_limit(&path, &FixedIdGenerator::new(1), 64).unwrap();
        let recorder = TraceRecorder::new(
            sink,
            Arc::new(FixedClock::baseline()),
            TraceCorrelation::default(),
        );
        for _ in 0..6 {
            recorder.record(TraceEventKind::RuntimeBoot, TraceCorrelation::default());
        }

        let rotated = rotated_path(&path);
        assert!(rotated.exists(), "the previous generation is kept");
        assert!(std::fs::metadata(&path).unwrap().len() < 6 * 64);
        // Whole lines on both sides: rotation never splits an event.
        for file in [&path, &rotated] {
            for line in std::fs::read_to_string(file).unwrap().lines() {
                serde_json::from_str::<TraceEvent>(line)
                    .unwrap_or_else(|e| panic!("rotation split a line: {line:?}: {e}"));
            }
        }
    }

    #[test]
    fn a_second_recorder_appends_rather_than_truncating() {
        let dir = tempfile::tempdir().unwrap();
        recorder(dir.path(), 1).record(TraceEventKind::RuntimeBoot, TraceCorrelation::default());
        let second = recorder(dir.path(), 2);
        second.record(TraceEventKind::RuntimeStopping, TraceCorrelation::default());

        let events = read_trace(second.path()).unwrap();
        assert_eq!(events.len(), 2);
        // Different invocations, so different trace IDs on the same file.
        assert_ne!(events[0].trace_id, events[1].trace_id);
    }
}
