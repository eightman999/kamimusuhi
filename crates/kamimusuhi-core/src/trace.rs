//! Operational trace.
//!
//! A trace answers "what did the runtime do", and it is a different thing from
//! [`crate::audit`], which answers "what changed in canonical state and why"
//! (plan §12). The audit event is written inside the same transaction as the
//! transition it explains and is part of the individual's accountability; a
//! trace event is observability, written outside any canonical transaction and
//! owning nothing.
//!
//! Three consequences are load-bearing:
//!
//! - **Trace is never authority.** Nothing may be read back out of a trace as
//!   self state, evidence, or permission. It is a record *about* the runtime,
//!   not a record the runtime believes.
//! - **A failed trace write is not a failed transition, and a successful trace
//!   write is not a durable audit record.** The two must never be reported as
//!   each other.
//! - **Correlation, not transcription.** Events carry stable IDs, kinds,
//!   digests and counts. Raw utterances stay in the evidence store and are
//!   referenced by [`EvidenceId`]; copying them here would duplicate private
//!   content into a file with weaker guarantees.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::ids::{
    BootId, CognitiveEpisodeId, CommitId, EvidenceId, IndividualId, LibraryArtifactId,
    LibraryChunkId, NodeId, ProposalId, ReceiptId, ResourceCallId, ResourceId, SessionId, TraceId,
    TurnId,
};
use crate::mutation::UnknownVocabulary;
use crate::time::UtcTimestamp;

/// What happened. One variant per step the W4 scenario needs to reconstruct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TraceEventKind {
    /// A process opened the runtime. Carries the boot identity, which is what
    /// makes a restart visible in the trace at all.
    #[serde(rename = "runtime.boot")]
    RuntimeBoot,
    #[serde(rename = "runtime.stopping")]
    RuntimeStopping,
    #[serde(rename = "session.started")]
    SessionStarted,
    #[serde(rename = "turn.started")]
    TurnStarted,
    /// A raw utterance was appended to the evidence store.
    #[serde(rename = "evidence.recorded")]
    EvidenceRecorded,
    #[serde(rename = "persona.completed")]
    PersonaCompleted,
    #[serde(rename = "mutation.proposed")]
    MutationProposed,
    #[serde(rename = "mutation.decided")]
    MutationDecided,
    /// An activation receipt was observed by the runtime. The canonical
    /// record of the activation is the audit event, not this.
    #[serde(rename = "continuity.receipt_observed")]
    ContinuityReceiptObserved,
    /// Durable episodic/relationship state was read back. A different act
    /// from [`Self::LibraryRetrieved`]: one is the individual's own state,
    /// the other is external material, and the trace keeps them apart for the
    /// same reason the workspace does.
    #[serde(rename = "memory.retrieved")]
    MemoryRetrieved,
    #[serde(rename = "library.imported")]
    LibraryImported,
    #[serde(rename = "library.retrieved")]
    LibraryRetrieved,
    /// The router chose a resource, or refused to. Operational only: a
    /// routing decision is not something the individual believes.
    #[serde(rename = "routing.decided")]
    RoutingDecided,
    #[serde(rename = "resource.selected")]
    ResourceSelected,
    #[serde(rename = "resource.completed")]
    ResourceCompleted,
    #[serde(rename = "workspace.assembled")]
    WorkspaceAssembled,
    /// A read-only inspection ran. Recorded here precisely because it is not
    /// a canonical event.
    #[serde(rename = "inspect.invoked")]
    InspectInvoked,
    #[serde(rename = "response.emitted")]
    ResponseEmitted,
}

impl TraceEventKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RuntimeBoot => "runtime.boot",
            Self::RuntimeStopping => "runtime.stopping",
            Self::SessionStarted => "session.started",
            Self::TurnStarted => "turn.started",
            Self::EvidenceRecorded => "evidence.recorded",
            Self::PersonaCompleted => "persona.completed",
            Self::MutationProposed => "mutation.proposed",
            Self::MutationDecided => "mutation.decided",
            Self::ContinuityReceiptObserved => "continuity.receipt_observed",
            Self::MemoryRetrieved => "memory.retrieved",
            Self::LibraryImported => "library.imported",
            Self::LibraryRetrieved => "library.retrieved",
            Self::RoutingDecided => "routing.decided",
            Self::ResourceSelected => "resource.selected",
            Self::ResourceCompleted => "resource.completed",
            Self::WorkspaceAssembled => "workspace.assembled",
            Self::InspectInvoked => "inspect.invoked",
            Self::ResponseEmitted => "response.emitted",
        }
    }
}

impl fmt::Display for TraceEventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TraceEventKind {
    type Err = UnknownVocabulary;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "runtime.boot" => Self::RuntimeBoot,
            "runtime.stopping" => Self::RuntimeStopping,
            "session.started" => Self::SessionStarted,
            "turn.started" => Self::TurnStarted,
            "evidence.recorded" => Self::EvidenceRecorded,
            "persona.completed" => Self::PersonaCompleted,
            "mutation.proposed" => Self::MutationProposed,
            "mutation.decided" => Self::MutationDecided,
            "continuity.receipt_observed" => Self::ContinuityReceiptObserved,
            "memory.retrieved" => Self::MemoryRetrieved,
            "library.imported" => Self::LibraryImported,
            "library.retrieved" => Self::LibraryRetrieved,
            "routing.decided" => Self::RoutingDecided,
            "resource.selected" => Self::ResourceSelected,
            "resource.completed" => Self::ResourceCompleted,
            "workspace.assembled" => Self::WorkspaceAssembled,
            "inspect.invoked" => Self::InspectInvoked,
            "response.emitted" => Self::ResponseEmitted,
            other => return Err(UnknownVocabulary::new("trace_event_kind", other)),
        })
    }
}

/// Typed IDs an event can correlate on. Every field is optional because
/// different events legitimately know different things; what matters is that
/// following the scenario never requires reading a message string.
///
/// `#[serde(skip_serializing_if)]` keeps each JSONL line to the IDs the event
/// actually has, so an absent field means "not applicable", not "unknown".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceCorrelation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub individual_id: Option<IndividualId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<NodeId>,
    /// One process lifetime. Two events with different boot IDs came from
    /// different runs, which is how a restart is found in the trace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_id: Option<BootId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_id: Option<CognitiveEpisodeId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_id: Option<EvidenceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_id: Option<ProposalId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_id: Option<CommitId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_id: Option<ReceiptId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_id: Option<LibraryArtifactId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chunk_id: Option<LibraryChunkId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<ResourceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_call_id: Option<ResourceCallId>,
    /// Digest of the assembled workspace, so two assemblies can be compared
    /// without copying their contents into the trace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_digest: Option<String>,
    /// OS process ID. Not an identity — a correlation aid that makes the
    /// process boundary legible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<u32>,
}

/// One line of the operational trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceEvent {
    /// Correlates every event produced by one runtime invocation.
    pub trace_id: TraceId,
    /// Monotonic within a trace file. Ordering does not depend on timestamps,
    /// which can repeat under a fixed clock.
    pub sequence: u64,
    pub event_kind: TraceEventKind,
    pub recorded_at: UtcTimestamp,
    #[serde(default)]
    pub correlation: TraceCorrelation,
    /// Structured, non-private detail: counts, digests, status codes, kinds.
    /// Never a raw utterance, a candidate payload, or a credential.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub detail: serde_json::Value,
}

impl TraceEvent {
    pub fn new(
        trace_id: TraceId,
        sequence: u64,
        event_kind: TraceEventKind,
        recorded_at: UtcTimestamp,
    ) -> Self {
        Self {
            trace_id,
            sequence,
            event_kind,
            recorded_at,
            correlation: TraceCorrelation::default(),
            detail: serde_json::Value::Null,
        }
    }

    pub fn with_correlation(mut self, correlation: TraceCorrelation) -> Self {
        self.correlation = correlation;
        self
    }

    pub fn with_detail(mut self, detail: serde_json::Value) -> Self {
        self.detail = detail;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TraceError {
    #[error("trace event could not be encoded: {message}")]
    Encode { message: String },
    #[error("trace sink backend error: {message}")]
    Backend { message: String },
}

/// Somewhere trace events are written.
///
/// Implementations must not participate in canonical transactions, and a
/// caller must treat a failure here as a lost observation, never as a failed
/// or successful state transition.
pub trait TraceSink: Send + Sync {
    fn emit(&self, event: &TraceEvent) -> Result<(), TraceError>;
}

/// A sink that drops everything. For paths where tracing is switched off.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullTraceSink;

impl TraceSink for NullTraceSink {
    fn emit(&self, _: &TraceEvent) -> Result<(), TraceError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_kind_vocabulary_round_trips() {
        for kind in [
            TraceEventKind::RuntimeBoot,
            TraceEventKind::RuntimeStopping,
            TraceEventKind::SessionStarted,
            TraceEventKind::TurnStarted,
            TraceEventKind::EvidenceRecorded,
            TraceEventKind::PersonaCompleted,
            TraceEventKind::MutationProposed,
            TraceEventKind::MutationDecided,
            TraceEventKind::ContinuityReceiptObserved,
            TraceEventKind::MemoryRetrieved,
            TraceEventKind::LibraryImported,
            TraceEventKind::LibraryRetrieved,
            TraceEventKind::RoutingDecided,
            TraceEventKind::ResourceSelected,
            TraceEventKind::ResourceCompleted,
            TraceEventKind::WorkspaceAssembled,
            TraceEventKind::InspectInvoked,
            TraceEventKind::ResponseEmitted,
        ] {
            assert_eq!(kind.as_str().parse::<TraceEventKind>().unwrap(), kind);
        }
        assert!("mutation.activated".parse::<TraceEventKind>().is_err());
    }

    #[test]
    fn a_line_carries_only_the_ids_the_event_has() {
        let event = TraceEvent::new(
            TraceId::from_u128(1),
            7,
            TraceEventKind::EvidenceRecorded,
            UtcTimestamp::from_unix_millis(5),
        )
        .with_correlation(TraceCorrelation {
            individual_id: Some(IndividualId::from_u128(0xA1)),
            evidence_id: Some(EvidenceId::from_u128(0xE1)),
            ..TraceCorrelation::default()
        });
        let line = serde_json::to_string(&event).unwrap();
        assert!(line.contains("evidence.recorded"));
        assert!(line.contains("\"evidence_id\""));
        // Absent IDs are absent, not null: an event says what it knows.
        assert!(!line.contains("\"proposal_id\""));
        assert!(!line.contains("\"resource_id\""));

        let back: TraceEvent = serde_json::from_str(&line).unwrap();
        assert_eq!(back, event);
    }

    #[test]
    fn the_null_sink_accepts_everything() {
        let event = TraceEvent::new(
            TraceId::from_u128(1),
            0,
            TraceEventKind::RuntimeBoot,
            UtcTimestamp::from_unix_millis(0),
        );
        assert_eq!(NullTraceSink.emit(&event), Ok(()));
    }
}
