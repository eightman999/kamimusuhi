//! The reflection cycle: observe recent experience → drafts → intake →
//! replay-gated activation.
//!
//! Nothing here trusts model output. Reflector output is intake-validated
//! against exactly the IDs it was shown; canonical drafts take the kernel
//! path; derived-lane drafts become pending proposals that a deterministic
//! replay gate decides on.

use kamimusuhi_core::c0::{
    self, AllowedEvidence, ImprovementProposal, ProposalStatus, ReflectionEpisode, ReflectionInput,
    ReflectionMemoryRecord, ReflectionOutput, TurnMetrics, validate_improvement_draft,
};
use kamimusuhi_core::continuity::WriterIdentity;
use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceLookup, EvidenceRelation, EvidenceStore, NewEvidenceLink,
};
use kamimusuhi_core::ids::{C0ProposalId, EvidenceId, SessionId, TurnId};
use kamimusuhi_core::memory::{MemoryQuery, MemoryRepository};
use kamimusuhi_core::mutation::{MutationDomain, OriginClass};
use serde::Serialize;

use crate::c0::{self as lane, replay};
use crate::{Runtime, RuntimeError};

/// What one reflection cycle did — returned to the caller and narrated into
/// canonical evidence.
#[derive(Debug, Clone, Serialize)]
pub struct ReflectionReport {
    pub reflection_evidence_id: EvidenceId,
    pub reflector_backend: String,
    /// Whether the reflector was a distinct component from the actor.
    /// `false` means the same backend both spoke and reflected — a material
    /// fact an honest report does not hide.
    pub evaluator_distinct_from_actor: bool,
    pub episodes_presented: usize,
    pub notes: Vec<String>,
    pub corrections_linked: usize,
    pub canonical_drafts_submitted: usize,
    pub canonical_drafts_activated: usize,
    pub proposals_created: Vec<lane::ProposalReport>,
    /// Intake rejections, with reasons — dropped drafts are reported, not
    /// silently swallowed.
    pub drafts_rejected: Vec<serde_json::Value>,
    /// Per-proposal gate outcomes.
    pub gate_results: Vec<serde_json::Value>,
    /// Activation seqs applied this cycle.
    pub activations: Vec<u64>,
}

/// Run one reflection cycle.
///
/// `subject` scopes the memory records and replay inputs the cycle works
/// with; `session_id`/`turn_id` correlate the canonical narration when the
/// cycle runs inside a chat session.
pub fn run(
    runtime: &mut Runtime,
    subject: &str,
    session_id: Option<SessionId>,
    turn_id: Option<TurnId>,
    writer_cache: &mut Option<WriterIdentity>,
) -> Result<ReflectionReport, RuntimeError> {
    let individual = runtime.individual_id();

    // --- gather what the reflector may cite --------------------------------
    let operative = lane::operative(runtime)?;
    let pool = runtime.store().c0_evidence_pool(individual, 64)?;
    // Utterances, corrections and system events are episodes; reflections
    // themselves are excluded so a cycle cannot recursively cite its own
    // narration.
    let episodes: Vec<_> = pool
        .iter()
        .filter(|r| {
            matches!(
                r.kind,
                EvidenceKind::UserUtterance
                    | EvidenceKind::AgentUtterance
                    | EvidenceKind::Correction
                    | EvidenceKind::SystemEvent
            )
        })
        .take(32)
        .cloned()
        .collect();
    let episode_ids: Vec<EvidenceId> = episodes.iter().map(|e| e.evidence_id).collect();
    let facts = runtime.store().facts(&episode_ids)?;
    let presented: Vec<ReflectionEpisode> = episodes
        .iter()
        .map(|record| {
            let text = record
                .payload
                .get("text")
                .and_then(|t| t.as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    record
                        .payload
                        .get("event")
                        .and_then(|t| t.as_str())
                        .unwrap_or("(event)")
                        .to_owned()
                });
            let speaker = match record.kind {
                EvidenceKind::UserUtterance => "user",
                EvidenceKind::AgentUtterance => "assistant",
                _ => "system",
            };
            ReflectionEpisode {
                evidence_id: record.evidence_id,
                kind: record.kind,
                speaker: speaker.to_owned(),
                text,
                is_corrected: facts
                    .get(record.evidence_id)
                    .is_some_and(|f| f.is_corrected),
                created_at: record.created_at,
            }
        })
        .collect();

    let mut memory_records = Vec::new();
    for domain in [MutationDomain::Relationship, MutationDomain::Episodic] {
        for attributed in MemoryRepository::retrieve(
            runtime.store(),
            &MemoryQuery::current(individual)
                .in_domain(domain)
                .about(subject)
                .limited(32),
        )? {
            memory_records.push(ReflectionMemoryRecord {
                state_record_id: attributed.record.state_record_id,
                domain: attributed.record.domain,
                subject_key: attributed.record.subject_key.clone(),
                kind: attributed.record.kind.clone(),
                payload: attributed.record.payload.clone(),
                evidence_refs: attributed.record.evidence_refs.clone(),
            });
        }
    }

    let metrics: Vec<TurnMetrics> = runtime
        .store()
        .c0_evaluations(individual, Some("turn"), 12)?
        .into_iter()
        .filter_map(|e| serde_json::from_value::<TurnMetrics>(e.metrics).ok())
        .collect();

    let pending = runtime
        .store()
        .c0_proposals(individual, Some(ProposalStatus::Pending))?;

    let input = ReflectionInput {
        individual_id: individual,
        subject_key: Some(subject.to_owned()),
        episodes: presented.clone(),
        memory_records,
        operative: operative.view.clone(),
        metrics,
        pending_proposals: pending,
        activation_seq: operative.activation_seq,
    };

    // --- reflect ------------------------------------------------------------
    let reflector = runtime.config().build_reflector()?;
    let descriptor = reflector.descriptor();
    let output: ReflectionOutput = reflector.reflect(&input)?;
    let actor_kind = runtime.config().persona.backend.as_str();
    let evaluator_distinct = descriptor.kind != actor_kind;

    // The manifest: which ids the output was allowed to cite, by kind.
    let allowed: AllowedEvidence = presented.iter().map(|e| (e.evidence_id, e.kind)).collect();

    // --- correction links ----------------------------------------------------
    let mut corrections_linked = 0usize;
    for hint in &output.evidence_corrections {
        let both_present =
            allowed.contains_key(&hint.correction) && allowed.contains_key(&hint.corrects);
        if !both_present || hint.correction == hint.corrects {
            continue;
        }
        runtime.store().link(NewEvidenceLink {
            from_evidence_id: hint.correction,
            to_evidence_id: hint.corrects,
            relation: EvidenceRelation::Corrects,
        })?;
        corrections_linked += 1;
    }

    // --- canonical drafts → kernel -------------------------------------------
    let draft_outcomes = lane::submit_drafts(
        runtime,
        session_id,
        turn_id,
        writer_cache,
        output.canonical_drafts.clone(),
    )?;
    let canonical_activated = draft_outcomes.iter().filter(|o| o.activated).count();

    // --- improvement drafts → pending proposals ------------------------------
    let mut proposals_created = Vec::new();
    let mut drafts_rejected = Vec::new();
    for draft in &output.improvement_drafts {
        match validate_improvement_draft(draft, &allowed, &operative.view) {
            Ok((kind, target, target_key, old_value)) => {
                let proposal = ImprovementProposal {
                    proposal_id: C0ProposalId::generate(runtime.ids().as_ref()),
                    individual_id: individual,
                    kind,
                    target,
                    target_key,
                    old_value: if old_value.is_null() {
                        None
                    } else {
                        Some(old_value)
                    },
                    proposed_value: draft.proposed_value.clone(),
                    evidence_refs: draft.evidence_refs.clone(),
                    expected_effect: draft.expected_effect.clone(),
                    risk: draft.risk.clone(),
                    confidence: draft.confidence,
                    status: ProposalStatus::Pending,
                    rejection_reason: None,
                    reflection_id: None, // set below once the evidence exists
                    created_at: runtime.now(),
                    decided_at: None,
                    activation_seq: None,
                };
                proposals_created.push(proposal);
            }
            Err(rejection) => drafts_rejected.push(serde_json::json!({
                "target": rejection.target,
                "reason": rejection.reason,
            })),
        }
    }

    // --- narrate the cycle into canonical evidence ----------------------------
    let cited: Vec<(EvidenceId, EvidenceRelation)> = presented
        .iter()
        .take(16)
        .map(|e| (e.evidence_id, EvidenceRelation::DerivedFrom))
        .collect();
    let reflection_evidence_id = lane::narrate(
        runtime,
        session_id,
        turn_id,
        EvidenceKind::Reflection,
        OriginClass::Inferred,
        serde_json::json!({
            "event": "reflection",
            "reflector": descriptor,
            "evaluator_distinct_from_actor": evaluator_distinct,
            "episodes_presented": presented.len(),
            "notes": output.notes,
            "corrections_linked": corrections_linked,
            "canonical_drafts": draft_outcomes.len(),
            "proposals": proposals_created
                .iter()
                .map(|p| serde_json::json!({
                    "proposal_id": p.proposal_id,
                    "kind": p.kind,
                    "target": p.target,
                }))
                .collect::<Vec<_>>(),
            "drafts_rejected": drafts_rejected,
        }),
        "c0-reflection",
        &cited,
    )?;

    // Persist proposals now that the reflection evidence id exists.
    for proposal in &mut proposals_created {
        proposal.reflection_id = Some(reflection_evidence_id);
        runtime.store().c0_insert_proposal(proposal)?;
        lane::narrate(
            runtime,
            session_id,
            turn_id,
            EvidenceKind::SystemEvent,
            OriginClass::Inferred,
            serde_json::json!({
                "event": "improvement_proposal",
                "proposal_id": proposal.proposal_id,
                "kind": proposal.kind,
                "target": proposal.target,
                "reflection_id": reflection_evidence_id,
            }),
            "c0-reflection",
            &[],
        )?;
    }

    // --- replay + gate + activate ---------------------------------------------
    let mut gate_results = Vec::new();
    let mut activations = Vec::new();
    if runtime.config().c0.auto_activate {
        for proposal in &proposals_created {
            let report = replay::replay_proposal(runtime, subject, proposal)?;
            let decision = c0::gate_decision(proposal, &report);
            let accepted = matches!(decision, c0::GateDecision::Accept);
            runtime
                .store()
                .c0_record_evaluation(&kamimusuhi_store_sqlite::c0::C0Evaluation {
                    evaluation_id: kamimusuhi_core::ids::C0EvaluationId::generate(
                        runtime.ids().as_ref(),
                    ),
                    individual_id: individual,
                    scope: "replay".to_owned(),
                    subject_key: Some(subject.to_owned()),
                    turn_id: None,
                    proposal_id: Some(proposal.proposal_id),
                    metrics: serde_json::json!({
                        "replay": report,
                        "gate": if accepted { "accept" } else { "reject" },
                    }),
                    evaluator: c0::EVALUATOR_KIND.to_owned(),
                    created_at: runtime.now(),
                })?;
            match &decision {
                c0::GateDecision::Accept => {
                    let activation =
                        lane::activate(runtime, proposal.proposal_id, session_id, turn_id)?;
                    gate_results.push(serde_json::json!({
                        "proposal_id": proposal.proposal_id,
                        "gate": "accept",
                        "activation_seq": activation.activation_seq,
                    }));
                    activations.push(activation.activation_seq);
                }
                c0::GateDecision::Reject { reason } => {
                    runtime.store().c0_decide(
                        proposal.proposal_id,
                        ProposalStatus::Rejected,
                        Some(reason),
                        runtime.now(),
                    )?;
                    gate_results.push(serde_json::json!({
                        "proposal_id": proposal.proposal_id,
                        "gate": "reject",
                        "reason": reason,
                    }));
                }
            }
        }
    }

    Ok(ReflectionReport {
        reflection_evidence_id,
        reflector_backend: format!("{}:{}", descriptor.kind, descriptor.name),
        evaluator_distinct_from_actor: evaluator_distinct,
        episodes_presented: presented.len(),
        notes: output.notes,
        corrections_linked,
        canonical_drafts_submitted: draft_outcomes.len(),
        canonical_drafts_activated: canonical_activated,
        proposals_created: proposals_created
            .iter()
            .map(lane::ProposalReport::from)
            .collect(),
        drafts_rejected,
        gate_results,
        activations,
    })
}
