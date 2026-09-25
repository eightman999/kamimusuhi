//! C0 derived lane at runtime: the movable-head operative view, lexical
//! retrieval, proposal intake and the activation/rollback orchestration.
//!
//! Canonical state is untouched by everything in this module — activations
//! and rollbacks move only the derived lane's head, and every lifecycle
//! transition is *also* written into canonical evidence so the immutable
//! record narrates what the lane did.

pub mod chat;
pub mod eval;
pub mod reflection;
pub mod replay;

use std::collections::BTreeSet;

use kamimusuhi_core::c0::{
    Activation, ImprovementKind, ImprovementProposal, OperativeView, ProposalStatus,
};
use kamimusuhi_core::continuity::WriterIdentity;
use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceRecord, EvidenceSource, EvidenceStore, NewEvidence, NewEvidenceLink,
    RetentionClass,
};
use kamimusuhi_core::ids::{
    C0ActivationId, C0ProposalId, EvidenceId, IndividualId, ProposalId, SessionId, TurnId,
};
use kamimusuhi_core::memory::{AttributedMemory, MemoryQuery, MemoryRepository};
use kamimusuhi_core::mutation::{
    MutationDomain, MutationPolicyV0, MutationProposal, OriginClass, ProposalAttribution,
};
use kamimusuhi_core::persona::ProposalDraft;
use kamimusuhi_core::trace::{TraceCorrelation, TraceEventKind};
use kamimusuhi_store_sqlite::SqliteStore;
use serde::Serialize;

use crate::{Runtime, RuntimeError};

impl From<kamimusuhi_core::c0::C0Error> for RuntimeError {
    fn from(e: kamimusuhi_core::c0::C0Error) -> Self {
        match e {
            kamimusuhi_core::c0::C0Error::NothingToRollBack => {
                RuntimeError::Usage("no C0 activation to roll back".to_owned())
            }
            other => RuntimeError::Usage(format!("C0 lane: {other}")),
        }
    }
}

/// The derived-lane head position plus the view it selects.
#[derive(Debug, Clone, Default)]
pub struct Operative {
    pub activation_seq: u64,
    pub view: OperativeView,
}

/// Read the operative view in force. Head 0 = documented defaults, and the
/// defaults are part of the type, not an implicit runtime choice.
pub fn operative(runtime: &Runtime) -> Result<Operative, RuntimeError> {
    let seq = runtime.store().c0_head(runtime.individual_id())?;
    let view = runtime.store().c0_view(runtime.individual_id())?;
    Ok(Operative {
        activation_seq: seq,
        view,
    })
}

// ---------------------------------------------------------------------------
// Lexical recall
// ---------------------------------------------------------------------------

/// Character-bigram coverage: |Q ∩ D| / |Q| over lowercase text with
/// whitespace removed. Deterministic, language-agnostic, adequate for both
/// Japanese and Latin text — bigrams give Japanese enough context that
/// single characters do not drown the match.
pub fn lexical_score(query: &str, text: &str) -> f64 {
    fn bigrams(s: &str) -> BTreeSet<(char, char)> {
        let normalized: Vec<char> = s
            .to_lowercase()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        normalized.windows(2).map(|w| (w[0], w[1])).collect()
    }
    let query = bigrams(query);
    if query.is_empty() {
        return 0.0;
    }
    let doc = bigrams(text);
    let shared = query.intersection(&doc).count();
    shared as f64 / query.len() as f64
}

/// Durable memories for this turn: relationship records about the subject,
/// plus episodic records about the subject ranked by lexical overlap with
/// the current input. All scoping stays subject-bound — nothing cross-subject
/// enters a turn's context.
pub fn retrieve_memories(
    store: &SqliteStore,
    individual_id: IndividualId,
    subject: &str,
    query_text: &str,
    params: &kamimusuhi_core::c0::OperativeParams,
) -> Result<Vec<AttributedMemory>, RuntimeError> {
    let mut memories = MemoryRepository::retrieve(
        store,
        &MemoryQuery::current(individual_id)
            .in_domain(MutationDomain::Relationship)
            .about(subject)
            .limited(params.retrieval.relationship_top_k),
    )?;

    // Episodic candidates: fetch a bounded pool, score in memory, keep the
    // ranked tail. Same input + same store state = same result.
    let pool = MemoryRepository::retrieve(
        store,
        &MemoryQuery::current(individual_id)
            .in_domain(MutationDomain::Episodic)
            .about(subject)
            .limited(64),
    )?;
    let mut scored: Vec<(f64, AttributedMemory)> = pool
        .into_iter()
        .map(|m| (lexical_score(query_text, &m.record.payload.to_string()), m))
        .filter(|(score, _)| *score >= params.retrieval.min_score)
        .collect();
    // Highest score first; ties break on creation time for determinism.
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.1.record.created_at.cmp(&a.1.record.created_at))
            .then(b.1.record.state_record_id.cmp(&a.1.record.state_record_id))
    });
    memories.extend(
        scored
            .into_iter()
            .take(params.retrieval.episodic_top_k)
            .map(|(_, m)| m),
    );
    Ok(memories)
}

/// Raw canonical utterances recalled by lexical match — older or outside the
/// visible history window, still first-party record. `exclude` holds ids
/// already presented this turn (current input, visible history).
pub fn recall_evidence(
    store: &SqliteStore,
    individual_id: IndividualId,
    source_id: &str,
    query_text: &str,
    params: &kamimusuhi_core::c0::OperativeParams,
    exclude: &BTreeSet<EvidenceId>,
) -> Result<Vec<EvidenceRecord>, RuntimeError> {
    if params.retrieval.evidence_top_k == 0 {
        return Ok(Vec::new());
    }
    let pool = store.c0_evidence_pool(individual_id, params.retrieval.evidence_pool)?;
    let mut scored: Vec<(f64, EvidenceRecord)> = pool
        .into_iter()
        .filter(|r| {
            matches!(
                r.kind,
                EvidenceKind::UserUtterance | EvidenceKind::AgentUtterance
            ) && r.source.source_id.as_deref() == Some(source_id)
                && !exclude.contains(&r.evidence_id)
        })
        .filter_map(|r| {
            let text = r.payload.get("text").and_then(|t| t.as_str())?;
            let score = lexical_score(query_text, text);
            (score >= params.retrieval.min_score).then_some((score, r))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.1.created_at.cmp(&a.1.created_at))
            .then(b.1.evidence_id.cmp(&a.1.evidence_id))
    });
    Ok(scored
        .into_iter()
        .take(params.retrieval.evidence_top_k)
        .map(|(_, r)| r)
        .collect())
}

// ---------------------------------------------------------------------------
// Canonical draft submission (lazy writer claim)
// ---------------------------------------------------------------------------

/// The outcome of submitting one canonical draft.
#[derive(Debug)]
pub struct DraftOutcome {
    pub proposal_id: ProposalId,
    pub activated: bool,
    pub reason_code: String,
}

/// Lazily claim writer authority — only when a draft actually exists, so a
/// read-only conversation never takes the epoch. A cached identity is reused
/// only while it is still the current epoch: another process (a concurrent
/// turn, a reflection job) may have claimed since, and submitting under the
/// fenced epoch would reject the draft. Call with the writer lock held.
pub fn ensure_writer(
    runtime: &Runtime,
    cached: &mut Option<WriterIdentity>,
) -> Result<WriterIdentity, RuntimeError> {
    if let Some(writer) = cached
        && writer.writer_epoch == runtime.head()?.writer_epoch
    {
        return Ok(*writer);
    }
    let writer = runtime.claim_writer()?;
    *cached = Some(writer);
    Ok(writer)
}

/// Submit persona/reflector drafts to the Continuity Kernel. Each draft is
/// attributed here — head, writer, policy version — because those are the
/// parts a model is never allowed to decide. Idempotent per (session, turn,
/// draft index) key.
pub fn submit_drafts(
    runtime: &Runtime,
    session_id: Option<SessionId>,
    turn_id: Option<TurnId>,
    cached_writer: &mut Option<WriterIdentity>,
    drafts: Vec<ProposalDraft>,
) -> Result<Vec<DraftOutcome>, RuntimeError> {
    let mut outcomes = Vec::new();
    if drafts.is_empty() {
        return Ok(outcomes);
    }
    // Claim and submit as one step with respect to other processes.
    let _lock = runtime.lock_writer()?;
    for (index, draft) in drafts.into_iter().enumerate() {
        let writer = ensure_writer(runtime, cached_writer)?;
        let head = runtime.head()?;
        let proposal = MutationProposal::from_draft(
            draft,
            ProposalAttribution {
                proposal_id: ProposalId::generate(runtime.ids().as_ref()),
                individual_id: runtime.individual_id(),
                expected_head: head.expected(),
                requested_by: writer,
                policy_version: MutationPolicyV0::VERSION,
                idempotency_key: format!(
                    "{}/{}/draft-{index}",
                    session_id
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "no-session".to_owned()),
                    turn_id
                        .map(|t| t.to_string())
                        .unwrap_or_else(|| "no-turn".to_owned()),
                ),
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
        let activated = outcome.receipt().is_some();
        let reason_code = match &outcome {
            kamimusuhi_core::continuity::ActivationOutcome::Activated(_)
            | kamimusuhi_core::continuity::ActivationOutcome::AlreadyActivated(_) => {
                "accepted".to_owned()
            }
            kamimusuhi_core::continuity::ActivationOutcome::Rejected(d) => {
                d.reason_code.as_str().to_owned()
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
        outcomes.push(DraftOutcome {
            proposal_id: proposal.proposal_id,
            activated,
            reason_code,
        });
    }
    Ok(outcomes)
}

// ---------------------------------------------------------------------------
// Canonical narration for derived-lane transitions
// ---------------------------------------------------------------------------

/// Append a canonical evidence record narrating a derived-lane event. The
/// lane's own tables hold the state; this is the immutable log that says it
/// happened. `links` attach lineage as (target, relation) pairs — the link's
/// `from` is always the new record itself.
#[allow(clippy::too_many_arguments)]
pub fn narrate(
    runtime: &Runtime,
    session_id: Option<SessionId>,
    turn_id: Option<TurnId>,
    kind: EvidenceKind,
    origin_class: OriginClass,
    payload: serde_json::Value,
    source_id: &str,
    links: &[(EvidenceId, kamimusuhi_core::evidence::EvidenceRelation)],
) -> Result<EvidenceId, RuntimeError> {
    let evidence_id = EvidenceId::generate(runtime.ids().as_ref());
    let digest = kamimusuhi_core::digest::json_digest(&payload);
    runtime.store().append(NewEvidence {
        evidence_id,
        individual_id: runtime.individual_id(),
        session_id,
        turn_id,
        kind,
        origin_class,
        payload,
        source: EvidenceSource {
            source_id: Some(source_id.to_owned()),
            source_sequence: None,
            content_digest: Some(digest.clone()),
        },
        retention_class: RetentionClass::Standard,
    })?;
    for (to, relation) in links {
        runtime.store().link(NewEvidenceLink {
            from_evidence_id: evidence_id,
            to_evidence_id: *to,
            relation: *relation,
        })?;
    }
    runtime.trace().record_with(
        TraceEventKind::EvidenceRecorded,
        TraceCorrelation {
            evidence_id: Some(evidence_id),
            ..TraceCorrelation::default()
        },
        serde_json::json!({"kind": kind, "content_digest": digest}),
    );
    Ok(evidence_id)
}

// ---------------------------------------------------------------------------
// Proposal lifecycle in the derived lane
// ---------------------------------------------------------------------------

/// Serializable proposal view for command output.
#[derive(Debug, Clone, Serialize)]
pub struct ProposalReport {
    pub proposal_id: C0ProposalId,
    pub kind: ImprovementKind,
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_key: Option<String>,
    pub proposed_value: serde_json::Value,
    pub status: ProposalStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activation_seq: Option<u64>,
}

impl From<&ImprovementProposal> for ProposalReport {
    fn from(p: &ImprovementProposal) -> Self {
        Self {
            proposal_id: p.proposal_id,
            kind: p.kind,
            target: p.target.clone(),
            target_key: p.target_key.clone(),
            proposed_value: p.proposed_value.clone(),
            status: p.status,
            rejection_reason: p.rejection_reason.clone(),
            activation_seq: p.activation_seq,
        }
    }
}

/// Activate a pending proposal: apply it into a new activation and move the
/// head. The gate decision happens in the caller (`reflection`); by the time
/// this runs the proposal is *accepted*.
pub fn activate(
    runtime: &Runtime,
    proposal_id: C0ProposalId,
    session_id: Option<SessionId>,
    turn_id: Option<TurnId>,
) -> Result<Activation, RuntimeError> {
    let activation_id = C0ActivationId::generate(runtime.ids().as_ref());
    let activation =
        runtime
            .store()
            .c0_apply_activation(proposal_id, activation_id, runtime.now())?;
    narrate(
        runtime,
        session_id,
        turn_id,
        EvidenceKind::SystemEvent,
        OriginClass::Inferred,
        serde_json::json!({
            "event": "c0_activation_applied",
            "activation_seq": activation.activation_seq,
            "activation_id": activation_id,
            "proposal_id": proposal_id,
        }),
        "c0-derived-lane",
        &[],
    )?;
    runtime.trace().record_with(
        TraceEventKind::MutationDecided,
        TraceCorrelation::default(),
        serde_json::json!({
            "c0_activation_seq": activation.activation_seq,
            "proposal_id": proposal_id,
            "activated": true,
        }),
    );
    Ok(activation)
}

/// Move the derived-lane head back one activation. Canonical history is
/// untouched; the crossed activation keeps its row, marked rolled-back.
pub fn rollback(
    runtime: &Runtime,
    session_id: Option<SessionId>,
    turn_id: Option<TurnId>,
) -> Result<kamimusuhi_store_sqlite::c0::RollbackOutcome, RuntimeError> {
    let outcome = runtime
        .store()
        .c0_rollback(runtime.individual_id(), runtime.now())?;
    narrate(
        runtime,
        session_id,
        turn_id,
        EvidenceKind::SystemEvent,
        OriginClass::Observed,
        serde_json::json!({
            "event": "c0_rollback",
            "rolled_back_seq": outcome.rolled_back_seq,
            "restored_seq": outcome.restored_seq,
            "proposal_id": outcome.proposal_id,
        }),
        "c0-derived-lane",
        &[],
    )?;
    Ok(outcome)
}
