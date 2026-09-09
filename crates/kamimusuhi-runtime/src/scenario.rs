//! The deterministic W4 vertical slice.
//!
//! Two phases, meant to be run by two different processes against one runtime
//! directory:
//!
//! ```text
//! first    input -> evidence -> proposal -> policy -> activation
//!          Library import + retrieval
//!          Fake A invocation
//!          typed workspace
//!          exit
//!
//! resume   restore the same individual and head from disk
//!          fresh session, fresh turn, fresh input, no prior chat buffer
//!          relationship retrieval + Library retrieval
//!          Fake B invocation
//!          typed workspace + Persona Core turn
//! ```
//!
//! What makes `resume` mean anything is what it is *not* given: no messages,
//! no prompt, no serialized conversation, nothing from the previous process's
//! heap. It is handed a directory. Everything it knows it reads from the
//! canonical store, the Library and the runtime config.
//!
//! `resume` performs no canonical mutation on purpose, so "the head did not
//! move" is a clean claim about resource replacement rather than a claim
//! entangled with whatever else the phase happened to write.
//!
//! Fixture strings here are test data. They are not Kamimusuhi's persona.

use std::fmt;
use std::str::FromStr;

use kamimusuhi_core::continuity::{ActivationOutcome, ContinuityHead};
use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceSource, EvidenceStore, NewEvidence, NewSession, NewTurn, RetentionClass,
};
use kamimusuhi_core::ids::{
    CognitiveEpisodeId, CommitId, EvidenceId, IndividualId, LibraryArtifactId, LibraryChunkId,
    MemoryId, ProposalId, ReceiptId, ResourceCallId, ResourceId, SessionId, TraceId, TurnId,
};
use kamimusuhi_core::library::{
    LibraryHit, LibraryMediaType, LibraryQuery, LibraryRepository, NewLibraryArtifact,
};
use kamimusuhi_core::memory::{AttributedMemory, MemoryQuery, MemoryRepository};
use kamimusuhi_core::mutation::{
    MutationDomain, MutationPolicyV0, MutationProposal, OriginClass, ProposalAttribution,
};
use kamimusuhi_core::persona::{CurrentInput, PersonaCore, PersonaTurnInput, TurnContext};
use kamimusuhi_core::resources::{ResourceRequest, ResourceSlot};
use kamimusuhi_core::trace::{TraceCorrelation, TraceEventKind};
use kamimusuhi_core::workspace::{
    SourceRef, Workspace, WorkspaceBuilder, WorkspaceDomain, WorkspaceItem,
};
use kamimusuhi_testkit::FakePersonaCore;
use serde::{Deserialize, Serialize};

use crate::config::GENERAL_SLOT;
use crate::error::RuntimeError;
use crate::runtime::Runtime;

/// Fixture: what the user says in the first process. The trailing
/// 「覚えておいて」 is what makes the Fake Persona Core draft a relationship
/// fact, so the fixture exercises the real proposal path.
pub const FIRST_INPUT: &str = "私はほうじ茶が好き。覚えておいて";

/// Fixture: what the user says after the restart. Deliberately produces no
/// proposal, so the phase is read-only and the head provably does not move.
pub const RESUME_INPUT: &str = "さっきの話、覚えてる?";

/// Fixture Library document. Plain external material: not a belief, and not
/// evidence about the user.
pub const LIBRARY_TITLE: &str = "お茶ノート";
pub const LIBRARY_SOURCE_URI: &str = "fixture:///tea-notes.md";
pub const LIBRARY_CONTENT: &str = "# お茶の淹れ方\n\nほうじ茶は高温で淹れる。\n\n玄米茶も高温で淹れる。\n\n## 抹茶\n\n抹茶は茶筅で点てる。\n";
pub const LIBRARY_QUERY: &str = "ほうじ茶";

/// The subject the fixture relationship fact is about.
pub const SUBJECT: &str = "user-fixture";

/// Which half of the scenario to run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DemoPhase {
    /// Process A: record, propose, activate, import, invoke Fake A, exit.
    First,
    /// Process B: restore from disk and think again with a different resource.
    Resume,
}

impl DemoPhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Resume => "resume",
        }
    }
}

impl fmt::Display for DemoPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DemoPhase {
    type Err = RuntimeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "first" => Ok(Self::First),
            "resume" => Ok(Self::Resume),
            other => Err(RuntimeError::Usage(format!(
                "unknown --phase {other:?}; expected first or resume"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeadReport {
    pub commit_id: CommitId,
    pub generation: u64,
    pub writer_epoch: u64,
}

impl From<ContinuityHead> for HeadReport {
    fn from(head: ContinuityHead) -> Self {
        Self {
            commit_id: head.commit_id,
            generation: head.generation.0,
            writer_epoch: head.writer_epoch.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalReport {
    pub proposal_id: ProposalId,
    pub reason_code: String,
    pub activated: bool,
    pub commit_id: Option<CommitId>,
    pub receipt_id: Option<ReceiptId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryReport {
    pub artifact_id: LibraryArtifactId,
    pub chunk_id: LibraryChunkId,
    pub ordinal: u32,
    pub chunk_count: u32,
    pub hit_count: usize,
    pub imported_now: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceReport {
    pub slot: String,
    pub implementation: String,
    /// The resource that actually answered, from its own descriptor.
    pub resource_id: ResourceId,
    pub resource_call_id: ResourceCallId,
    /// The turn the call is correlated to in the database.
    pub turn_id: Option<TurnId>,
    /// Physical tries behind this one logical call.
    pub attempts: u32,
    pub latency_ms: u64,
    pub answer: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryReport {
    pub state_record_id: MemoryId,
    pub subject_key: Option<String>,
    pub payload: serde_json::Value,
    pub evidence_refs: Vec<EvidenceId>,
    pub independent_evidence_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceItemReport {
    pub position: u32,
    pub domain: WorkspaceDomain,
    pub authority: String,
    /// The typed source ref, verbatim. A reader distinguishes domains by this,
    /// never by reading the item's content.
    pub source_ref: SourceRef,
}

impl From<&WorkspaceItem> for WorkspaceItemReport {
    fn from(item: &WorkspaceItem) -> Self {
        Self {
            position: item.position,
            domain: item.domain,
            authority: item.authority.as_str().to_owned(),
            source_ref: item.source_ref.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceReport {
    pub digest: String,
    pub domains: Vec<WorkspaceDomain>,
    pub items: Vec<WorkspaceItemReport>,
}

/// Everything one phase did, as machine-readable output.
///
/// The child-process test reads this from stdout: it is how one process
/// reports to the harness without any process sharing memory with another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseReport {
    pub phase: DemoPhase,
    pub process_id: u32,
    pub trace_id: TraceId,
    pub individual_id: IndividualId,
    pub node_id: kamimusuhi_core::ids::NodeId,
    pub boot_id: kamimusuhi_core::ids::BootId,
    pub writer_epoch: u64,
    pub schema_version: u32,
    pub head_before: HeadReport,
    pub head_after: HeadReport,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub episode_id: CognitiveEpisodeId,
    pub current_input_evidence_id: EvidenceId,
    /// Always `"none"`. The phase is handed a directory, never a transcript.
    pub prior_context: String,
    pub proposal: Option<ProposalReport>,
    pub library: LibraryReport,
    pub resource: ResourceReport,
    pub relationship: Vec<MemoryReport>,
    pub workspace: WorkspaceReport,
    pub persona_response: String,
}

/// Run one phase of the scenario against an opened runtime.
pub fn run(runtime: &mut Runtime, phase: DemoPhase) -> Result<PhaseReport, RuntimeError> {
    let individual_id = runtime.individual_id();

    // Session and turn. Fresh in both phases: resuming an individual is not
    // resuming a conversation.
    //
    // Minted before the writer epoch is claimed, because the first of them is
    // how an id-source collision is detected — and detection has to happen
    // before the first write, not after one has already been attempted.
    let session_id = SessionId::generate(runtime.ids().as_ref());
    let turn_id = TurnId::generate(runtime.ids().as_ref());
    let episode_id = CognitiveEpisodeId::generate(runtime.ids().as_ref());

    // The runtime only ever mints fresh IDs, so an ID that already exists is
    // not a retry — it is another process replaying the same seed. Refuse
    // before writing anything rather than quietly reusing its records.
    if runtime.store().session(session_id)?.is_some() {
        return Err(RuntimeError::IdCollision {
            kind: "session",
            id: session_id.to_string(),
        });
    }

    let writer = runtime.claim_writer()?;
    let head_before = runtime.head()?;
    runtime.set_trace_base(TraceCorrelation {
        session_id: Some(session_id),
        turn_id: Some(turn_id),
        episode_id: Some(episode_id),
        ..runtime.trace().base()
    });

    runtime.store().open_session(NewSession {
        session_id,
        individual_id,
    })?;
    runtime
        .trace()
        .record(TraceEventKind::SessionStarted, TraceCorrelation::default());
    runtime.store().record_turn(NewTurn {
        turn_id,
        session_id,
        individual_id,
        sequence: 0,
    })?;
    runtime
        .trace()
        .record(TraceEventKind::TurnStarted, TraceCorrelation::default());

    // The current utterance, recorded as canonical raw evidence. This is a
    // different act from putting it in the workspace: the evidence record is
    // what a later proposal may cite, the workspace item is only what this
    // turn is thinking with.
    let text = match phase {
        DemoPhase::First => FIRST_INPUT,
        DemoPhase::Resume => RESUME_INPUT,
    };
    let evidence_id = EvidenceId::generate(runtime.ids().as_ref());
    let evidence = runtime.store().append(NewEvidence {
        evidence_id,
        individual_id,
        session_id: Some(session_id),
        turn_id: Some(turn_id),
        kind: EvidenceKind::UserUtterance,
        origin_class: OriginClass::Reported,
        payload: serde_json::json!({ "text": text }),
        source: EvidenceSource {
            source_id: Some("fixture-channel".to_owned()),
            source_sequence: Some(0),
            content_digest: Some(kamimusuhi_core::digest::content_digest(text.as_bytes())),
        },
        retention_class: RetentionClass::Standard,
    })?;
    // The trace names the record and its digest; the words stay in the
    // evidence store, which is the thing that owns them.
    runtime.trace().record_with(
        TraceEventKind::EvidenceRecorded,
        TraceCorrelation {
            evidence_id: Some(evidence.evidence_id),
            ..TraceCorrelation::default()
        },
        serde_json::json!({
            "kind": evidence.kind,
            "origin_class": evidence.origin_class,
            "content_digest": evidence.source.content_digest,
        }),
    );

    let current_input = CurrentInput {
        evidence_id,
        text: text.to_owned(),
    };
    let context = TurnContext {
        individual_id,
        session_id,
        turn_id,
    };

    // Phase A proposes and activates through the ordinary W1/W2 path. The
    // runtime never writes canonical state itself.
    let proposal = match phase {
        DemoPhase::First => Some(propose_and_activate(
            runtime,
            &context,
            &current_input,
            head_before,
            writer,
        )?),
        DemoPhase::Resume => None,
    };

    // Library: imported once in phase A, retrieved in both. Neither touches
    // the head.
    let library = library_step(runtime, phase)?;

    // The external resource, whichever implementation the config currently
    // names for the slot.
    let resource = resource_step(runtime, &current_input, turn_id)?;

    // Durable memory, retrieved from disk. In phase B this is the only place
    // the earlier conversation can come from.
    let memories = MemoryRepository::retrieve(
        runtime.store(),
        &MemoryQuery::current(individual_id).in_domain(MutationDomain::Relationship),
    )?;

    let workspace = build_workspace(runtime, &current_input, &memories, &library.1, &resource.1)?;
    runtime.trace().record_with(
        TraceEventKind::WorkspaceAssembled,
        TraceCorrelation {
            workspace_digest: Some(workspace.digest()),
            ..TraceCorrelation::default()
        },
        serde_json::json!({
            "item_count": workspace.len(),
            "domains": workspace.domains(),
        }),
    );

    let turn = FakePersonaCore.turn(PersonaTurnInput {
        context,
        input: current_input,
        workspace: Some(workspace.clone()),
    })?;
    runtime.trace().record_with(
        TraceEventKind::PersonaCompleted,
        TraceCorrelation {
            workspace_digest: Some(workspace.digest()),
            ..TraceCorrelation::default()
        },
        serde_json::json!({ "draft_count": turn.proposals.len() }),
    );
    runtime
        .trace()
        .record(TraceEventKind::ResponseEmitted, TraceCorrelation::default());

    let head_after = runtime.head()?;
    Ok(PhaseReport {
        phase,
        process_id: std::process::id(),
        trace_id: runtime.trace().trace_id(),
        individual_id,
        node_id: runtime.config().node_id,
        boot_id: runtime.boot_id(),
        writer_epoch: writer.writer_epoch.0,
        schema_version: runtime.schema_version().0,
        head_before: head_before.into(),
        head_after: head_after.into(),
        session_id,
        turn_id,
        episode_id,
        current_input_evidence_id: evidence_id,
        prior_context: "none".to_owned(),
        proposal,
        library: library.0,
        resource: resource.0,
        relationship: memories.iter().map(memory_report).collect(),
        workspace: WorkspaceReport {
            digest: workspace.digest(),
            domains: workspace.domains(),
            items: workspace
                .items
                .iter()
                .map(WorkspaceItemReport::from)
                .collect(),
        },
        persona_response: turn.response_intent,
    })
}

/// Draft through the Persona Core, attribute, and submit to the kernel.
fn propose_and_activate(
    runtime: &Runtime,
    context: &TurnContext,
    input: &CurrentInput,
    head: ContinuityHead,
    writer: kamimusuhi_core::continuity::WriterIdentity,
) -> Result<ProposalReport, RuntimeError> {
    // The draft carries evidence refs and content; it carries no authority.
    // Head, writer and policy version are the runtime's to attach.
    let drafted = FakePersonaCore.turn(PersonaTurnInput {
        context: *context,
        input: input.clone(),
        workspace: None,
    })?;
    let draft =
        drafted
            .proposals
            .into_iter()
            .next()
            .ok_or_else(|| RuntimeError::MutationNotActivated {
                reason: "the persona fixture drafted no proposal for the first-phase input"
                    .to_owned(),
            })?;

    let proposal = MutationProposal::from_draft(
        draft,
        ProposalAttribution {
            proposal_id: ProposalId::generate(runtime.ids().as_ref()),
            individual_id: context.individual_id,
            expected_head: head.expected(),
            requested_by: writer,
            policy_version: MutationPolicyV0::VERSION,
            // Deterministic and unique per turn, so a retry of this turn is
            // recognised as the same logical mutation.
            idempotency_key: format!("{}/{}/draft-0", context.session_id, context.turn_id),
            created_at: runtime.now(),
        },
    );
    runtime.trace().record_with(
        TraceEventKind::MutationProposed,
        TraceCorrelation {
            proposal_id: Some(proposal.proposal_id),
            evidence_id: proposal.evidence_refs.first().copied(),
            ..TraceCorrelation::default()
        },
        serde_json::json!({
            "domain": proposal.domain,
            "operation": proposal.operation,
            "origin_class": proposal.origin_class,
            "evidence_count": proposal.evidence_refs.len(),
        }),
    );

    let outcome = runtime.kernel().submit(&proposal)?;
    let (reason_code, activated, commit_id, receipt_id) = match &outcome {
        ActivationOutcome::Activated(receipt) | ActivationOutcome::AlreadyActivated(receipt) => (
            kamimusuhi_core::mutation::ReasonCode::Accepted
                .as_str()
                .to_owned(),
            true,
            Some(receipt.commit_id),
            Some(receipt.receipt_id),
        ),
        ActivationOutcome::Rejected(decision) => {
            (decision.reason_code.as_str().to_owned(), false, None, None)
        }
    };
    runtime.trace().record_with(
        TraceEventKind::MutationDecided,
        TraceCorrelation {
            proposal_id: Some(proposal.proposal_id),
            ..TraceCorrelation::default()
        },
        serde_json::json!({ "reason_code": reason_code, "activated": activated }),
    );
    if let Some(receipt) = outcome.receipt() {
        // The canonical record of this activation is the audit event written
        // inside the transaction. This line only observes that it happened.
        runtime.trace().record_with(
            TraceEventKind::ContinuityReceiptObserved,
            TraceCorrelation {
                proposal_id: Some(proposal.proposal_id),
                commit_id: Some(receipt.commit_id),
                receipt_id: Some(receipt.receipt_id),
                ..TraceCorrelation::default()
            },
            serde_json::json!({ "generation": receipt.generation }),
        );
    }
    if !activated {
        return Err(RuntimeError::MutationNotActivated {
            reason: reason_code,
        });
    }

    Ok(ProposalReport {
        proposal_id: proposal.proposal_id,
        reason_code,
        activated,
        commit_id,
        receipt_id,
    })
}

/// Import (phase A only) and retrieve the fixture document.
fn library_step(
    runtime: &Runtime,
    phase: DemoPhase,
) -> Result<(LibraryReport, Vec<LibraryHit>), RuntimeError> {
    let artifact = match phase {
        DemoPhase::First => {
            let artifact_id = LibraryArtifactId::generate(runtime.ids().as_ref());
            let imported = runtime.store().import(NewLibraryArtifact {
                artifact_id,
                source_uri: Some(LIBRARY_SOURCE_URI.to_owned()),
                title: Some(LIBRARY_TITLE.to_owned()),
                media_type: LibraryMediaType::Markdown,
                content: LIBRARY_CONTENT.to_owned(),
            })?;
            runtime.trace().record_with(
                TraceEventKind::LibraryImported,
                TraceCorrelation {
                    artifact_id: Some(imported.artifact_id),
                    ..TraceCorrelation::default()
                },
                serde_json::json!({
                    "chunk_count": imported.chunk_count,
                    "content_digest": imported.content_digest,
                    "chunker_version": imported.chunker_version,
                }),
            );
            Some(imported)
        }
        // Phase B does not re-import: the document has to still be there.
        DemoPhase::Resume => None,
    };

    let hits = LibraryRepository::retrieve(
        runtime.store(),
        &LibraryQuery::new(LIBRARY_QUERY).limited(1),
    )?;
    let hit = hits.first().ok_or_else(|| {
        RuntimeError::Library(kamimusuhi_core::library::LibraryError::Invalid {
            reason: format!("the fixture query {LIBRARY_QUERY:?} matched no chunk"),
        })
    })?;
    runtime.trace().record_with(
        TraceEventKind::LibraryRetrieved,
        TraceCorrelation {
            artifact_id: Some(hit.artifact_id()),
            chunk_id: Some(hit.chunk_id()),
            ..TraceCorrelation::default()
        },
        serde_json::json!({
            "hit_count": hits.len(),
            "ordinal": hit.chunk.ordinal,
            "matched_terms": hit.matched_terms,
        }),
    );

    let report = LibraryReport {
        artifact_id: hit.artifact_id(),
        chunk_id: hit.chunk_id(),
        ordinal: hit.chunk.ordinal,
        chunk_count: hit.artifact.chunk_count,
        hit_count: hits.len(),
        imported_now: artifact.is_some(),
    };
    Ok((report, hits))
}

/// Invoke whatever fills the general slot, and keep its own attribution.
fn resource_step(
    runtime: &Runtime,
    input: &CurrentInput,
    turn_id: TurnId,
) -> Result<
    (
        ResourceReport,
        Vec<kamimusuhi_core::resources::AttributedResult>,
    ),
    RuntimeError,
> {
    let implementation = runtime
        .config()
        .implementation(GENERAL_SLOT)
        .ok_or_else(|| RuntimeError::SlotNotConfigured {
            slot: GENERAL_SLOT.to_owned(),
        })?;
    let registry = runtime.config().build_registry()?;
    let slot = ResourceSlot::new(GENERAL_SLOT);
    let descriptor = registry
        .descriptor(&slot)
        .ok_or_else(|| RuntimeError::SlotNotConfigured {
            slot: GENERAL_SLOT.to_owned(),
        })?;
    runtime.trace().record_with(
        TraceEventKind::ResourceSelected,
        TraceCorrelation {
            resource_id: Some(descriptor.resource_id),
            ..TraceCorrelation::default()
        },
        serde_json::json!({
            "slot": slot.as_str(),
            "implementation": implementation.as_str(),
            "adapter": descriptor.adapter,
            "read_only": descriptor.read_only,
        }),
    );

    // The evidence ID, not the utterance, is what the request is about: the
    // fake needs something deterministic to echo, not the private text.
    let request = ResourceRequest::new(
        runtime.individual_id(),
        "summarize-turn",
        serde_json::json!({ "evidence_id": input.evidence_id }),
    )
    // Correlates the durable call row with the turn, so the relationship
    // survives even if the operational trace is rotated away.
    .in_turn(turn_id);
    let call_id = ResourceCallId::generate(runtime.ids().as_ref());
    // Wall time places the call; the monotonic clock measures it. Retry, if
    // the configured implementation does any, happens inside the adapter and
    // stays inside this one logical call.
    let attributed =
        registry.invoke_timed(&slot, &request, runtime.store(), call_id, runtime.clocks())?;
    runtime.trace().record_with(
        TraceEventKind::ResourceCompleted,
        TraceCorrelation {
            resource_id: Some(attributed.result.resource_id),
            resource_call_id: Some(attributed.call.resource_call_id),
            ..TraceCorrelation::default()
        },
        serde_json::json!({
            "outcome": attributed.call.outcome,
            "request_digest": attributed.call.request_digest,
            "result_digest": attributed.call.result_digest,
            "attempts": attributed.call.attempts,
            "latency_ms": attributed.call.latency_ms,
        }),
    );

    let answer = attributed
        .result
        .content
        .get("answer")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let report = ResourceReport {
        slot: slot.as_str().to_owned(),
        implementation: implementation.as_str().to_owned(),
        resource_id: attributed.result.resource_id,
        resource_call_id: attributed.call.resource_call_id,
        turn_id: attributed.call.turn_id,
        attempts: attributed.call.attempts,
        latency_ms: attributed.call.latency_ms,
        answer,
    };
    Ok((report, vec![attributed]))
}

/// Assemble the four domains W4 has to keep apart.
fn build_workspace(
    runtime: &Runtime,
    input: &CurrentInput,
    memories: &[AttributedMemory],
    hits: &[LibraryHit],
    results: &[kamimusuhi_core::resources::AttributedResult],
) -> Result<Workspace, RuntimeError> {
    let head = runtime.head()?;
    Ok(
        WorkspaceBuilder::new(runtime.individual_id(), runtime.now())
            .with_continuity(&head)
            .with_current_input(input)
            .with_memories(memories)
            .with_library_hits(hits)
            .with_resource_results(results)
            .build(),
    )
}

fn memory_report(memory: &AttributedMemory) -> MemoryReport {
    MemoryReport {
        state_record_id: memory.record.state_record_id,
        subject_key: memory.record.subject_key.clone(),
        payload: memory.record.payload.clone(),
        evidence_refs: memory.record.evidence_refs.clone(),
        independent_evidence_count: memory.independent_evidence_count,
    }
}
