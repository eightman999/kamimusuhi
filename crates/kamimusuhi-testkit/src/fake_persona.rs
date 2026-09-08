use kamimusuhi_core::mutation::{MutationDomain, MutationOperation, OriginClass};
use kamimusuhi_core::persona::{
    PersonaCore, PersonaError, PersonaTurnInput, PersonaTurnResult, ProposalDraft,
};

/// Subject key used for the fixture user in all deterministic scenarios.
pub const FIXTURE_USER_SUBJECT: &str = "user-fixture";

/// Deterministic Persona Core fixture.
///
/// Rule: an utterance of the form `私は<X>が好き` that also contains
/// `覚えておいて` yields one `relationship.fact` draft
/// `{ "preference": "<X>" }` for [`FIXTURE_USER_SUBJECT`], citing exactly the
/// input evidence ID. Every other utterance yields an acknowledgement with no
/// drafts. The fake never touches storage and never fabricates evidence.
#[derive(Debug, Default, Clone, Copy)]
pub struct FakePersonaCore;

impl FakePersonaCore {
    fn extract_preference(text: &str) -> Option<&str> {
        if !text.contains("覚えておいて") {
            return None;
        }
        let start = text.find("私は")? + "私は".len();
        let rest = &text[start..];
        let end = rest.find("が好き")?;
        let preference = rest[..end].trim();
        (!preference.is_empty()).then_some(preference)
    }
}

impl PersonaCore for FakePersonaCore {
    fn turn(&self, input: PersonaTurnInput) -> Result<PersonaTurnResult, PersonaError> {
        if input.input.text.trim().is_empty() {
            return Err(PersonaError::InvalidInput {
                reason: "empty utterance".to_owned(),
            });
        }
        if input.input.evidence_id.is_nil() {
            return Err(PersonaError::InvalidInput {
                reason: "current input has no evidence id".to_owned(),
            });
        }

        let (response_intent, proposals) = match Self::extract_preference(&input.input.text) {
            Some(preference) => (
                format!("fixture-ack: remembered preference for {preference}"),
                vec![ProposalDraft {
                    domain: MutationDomain::Relationship,
                    operation: MutationOperation::Fact,
                    subject_key: Some(FIXTURE_USER_SUBJECT.to_owned()),
                    candidate: serde_json::json!({ "preference": preference }),
                    evidence_refs: vec![input.input.evidence_id],
                    supersedes: None,
                    origin_class: OriginClass::Reported,
                }],
            ),
            None => ("fixture-ack: noted".to_owned(), Vec::new()),
        };

        Ok(PersonaTurnResult {
            context: input.context,
            response_intent,
            proposals,
        })
    }
}

#[cfg(test)]
mod tests {
    use kamimusuhi_core::ids::{EvidenceId, IndividualId, SessionId, TurnId};
    use kamimusuhi_core::persona::{CurrentInput, TurnContext};

    use super::*;

    fn input(text: &str) -> PersonaTurnInput {
        PersonaTurnInput {
            context: TurnContext {
                individual_id: IndividualId::from_u128(1),
                session_id: SessionId::from_u128(2),
                turn_id: TurnId::from_u128(3),
            },
            input: CurrentInput {
                evidence_id: EvidenceId::from_u128(4),
                text: text.to_owned(),
            },
        }
    }

    #[test]
    fn preference_fixture_yields_relationship_fact_citing_input_evidence() {
        let result = FakePersonaCore
            .turn(input("私はほうじ茶が好き。覚えておいて"))
            .unwrap();
        assert_eq!(result.context.individual_id, IndividualId::from_u128(1));
        assert_eq!(result.context.turn_id, TurnId::from_u128(3));
        assert_eq!(result.proposals.len(), 1);
        let draft = &result.proposals[0];
        assert_eq!(draft.domain, MutationDomain::Relationship);
        assert_eq!(draft.operation, MutationOperation::Fact);
        assert_eq!(draft.subject_key.as_deref(), Some(FIXTURE_USER_SUBJECT));
        assert_eq!(
            draft.candidate,
            serde_json::json!({ "preference": "ほうじ茶" })
        );
        assert_eq!(draft.evidence_refs, vec![EvidenceId::from_u128(4)]);
        assert_eq!(draft.origin_class, OriginClass::Reported);
    }

    #[test]
    fn same_input_is_deterministic() {
        let a = FakePersonaCore
            .turn(input("私はほうじ茶が好き。覚えておいて"))
            .unwrap();
        let b = FakePersonaCore
            .turn(input("私はほうじ茶が好き。覚えておいて"))
            .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn plain_utterance_produces_no_drafts() {
        let result = FakePersonaCore.turn(input("こんにちは")).unwrap();
        assert!(result.proposals.is_empty());
        assert_eq!(result.response_intent, "fixture-ack: noted");
    }

    #[test]
    fn preference_without_remember_request_is_not_drafted() {
        let result = FakePersonaCore.turn(input("私はほうじ茶が好き")).unwrap();
        assert!(result.proposals.is_empty());
    }

    #[test]
    fn rejects_empty_or_unevidenced_input() {
        assert!(matches!(
            FakePersonaCore.turn(input("   ")),
            Err(PersonaError::InvalidInput { .. })
        ));
        let mut unevidenced = input("こんにちは");
        unevidenced.input.evidence_id = EvidenceId::from_u128(0);
        assert!(matches!(
            FakePersonaCore.turn(unevidenced),
            Err(PersonaError::InvalidInput { .. })
        ));
    }
}
