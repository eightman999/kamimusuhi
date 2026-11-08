//! The reflection cycle: observe recent experience → drafts → intake →
//! replay-gated activation.
//!
//! Nothing here trusts model output. Reflector output is intake-validated
//! against exactly the IDs it was shown; canonical drafts take the kernel
//! path; derived-lane drafts become pending proposals that a deterministic
//! replay gate decides on.

use std::collections::BTreeSet;

use kamimusuhi_core::c0::{
    self, AllowedEvidence, ImprovementProposal, ProposalStatus, ReflectionEpisode, ReflectionInput,
    ReflectionMemoryRecord, ReflectionOutput, Reflector, TurnMetrics, validate_improvement_draft,
};
use kamimusuhi_core::continuity::WriterIdentity;
use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceLookup, EvidenceRecord, EvidenceRelation, EvidenceStore, NewEvidenceLink,
};
use kamimusuhi_core::ids::{C0ProposalId, EvidenceId, MemoryId, SessionId, TurnId};
use kamimusuhi_core::memory::{MemoryLink, MemoryQuery, MemoryRepository};
use kamimusuhi_core::mutation::{MutationDomain, OriginClass};
use kamimusuhi_core::persona::ProposalDraft;
use kamimusuhi_core::routing::PrivacyConstraint;
use kamimusuhi_core::time::UtcTimestamp;
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
    let reflector = runtime.config().build_reflector()?;
    run_with_reflector(
        runtime,
        subject,
        session_id,
        turn_id,
        writer_cache,
        reflector.as_ref(),
    )
}

/// Recent delivered dialogue from this subject only, in chronological order.
/// Limit after selecting the source so other subjects cannot crowd it out.
pub fn source_episodes(
    runtime: &Runtime,
    subject: &str,
) -> Result<Vec<EvidenceRecord>, RuntimeError> {
    let episodes = runtime.store().recent_conversation(
        runtime.individual_id(),
        &format!("text-chat:{subject}"),
        128,
    )?;
    let start = episodes.len().saturating_sub(32);
    Ok(episodes.into_iter().skip(start).collect())
}

/// One standalone reflection under an explicit data boundary. The chosen
/// reflector's declaration is checked before an input is sent to an API.
pub fn run_with_privacy(
    runtime: &mut Runtime,
    subject: &str,
    privacy: PrivacyConstraint,
) -> Result<ReflectionReport, RuntimeError> {
    runtime.config().check_reflector_privacy(privacy)?;
    if runtime.config().c0.auto_activate {
        runtime.config().persona.check_privacy(privacy)?;
    }
    let mut writer = None;
    run(runtime, subject, None, None, &mut writer)
}

/// [`run`] with an explicit reflector. Everything past backend construction
/// is identical — the seam exists so tests can drive the cycle with a
/// fixture carrying declared metadata.
pub fn run_with_reflector(
    runtime: &mut Runtime,
    subject: &str,
    session_id: Option<SessionId>,
    turn_id: Option<TurnId>,
    writer_cache: &mut Option<WriterIdentity>,
    reflector: &dyn Reflector,
) -> Result<ReflectionReport, RuntimeError> {
    let individual = runtime.individual_id();

    // --- gather what the reflector may cite --------------------------------
    let operative = lane::operative(runtime)?;
    let episodes = source_episodes(runtime, subject)?;
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
        .c0_evaluations_for_subject(individual, Some("turn"), subject, 12)?
        .into_iter()
        .filter_map(|e| serde_json::from_value::<TurnMetrics>(e.metrics).ok())
        .collect();

    let source_id = format!("text-chat:{subject}");
    let mut pending = Vec::new();
    for proposal in runtime
        .store()
        .c0_proposals(individual, Some(ProposalStatus::Pending))?
    {
        let mut scoped = !proposal.evidence_refs.is_empty();
        for id in &proposal.evidence_refs {
            if !runtime.store().get(*id)?.is_some_and(|record| {
                record.source.source_id.as_deref() == Some(source_id.as_str())
            }) {
                scoped = false;
                break;
            }
        }
        if scoped {
            pending.push(proposal);
        }
    }

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
    let _identity_lease = runtime.lock_identity()?;
    runtime.ensure_identity()?;
    let manifest = runtime.identity_manifest()?;
    let mut identity_context = runtime.identity_context().clone();
    if identity_context.is_null() {
        identity_context = serde_json::json!({});
    }
    identity_context["model_hashes"] = serde_json::json!(manifest.allowed_model_hashes);
    reflector.set_identity_context(identity_context);
    let descriptor = reflector.descriptor();
    let mut output: ReflectionOutput = reflector.reflect(&input)?;
    (|| -> Result<ReflectionReport, RuntimeError> {
        // Configuration changes while the API was running must precede every
        // correction link, canonical draft and derived-lane write below.
        runtime.ensure_identity()?;
        let actor_kind = runtime.config().persona.backend.as_str();
        let evaluator_distinct = descriptor.kind != actor_kind;

        // This reflection was supplied one person's conversation. A model cannot
        // redirect person-scoped memory into somebody else's or a global record.
        // Self-model drafts retain their existing individual-level intake rules.
        let mut drafts_rejected = Vec::new();
        output.canonical_drafts.retain(|draft| {
            let person_scoped = matches!(
                draft.domain,
                MutationDomain::Relationship | MutationDomain::Episodic
            );
            if person_scoped && draft.subject_key.as_deref() != Some(subject) {
                drafts_rejected.push(serde_json::json!({
                    "domain": draft.domain.as_str(),
                    "operation": draft.operation.as_str(),
                    "subject_key": draft.subject_key,
                    "reason": "reflection_subject_mismatch",
                }));
                false
            } else {
                true
            }
        });

        // The manifest: which ids the output was allowed to cite, by kind.
        let allowed: AllowedEvidence = presented.iter().map(|e| (e.evidence_id, e.kind)).collect();
        // Declared recall metadata is model output riding inside `candidate`:
        // normalize it against the manifest before it becomes payload.
        sanitize_declared_metadata(
            &mut output.canonical_drafts,
            &allowed,
            &input.memory_records,
        );

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
        let cycle_scope = format!(
            "reflection/{}",
            C0ProposalId::generate(runtime.ids().as_ref())
        );
        let draft_outcomes = lane::submit_reflection_drafts(
            runtime,
            session_id,
            turn_id,
            writer_cache,
            output.canonical_drafts.clone(),
            &cycle_scope,
        )?;
        let canonical_activated = draft_outcomes.iter().filter(|o| o.activated).count();

        // --- improvement drafts → pending proposals ------------------------------
        let mut proposals_created = Vec::new();
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
                runtime.store().c0_record_evaluation(
                    &kamimusuhi_store_sqlite::c0::C0Evaluation {
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
                    },
                )?;
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
    })()
    .map_err(|error| RuntimeError::ReflectionAfterIntake(Box::new(error)))
}

// ---------------------------------------------------------------------------
// Declared recall metadata (producer side of recall v0)
// ---------------------------------------------------------------------------

/// The most tags a draft may declare — the cap the prompt states. Extras
/// are dropped, not the draft.
const MAX_DECLARED_TAGS: usize = 8;

/// Fail-soft cleanup of producer-declared recall metadata on canonical
/// drafts.
///
/// A draft may carry `tags`/`links`/`valid_from`/`valid_to` inside
/// `candidate`; they land verbatim in the state record's payload where
/// `StateRecord::{tags,links,valid_from,valid_to}` read them. Everything
/// here is model output, so intake normalizes it the same way the rest of
/// the reflector contract does: entries are kept only when they are
/// well-formed *and* `evidence:`/`state:` links name ids the reflector was
/// actually shown. A malformed field is dropped — never the draft.
fn sanitize_declared_metadata(
    drafts: &mut [ProposalDraft],
    allowed: &AllowedEvidence,
    presented_records: &[ReflectionMemoryRecord],
) {
    let presented_states: BTreeSet<MemoryId> = presented_records
        .iter()
        .map(|record| record.state_record_id)
        .collect();
    crate::faithfulness::reject_unpresented_references(drafts, &allowed.keys().copied().collect());
    crate::faithfulness::reject_unpresented_links(
        drafts,
        &allowed.keys().copied().collect(),
        &presented_states,
    );
    for draft in drafts {
        let invalid_link = draft.candidate.get("links").is_some_and(|links| {
            links.as_array().is_none_or(|links| {
                links
                    .iter()
                    .any(|value| match value.as_str().and_then(MemoryLink::parse) {
                        Some(MemoryLink::Evidence(id)) => !allowed.contains_key(&id),
                        Some(MemoryLink::State(id)) => !presented_states.contains(&id),
                        None => true,
                    })
            })
        });
        if invalid_link {
            crate::faithfulness::reject_intake(draft);
        }
        let invalid_time = ["valid_from", "valid_to"].iter().any(|key| {
            draft.candidate.get(key).is_some_and(|value| {
                !(value.as_i64().is_some()
                    || value
                        .as_str()
                        .is_some_and(|text| UtcTimestamp::from_rfc3339(text).is_some()))
            })
        });
        if invalid_time {
            crate::faithfulness::reject_time_intake(draft);
        }
        let Some(candidate) = draft.candidate.as_object_mut() else {
            continue;
        };
        // tags: a deduplicated string list, capped at MAX_DECLARED_TAGS.
        let tags = candidate.get("tags").and_then(|value| {
            let tags: Vec<serde_json::Value> = value
                .as_array()?
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .take(MAX_DECLARED_TAGS)
                .map(serde_json::Value::String)
                .collect();
            (!tags.is_empty()).then_some(serde_json::Value::Array(tags))
        });
        if let Some(tags) = tags {
            candidate.insert("tags".to_owned(), tags);
        } else {
            candidate.remove("tags");
        }
        // links: typed refs to exactly the ids the input presented —
        // evidence ids from the episode list, state ids from the memory
        // records. Anything else is a dead end dropped at the boundary.
        let links = candidate.get("links").and_then(|value| {
            let links: Vec<serde_json::Value> = value
                .as_array()?
                .iter()
                .filter_map(serde_json::Value::as_str)
                .filter_map(MemoryLink::parse)
                .filter(|link| match link {
                    MemoryLink::Evidence(id) => allowed.contains_key(id),
                    MemoryLink::State(id) => presented_states.contains(id),
                })
                .map(|link| link.as_str())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .map(serde_json::Value::String)
                .collect();
            (!links.is_empty()).then_some(serde_json::Value::Array(links))
        });
        if let Some(links) = links {
            candidate.insert("links".to_owned(), links);
        } else {
            candidate.remove("links");
        }
        // valid_from/valid_to: keep only the shapes the StateRecord readers
        // accept — unix seconds (integer) or RFC 3339 text.
        for key in ["valid_from", "valid_to"] {
            let usable = candidate.get(key).is_some_and(|value| {
                value.as_i64().is_some()
                    || value
                        .as_str()
                        .is_some_and(|text| UtcTimestamp::from_rfc3339(text).is_some())
            });
            if !usable {
                candidate.remove(key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use kamimusuhi_core::ids::{C0EvaluationId, PersonaBackendId};
    use kamimusuhi_core::mutation::MutationOperation;
    use kamimusuhi_core::persona::{PersonaBackendDescriptor, PersonaError};
    use serde_json::json;

    use super::*;

    #[derive(Default)]
    struct ObservingReflector {
        input: Mutex<Option<ReflectionInput>>,
        identity: Mutex<serde_json::Value>,
        capture_latest: bool,
        additional_drafts: Vec<ProposalDraft>,
        additional_improvements: Vec<c0::ImprovementDraft>,
        fail_before_intake: bool,
    }

    impl Reflector for ObservingReflector {
        fn set_identity_context(&self, context: serde_json::Value) {
            *self.identity.lock().unwrap() = context;
        }

        fn descriptor(&self) -> PersonaBackendDescriptor {
            PersonaBackendDescriptor {
                backend_id: PersonaBackendId::from_u128(0x1D1E),
                kind: "fixture".to_owned(),
                name: "observing-reflector".to_owned(),
                version: "1".to_owned(),
            }
        }

        fn reflect(&self, input: &ReflectionInput) -> Result<ReflectionOutput, PersonaError> {
            if self.fail_before_intake {
                return Err(PersonaError::Transport {
                    backend_id: self.descriptor().backend_id,
                    message: "fixture transport failure".to_owned(),
                });
            }
            *self.input.lock().unwrap() = Some(input.clone());
            let mut output = ReflectionOutput::default();
            if self.capture_latest {
                let episode = input.episodes.last().unwrap();
                output.canonical_drafts.push(ProposalDraft {
                    domain: MutationDomain::Episodic,
                    operation: MutationOperation::Capture,
                    subject_key: input.subject_key.clone(),
                    candidate: json!({"summary": episode.text}),
                    evidence_refs: vec![episode.evidence_id],
                    supersedes: None,
                    origin_class: OriginClass::Reported,
                });
            }
            output
                .canonical_drafts
                .extend(self.additional_drafts.clone());
            output
                .improvement_drafts
                .extend(self.additional_improvements.clone());
            Ok(output)
        }
    }

    fn runtime() -> (tempfile::TempDir, Runtime) {
        let dir = tempfile::tempdir().unwrap();
        let runtime = Runtime::init(
            dir.path(),
            crate::RuntimeOptions::deterministic(0x1D1E),
            crate::ResourceImplementation::FakeA,
        )
        .unwrap();
        (dir, runtime)
    }

    fn utterance(runtime: &Runtime, subject: &str, text: &str) -> EvidenceId {
        lane::narrate(
            runtime,
            None,
            None,
            EvidenceKind::UserUtterance,
            OriginClass::Reported,
            json!({"text": text}),
            &format!("text-chat:{subject}"),
            &[],
        )
        .unwrap()
    }

    fn metric(runtime: &Runtime, subject: &str, response_chars: usize) {
        runtime
            .store()
            .c0_record_evaluation(&kamimusuhi_store_sqlite::c0::C0Evaluation {
                evaluation_id: C0EvaluationId::generate(runtime.ids().as_ref()),
                individual_id: runtime.individual_id(),
                scope: "turn".to_owned(),
                subject_key: Some(subject.to_owned()),
                turn_id: None,
                proposal_id: None,
                metrics: serde_json::to_value(TurnMetrics {
                    response_chars,
                    ..Default::default()
                })
                .unwrap(),
                evaluator: "fixture".to_owned(),
                created_at: runtime.now(),
            })
            .unwrap();
    }

    #[test]
    fn standalone_reflection_sources_and_metrics_are_subject_scoped_before_limits() {
        let (_dir, mut runtime) = runtime();
        for index in 0..40 {
            utterance(&runtime, "alice", &format!("alice-{index}"));
        }
        metric(&runtime, "alice", 12);
        for index in 0..80 {
            utterance(&runtime, "bob", &format!("bob-private-{index}"));
            metric(&runtime, "bob", 999);
        }
        let episodes = source_episodes(&runtime, "alice").unwrap();
        assert_eq!(episodes.len(), 32);
        assert_eq!(episodes.first().unwrap().payload["text"], "alice-8");
        assert_eq!(episodes.last().unwrap().payload["text"], "alice-39");
        let reflector = ObservingReflector::default();
        run_with_reflector(&mut runtime, "alice", None, None, &mut None, &reflector).unwrap();
        let input = reflector.input.lock().unwrap();
        let input = input.as_ref().unwrap();
        assert_eq!(input.episodes.len(), 32);
        assert!(
            input
                .episodes
                .iter()
                .all(|episode| !episode.text.contains("bob-private"))
        );
        assert_eq!(input.metrics.len(), 1);
        assert_eq!(input.metrics[0].response_chars, 12);
        assert!(reflector.identity.lock().unwrap()["model_hashes"].is_array());
    }

    #[test]
    fn standalone_cycles_have_independent_canonical_draft_keys() {
        let (_dir, mut runtime) = runtime();
        let reflector = ObservingReflector {
            capture_latest: true,
            ..Default::default()
        };
        let generation = runtime.head().unwrap().generation.0;
        utterance(&runtime, "alice", "first source text");
        let first =
            run_with_reflector(&mut runtime, "alice", None, None, &mut None, &reflector).unwrap();
        assert_eq!(first.canonical_drafts_activated, 1);
        utterance(&runtime, "alice", "second source text");
        let second =
            run_with_reflector(&mut runtime, "alice", None, None, &mut None, &reflector).unwrap();
        assert_eq!(second.canonical_drafts_activated, 1);
        assert_eq!(runtime.head().unwrap().generation.0, generation + 2);
    }

    #[test]
    fn reflection_refuses_other_subject_drafts_without_rewriting_self_model_scope() {
        let (_dir, mut runtime) = runtime();
        let evidence = utterance(&runtime, "alice", "Alice's source text");
        let individual_key = runtime.individual_id().to_string();
        let make_draft = |domain, subject: Option<String>| ProposalDraft {
            domain,
            operation: if domain == MutationDomain::Episodic {
                MutationOperation::Capture
            } else {
                MutationOperation::Fact
            },
            subject_key: subject,
            candidate: json!({"summary": "Alice's source text"}),
            evidence_refs: vec![evidence],
            supersedes: None,
            origin_class: OriginClass::Reported,
        };
        let reflector = ObservingReflector {
            capture_latest: true,
            additional_drafts: vec![
                make_draft(MutationDomain::Relationship, Some("bob".to_owned())),
                make_draft(MutationDomain::Episodic, Some("bob".to_owned())),
                make_draft(MutationDomain::Episodic, None),
                make_draft(MutationDomain::SelfModel, Some(individual_key.clone())),
            ],
            ..Default::default()
        };
        let report =
            run_with_reflector(&mut runtime, "alice", None, None, &mut None, &reflector).unwrap();
        assert_eq!(report.drafts_rejected.len(), 3);
        assert!(
            report
                .drafts_rejected
                .iter()
                .all(|rejection| rejection["reason"] == "reflection_subject_mismatch")
        );
        // Alice's capture and the individual-level draft reach their existing
        // domain intake. The latter remains reserved/rejected by that policy.
        assert_eq!(report.canonical_drafts_submitted, 2);
        assert_eq!(report.canonical_drafts_activated, 1);
        assert_eq!(
            reflector.additional_drafts[0].subject_key.as_deref(),
            Some("bob")
        );
        assert_eq!(
            reflector.additional_drafts[3].subject_key.as_deref(),
            Some(individual_key.as_str())
        );
        for domain in [MutationDomain::Relationship, MutationDomain::Episodic] {
            assert!(
                MemoryRepository::retrieve(
                    runtime.store(),
                    &MemoryQuery::current(runtime.individual_id())
                        .in_domain(domain)
                        .about("bob"),
                )
                .unwrap()
                .is_empty()
            );
        }
    }

    #[test]
    fn standalone_privacy_checks_the_selected_reflector_before_an_api_call() {
        use crate::config::{PersonaProviderConfig, ReasoningSetting, ReflectorBackend};
        use kamimusuhi_core::routing::LocalityClass;
        use kamimusuhi_testkit::http_fixture::{FixtureResponse, FixtureServer};

        let server = FixtureServer::always(FixtureResponse::ok("{}")).unwrap();
        for locality in [LocalityClass::LocalNetwork, LocalityClass::External] {
            let (_dir, mut runtime) = runtime();
            let mut config = runtime.config().clone();
            config.c0.reflector.backend = ReflectorBackend::OpenaiCompatible;
            config.c0.reflector.provider = Some(PersonaProviderConfig {
                backend_id: PersonaBackendId::from_u128(0x1D1E),
                locality,
                base_url: server.base_url(),
                model: "reflection-fixture".to_owned(),
                auth_env: None,
                timeout_ms: 1000,
                tls_root_ca_path: None,
                system_instruction: None,
                reasoning: ReasoningSetting::Off,
                extra_body: None,
            });
            runtime.save_config(config).unwrap();
            assert!(matches!(
                run_with_privacy(&mut runtime, "alice", PrivacyConstraint::LocalOnly),
                Err(RuntimeError::PersonaPrivacy { .. })
            ));
        }
        assert_eq!(server.request_count(), 0);
    }

    #[test]
    fn standalone_auto_activation_checks_replay_persona_privacy_before_reflection() {
        use crate::config::{PersonaBackendKind, ReflectorBackend};
        use kamimusuhi_core::routing::LocalityClass;
        use kamimusuhi_testkit::http_fixture::{FixtureResponse, FixtureServer};

        let server = FixtureServer::always(FixtureResponse::ok("{}")).unwrap();
        let (_dir, mut runtime) = runtime();
        let mut config = runtime.config().clone();
        config.c0.reflector.backend = ReflectorBackend::Fake;
        config.persona.backend = PersonaBackendKind::OpenaiCompatible;
        config.persona.provider = Some(
            serde_json::from_value(json!({
                "backend_id": PersonaBackendId::from_u128(0x1D1F),
                "locality": LocalityClass::External,
                "base_url": server.base_url(), "model": "replay-fixture", "timeout_ms": 1000,
            }))
            .unwrap(),
        );
        runtime.save_config(config.clone()).unwrap();
        assert!(matches!(
            run_with_privacy(&mut runtime, "alice", PrivacyConstraint::LocalOnly),
            Err(RuntimeError::PersonaPrivacy { .. })
        ));
        config.c0.auto_activate = false;
        runtime.save_config(config).unwrap();
        let status = runtime.identity_status().unwrap();
        runtime.accept_identity(&status.review_token).unwrap();
        run_with_privacy(&mut runtime, "alice", PrivacyConstraint::LocalOnly).unwrap();
        assert_eq!(server.request_count(), 0);
    }

    #[test]
    fn reflection_and_later_replay_failures_have_different_retry_boundaries() {
        use crate::config::PersonaBackendKind;
        use kamimusuhi_core::c0::{ImprovementDraft, ImprovementKind};
        use kamimusuhi_core::routing::LocalityClass;
        use kamimusuhi_testkit::http_fixture::{FixtureResponse, FixtureServer};

        let server = FixtureServer::always(FixtureResponse::Status { code: 503 }).unwrap();
        let (_dir, mut runtime) = runtime();
        let evidence = utterance(&runtime, "alice", "source text before replay failure");
        let early = ObservingReflector {
            fail_before_intake: true,
            ..Default::default()
        };
        assert!(matches!(
            run_with_reflector(&mut runtime, "alice", None, None, &mut None, &early),
            Err(RuntimeError::Persona(PersonaError::Transport { .. }))
        ));
        let mut config = runtime.config().clone();
        config.persona.backend = PersonaBackendKind::OpenaiCompatible;
        config.persona.provider = Some(
            serde_json::from_value(json!({
                "backend_id": PersonaBackendId::from_u128(0x1D1F),
                "locality": LocalityClass::LocalHost,
                "base_url": server.base_url(), "model": "replay-fixture", "timeout_ms": 1000,
            }))
            .unwrap(),
        );
        runtime.save_config(config).unwrap();
        let status = runtime.identity_status().unwrap();
        runtime.accept_identity(&status.review_token).unwrap();
        let generation = runtime.head().unwrap().generation.0;
        let later = ObservingReflector {
            capture_latest: true,
            additional_improvements: vec![ImprovementDraft {
                kind: ImprovementKind::PolicyUpdate,
                target: "conversation.max_response_chars".to_owned(),
                target_key: None,
                proposed_value: json!(200),
                evidence_refs: vec![evidence],
                expected_effect: None,
                risk: None,
                confidence: None,
            }],
            ..Default::default()
        };
        let error =
            run_with_reflector(&mut runtime, "alice", None, None, &mut None, &later).unwrap_err();
        assert!(
            matches!(error, RuntimeError::ReflectionAfterIntake(inner) if matches!(*inner, RuntimeError::Persona(PersonaError::HttpStatus { status: 503, .. })))
        );
        assert_eq!(runtime.head().unwrap().generation.0, generation + 1);
        assert_eq!(server.request_count(), 1);
    }

    fn draft(candidate: serde_json::Value) -> ProposalDraft {
        ProposalDraft {
            domain: MutationDomain::Episodic,
            operation: MutationOperation::Capture,
            subject_key: Some("alice".to_owned()),
            candidate,
            evidence_refs: vec![],
            supersedes: None,
            origin_class: OriginClass::Reported,
        }
    }

    fn presented(
        evidence: &[EvidenceId],
        states: &[MemoryId],
    ) -> (AllowedEvidence, Vec<ReflectionMemoryRecord>) {
        (
            evidence
                .iter()
                .map(|id| (*id, EvidenceKind::UserUtterance))
                .collect(),
            states
                .iter()
                .map(|id| ReflectionMemoryRecord {
                    state_record_id: *id,
                    domain: MutationDomain::Episodic,
                    subject_key: Some("alice".to_owned()),
                    kind: "episode".to_owned(),
                    payload: serde_json::json!({}),
                    evidence_refs: vec![],
                })
                .collect(),
        )
    }

    #[test]
    fn declared_metadata_is_normalized_against_what_was_presented() {
        let evidence = EvidenceId::from_u128(0xE1);
        let state = MemoryId::from_u128(0x51);
        let (allowed, records) = presented(&[evidence], &[state]);
        let mut drafts = vec![draft(json!({
            "summary": "two utterances",
            "tags": ["tea", 42, "preference", "tea"],
            "links": [
                format!("evidence:{evidence}"),
                format!("state:{state}"),
                "nonsense",
                // Well-formed but never presented: dropped at intake.
                format!("state:{}", MemoryId::from_u128(0xFF)),
            ],
            "valid_from": 1_700_000_000,
            "valid_to": "2027-01-01T00:00:00Z",
        }))];
        sanitize_declared_metadata(&mut drafts, &allowed, &records);
        let candidate = &drafts[0].candidate;
        assert_eq!(candidate["tags"], json!(["preference", "tea"]));
        assert_eq!(
            candidate["links"],
            json!([format!("evidence:{evidence}"), format!("state:{state}")])
        );
        assert_eq!(candidate["valid_from"], json!(1_700_000_000));
        assert_eq!(candidate["valid_to"], json!("2027-01-01T00:00:00Z"));
        assert_eq!(candidate["summary"], json!("two utterances"));
    }

    #[test]
    fn malformed_declared_fields_retain_a_whole_item_refusal() {
        let (allowed, records) = presented(&[], &[]);
        let mut drafts = vec![draft(json!({
            "summary": "two utterances",
            "tags": "not-an-array",
            "links": [42, "state:zz"],
            "valid_from": "not-a-date",
            "valid_to": 1.5,
        }))];
        sanitize_declared_metadata(&mut drafts, &allowed, &records);
        let candidate = &drafts[0].candidate;
        for key in ["tags", "links", "valid_from", "valid_to"] {
            assert!(candidate.get(key).is_none(), "{key} must be dropped");
        }
        assert_eq!(candidate["summary"], json!("two utterances"));
        assert_eq!(candidate["_fi2_intake_rejection"], json!(true));
    }

    #[test]
    fn invalid_time_alone_retains_a_refusal_after_normalization() {
        let (allowed, records) = presented(&[], &[]);
        let mut drafts = vec![draft(json!({"summary":"source","valid_from":"nonsense"}))];
        sanitize_declared_metadata(&mut drafts, &allowed, &records);
        assert!(drafts[0].candidate.get("valid_from").is_none());
        assert_eq!(
            drafts[0].candidate["_fi2_intake_rejection"],
            json!("invalid_time_scope")
        );
    }

    #[test]
    fn nested_unpresented_links_refuse_the_whole_item() {
        let (allowed, records) = presented(&[], &[]);
        for link in [
            format!("evidence:{}", EvidenceId::from_u128(0xdead)),
            format!("state:{}", MemoryId::from_u128(0xdead)),
        ] {
            let mut drafts = vec![draft(
                json!({"projection":{"summary":"source","links":[link]}}),
            )];
            sanitize_declared_metadata(&mut drafts, &allowed, &records);
            assert_eq!(drafts[0].candidate["_fi2_intake_rejection"], json!(true));
        }
    }

    #[test]
    fn declared_tags_are_deduplicated_and_capped() {
        let (allowed, records) = presented(&[], &[]);
        let mut drafts = vec![draft(json!({
            "tags": ["t9", "t8", "t7", "t6", "t5", "t4", "t3", "t2", "t1", "t0"],
        }))];
        sanitize_declared_metadata(&mut drafts, &allowed, &records);
        let tags = drafts[0].candidate["tags"].as_array().unwrap();
        assert_eq!(tags.len(), MAX_DECLARED_TAGS);
    }
}
