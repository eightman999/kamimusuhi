use kamimusuhi_core::c0::{
    CorrectionHint, ImprovementDraft, ImprovementKind, ReflectionInput, ReflectionOutput,
    Reflector, SelfField,
};
use kamimusuhi_core::evidence::EvidenceKind;
use kamimusuhi_core::ids::PersonaBackendId;
use kamimusuhi_core::mutation::{MutationDomain, MutationOperation, OriginClass};
use kamimusuhi_core::persona::{PersonaBackendDescriptor, PersonaError, ProposalDraft};

/// Deterministic reflector fixture.
///
/// Rules, all cite-checked against the presented input — the fixture invents
/// nothing and never names an ID it was not shown:
///
/// * **correction**: a user episode containing `間違` or `訂正` yields a
///   `corrects` link to the latest earlier user utterance, and — when an
///   active relationship record exists for the subject — a
///   `relationship.correction` draft superseding it;
/// * **episodic capture**: uncaptured recent user utterances yield one
///   `episodic.capture` draft citing them;
/// * **retrieval update**: a measured `retrieval_missed` yields a
///   `retrieval.evidence_top_k` raise;
/// * **policy update**: mean response length over the configured bound (or
///   400 chars when unbounded) yields a `conversation.max_response_chars`
///   bound;
/// * **self update**: with agent utterances present, a `capabilities`
///   entry records that relationship facts were retained — cited to the
///   individual's own utterances, never to user speech.
///
/// Every rule fires only on what was presented, so two identical reflections
/// produce identical output.
#[derive(Debug, Default, Clone, Copy)]
pub struct FakeReflector;

/// The fixture's backend identity. Distinct from the fake persona's.
pub const FIXTURE_REFLECTOR_BACKEND_ID: PersonaBackendId = PersonaBackendId::from_u128(0x0FE5);

impl Reflector for FakeReflector {
    fn descriptor(&self) -> PersonaBackendDescriptor {
        PersonaBackendDescriptor {
            backend_id: FIXTURE_REFLECTOR_BACKEND_ID,
            kind: "fixture".to_owned(),
            name: "fake-reflector".to_owned(),
            version: "1".to_owned(),
        }
    }

    fn reflect(&self, input: &ReflectionInput) -> Result<ReflectionOutput, PersonaError> {
        let mut output = ReflectionOutput {
            notes: vec![format!(
                "reviewed {} episodes, {} memory records, {} pending proposals",
                input.episodes.len(),
                input.memory_records.len(),
                input.pending_proposals.len()
            )],
            ..ReflectionOutput::default()
        };

        let user_episodes: Vec<_> = input
            .episodes
            .iter()
            .filter(|e| e.kind == EvidenceKind::UserUtterance)
            .collect();
        let agent_episodes: Vec<_> = input
            .episodes
            .iter()
            .filter(|e| e.kind == EvidenceKind::AgentUtterance)
            .collect();

        // --- correction rule ------------------------------------------------
        if let Some(correction) = user_episodes
            .iter()
            .rev()
            .find(|e| e.text.contains("間違") || e.text.contains("訂正"))
            && let Some(target) = user_episodes
                .iter()
                .rev()
                .find(|e| e.evidence_id != correction.evidence_id && !e.is_corrected)
        {
            output.evidence_corrections.push(CorrectionHint {
                correction: correction.evidence_id,
                corrects: target.evidence_id,
            });
            if let Some(record) = input
                .memory_records
                .iter()
                .rev()
                .find(|r| r.domain == MutationDomain::Relationship)
            {
                output.canonical_drafts.push(ProposalDraft {
                    domain: MutationDomain::Relationship,
                    operation: MutationOperation::Correction,
                    subject_key: record.subject_key.clone(),
                    candidate: serde_json::json!({
                        "statement": correction.text,
                        "corrected_by": correction.evidence_id,
                    }),
                    evidence_refs: vec![correction.evidence_id],
                    supersedes: Some(record.state_record_id),
                    // The user said it: canonical facts require an
                    // external-event origin.
                    origin_class: OriginClass::Reported,
                });
            }
        }

        // --- episodic capture rule ------------------------------------------
        // Capture recent user testimony that no active episodic record cites.
        let cited: std::collections::BTreeSet<_> = input
            .memory_records
            .iter()
            .flat_map(|r| r.evidence_refs.iter().copied())
            .collect();
        let uncaptured: Vec<_> = user_episodes
            .iter()
            .filter(|e| !cited.contains(&e.evidence_id))
            .collect();
        if uncaptured.len() >= 2 {
            output.canonical_drafts.push(ProposalDraft {
                domain: MutationDomain::Episodic,
                operation: MutationOperation::Capture,
                subject_key: input.subject_key.clone(),
                candidate: serde_json::json!({
                    "summary": format!("{} utterances from this subject", uncaptured.len()),
                    "turns": uncaptured.len(),
                }),
                evidence_refs: uncaptured.iter().map(|e| e.evidence_id).collect(),
                supersedes: None,
                origin_class: OriginClass::Reported,
            });
        }

        // --- retrieval update rule ------------------------------------------
        let missed: Vec<_> = input
            .metrics
            .iter()
            .enumerate()
            .filter(|(_, m)| m.retrieval_missed)
            .collect();
        if !missed.is_empty() {
            let current = input.operative.params.retrieval.evidence_top_k;
            let proposed = (current + 4).min(16);
            if proposed > current {
                output.improvement_drafts.push(ImprovementDraft {
                    kind: ImprovementKind::RetrievalUpdate,
                    target: "retrieval.evidence_top_k".to_owned(),
                    target_key: None,
                    proposed_value: serde_json::json!(proposed),
                    evidence_refs: user_episodes
                        .iter()
                        .rev()
                        .take(2)
                        .map(|e| e.evidence_id)
                        .collect(),
                    expected_effect: Some(
                        "surface recalled evidence on questions memory did not answer".to_owned(),
                    ),
                    risk: Some("larger recall section per turn".to_owned()),
                    confidence: Some(0.7),
                });
            }
        }

        // --- policy update rule ----------------------------------------------
        if !input.metrics.is_empty() {
            let mean_len = input
                .metrics
                .iter()
                .map(|m| m.response_chars as f64)
                .sum::<f64>()
                / input.metrics.len() as f64;
            let bound = input.operative.params.conversation.max_response_chars;
            if (bound == 0 && mean_len > 400.0) || (bound > 0 && mean_len > bound as f64 * 1.5) {
                output.improvement_drafts.push(ImprovementDraft {
                    kind: ImprovementKind::PolicyUpdate,
                    target: "conversation.max_response_chars".to_owned(),
                    target_key: None,
                    proposed_value: serde_json::json!(160),
                    evidence_refs: agent_episodes
                        .iter()
                        .rev()
                        .take(1)
                        .map(|e| e.evidence_id)
                        .collect(),
                    expected_effect: Some("shorter responses".to_owned()),
                    risk: Some("responses may become terse".to_owned()),
                    confidence: Some(0.55),
                });
            }
        }

        // --- self update rule -------------------------------------------------
        let has_capability_entry = input
            .operative
            .self_model
            .field(SelfField::Capabilities)
            .iter()
            .any(|e| e.key.as_deref() == Some("relationship_recall"));
        if !has_capability_entry && !agent_episodes.is_empty() {
            output.improvement_drafts.push(ImprovementDraft {
                kind: ImprovementKind::SelfUpdate,
                target: SelfField::Capabilities.as_str().to_owned(),
                target_key: Some("relationship_recall".to_owned()),
                proposed_value: serde_json::json!(
                    "retains and recalls subject-scoped facts across sessions"
                ),
                // First-party evidence only: the individual's own utterances.
                evidence_refs: agent_episodes
                    .iter()
                    .rev()
                    .take(1)
                    .map(|e| e.evidence_id)
                    .collect(),
                expected_effect: Some("self model records a working capability".to_owned()),
                risk: None,
                confidence: Some(0.6),
            });
        }

        Ok(output)
    }
}
