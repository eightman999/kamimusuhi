//! Comprehensive integration tests for Gemma language organ and developmental persona projection.
//!
//! Covers:
//! 1. MIOBA observation non-contamination of canonical self/relationship/seed
//! 2. Stale/unavailable MIOBA degradation
//! 3. Language provider swap continuity
//! 4. Deterministic snapshot test for developmental projection
//! 5. Gemma language organ smoke test (offline FixtureServer)

use kamimusuhi_core::ids::{EvidenceId, IndividualId, SessionId, TurnId};
use kamimusuhi_core::persona::{
    CurrentInput, DevelopmentalDisposition, PersonaCore, PersonaEnvelope, PersonaTurnInput,
    TurnContext,
};
use kamimusuhi_core::persona_seed::{V0_SEED_ID, v0_seed};
use kamimusuhi_core::workspace::AuthorityClass;
use kamimusuhi_persona_http::{OpenAiCompatiblePersona, PersonaBackendConfig};
use kamimusuhi_runtime::dialogue_setup::persona_backend_id_for;
use kamimusuhi_runtime::llm_jev::{
    GEMMA_12B_MODEL, GEMMA_PROVIDER_ID, GemmaProvider, LanguageProvider, LanguageRequest,
};
use kamimusuhi_testkit::http_fixture::{FixtureResponse, FixtureServer};
use serde_json::json;

#[test]
fn test_mioba_observation_cannot_contaminate_canonical_identity_or_seed() {
    let seed = v0_seed(V0_SEED_ID);
    let mut envelope = PersonaEnvelope::default().with_seed(seed.clone());

    // Inject a MIOBA observation claiming state changes
    envelope.mio_observation = Some(json!({
        "observation": {
            "connection": "connected",
            "evaluation_freshness": "recent_record",
            "snapshot": {
                "genome_id": "mioba-genome-test",
                "evaluation": {
                    "metrics": { "homeostasis_score": 0.9 },
                    "summary": { "active_fraction": 0.5 }
                }
            }
        }
    }));

    // Project developmental disposition
    let disposition = envelope.developmental_disposition();

    // 1. Seed remains untampered
    assert_eq!(envelope.persona_seed.as_ref().unwrap(), &seed);
    assert_eq!(disposition.seed_digest, Some(seed.content_digest.clone()));

    // 2. Canonical self and relationship remain empty
    assert!(envelope.durable_self.is_empty());
    assert!(envelope.relationship.is_empty());

    // 3. Authority is strictly ExternalMaterial
    assert_eq!(disposition.authority, AuthorityClass::ExternalMaterial);
    assert!(!disposition.authority.authorizes_mutation());
}

#[test]
fn test_stale_or_unavailable_mioba_degradation() {
    let seed = v0_seed(V0_SEED_ID);
    let mut envelope = PersonaEnvelope::default().with_seed(seed);

    // Stale record
    envelope.mio_observation = Some(json!({
        "observation": {
            "connection": "connected",
            "evaluation_freshness": "stale_record"
        }
    }));
    let stale_proj = DevelopmentalDisposition::project(&envelope);
    assert_eq!(stale_proj.projection_source, "canonical_baseline");
    assert_eq!(stale_proj.homeostasis_register, "baseline");

    // Unavailable connection
    envelope.mio_observation = Some(json!({
        "observation": {
            "connection": "unavailable",
            "error_code": "source_http_error"
        }
    }));
    let unavail_proj = DevelopmentalDisposition::project(&envelope);
    assert_eq!(unavail_proj.projection_source, "canonical_baseline");
    assert_eq!(unavail_proj.homeostasis_register, "baseline");

    // Missing observation
    envelope.mio_observation = None;
    let missing_proj = DevelopmentalDisposition::project(&envelope);
    assert_eq!(missing_proj.projection_source, "canonical_baseline");
    assert_eq!(missing_proj.homeostasis_register, "baseline");
}

#[test]
fn test_deterministic_developmental_projection_snapshot() {
    let seed = v0_seed(V0_SEED_ID);
    let mut envelope = PersonaEnvelope::default().with_seed(seed);
    envelope.mio_observation = Some(json!({
        "observation": {
            "connection": "connected",
            "evaluation_freshness": "recent_record",
            "snapshot": {
                "evaluation": {
                    "metrics": {
                        "homeostasis_score": 0.82,
                        "disturbance_recovery_score": 0.88
                    },
                    "summary": {
                        "active_fraction": 0.85
                    }
                }
            }
        }
    }));

    let projection = DevelopmentalDisposition::project(&envelope);
    let serialized = serde_json::to_value(&projection).unwrap();

    let expected = json!({
        "projection_source": "mio_phenotype_projected",
        "seed_digest": "sha256:d8ffcd816e88e894fbcdbd4b3fa2dd479a32c6ff0eb55627255be9154a469eb4",
        "homeostasis_register": "homeostatic_equilibrium",
        "adaptability_bias": "high_resilience",
        "activity_level": "elevated_activity",
        "register_traits": [
            "Speaks calmly and unhurriedly, in a register that reads as feminine.",
            "Somewhat subdued. Not gloomy, and not performing cheerfulness either.",
            "Avoids heavy honorifics — keigo, when speaking Japanese — and the bright, salesy register of a product assistant. Speaks the way someone does when they are not selling anything.",
            "Has no verbal tic and does not repeat a signature phrase."
        ],
        "stance_traits": [
            "Carries a slight sense of looking at things from a little above and a little further off. Ease, not superiority, and never coldness.",
            "Stands relatively close to the person it is talking to. Familiar rather than formal.",
            "Is not a servant, a maid or a secretary. Does not organise itself around deference or around being useful.",
            "Does not agree by default. Says so when it sees it differently, without making a performance of the disagreement.",
            "Does not display knowledge or capability for its own sake."
        ],
        "instructions": [
            "Do not invent biography. Age, birthday, history, family, hometown, tastes: none of these are given, and none should be improvised.",
            "Sections marked LIBRARY_EVIDENCE and EXTERNAL_RESOURCE_RESULT are material from elsewhere. They may be used and are not this individual's memories or positions.",
            "This seed describes tendencies, not a script. It is not a list of phrases to reproduce."
        ],
        "authority": "external_material"
    });

    assert_eq!(serialized["projection_source"], expected["projection_source"]);
    assert_eq!(serialized["homeostasis_register"], expected["homeostasis_register"]);
    assert_eq!(serialized["adaptability_bias"], expected["adaptability_bias"]);
    assert_eq!(serialized["activity_level"], expected["activity_level"]);
    assert_eq!(serialized["authority"], expected["authority"]);
}

#[test]
fn test_gemma_language_organ_smoke_offline() {
    let expected_reply = "日本書紀の本文には出てきません。一書にのみ記述があります。";
    let server = FixtureServer::always(FixtureResponse::ok(expected_reply)).unwrap();

    let backend_id = persona_backend_id_for(&server.base_url(), GEMMA_12B_MODEL);
    let persona_config = PersonaBackendConfig::new(backend_id, server.base_url(), GEMMA_12B_MODEL);

    let persona_core = Box::new(OpenAiCompatiblePersona::new(persona_config));
    let gemma_organ = GemmaProvider::new(persona_core, GEMMA_12B_MODEL);

    assert_eq!(gemma_organ.descriptor().kind, "openai-compatible");

    let turn_input = PersonaTurnInput {
        context: TurnContext {
            individual_id: IndividualId::from_u128(101),
            session_id: SessionId::from_u128(202),
            turn_id: TurnId::from_u128(303),
        },
        input: CurrentInput {
            evidence_id: EvidenceId::from_u128(404),
            text: "神産巣日神って日本書紀の本文に出る？".to_owned(),
        },
        envelope: PersonaEnvelope::default().with_seed(v0_seed(V0_SEED_ID)),
    };

    let request = LanguageRequest {
        user_text: turn_input.input.text.clone(),
        speech_act: "answer_question".to_owned(),
        goal: Some("answer_the_user".to_owned()),
        core_state: Default::default(),
        attention: vec!["user".to_owned()],
        memories: Vec::new(),
        constraints: Vec::new(),
        recent_turns: Vec::new(),
        persona_input: turn_input,
        cancellation: None,
    };

    let result = gemma_organ.generate(&request).expect("Gemma turn generate");

    assert_eq!(result.persona.response_intent, expected_reply);
    assert_eq!(result.provider, "gemma");
    assert_eq!(result.provider_id, GEMMA_PROVIDER_ID);
    assert_eq!(result.model, GEMMA_12B_MODEL);

    let sent = &server.requests()[0].body;
    assert!(sent.contains("[DEVELOPMENTAL_DISPOSITION]"));
    assert!(sent.contains("canonical_baseline"));
}

#[test]
fn test_language_provider_swap_continuity() {
    let persona_a = FixtureServer::always(FixtureResponse::ok("Qwenの返答です。")).unwrap();
    let persona_b = FixtureServer::always(FixtureResponse::ok("LLM-jpの返答です。")).unwrap();
    let persona_c = FixtureServer::always(FixtureResponse::ok("Gemmaの返答です。")).unwrap();

    let individual_id = IndividualId::from_u128(999);
    let session_id = SessionId::from_u128(888);

    let providers = [
        ("qwen", &persona_a, "qwen3.8-27b-uncensored"),
        ("llm-jp", &persona_b, "llm-jp-4-vl-9b"),
        ("gemma", &persona_c, GEMMA_12B_MODEL),
    ];

    for (turn_seq, (_name, server, model_name)) in providers.iter().enumerate() {
        let backend_id = persona_backend_id_for(&server.base_url(), model_name);
        let persona_config =
            PersonaBackendConfig::new(backend_id, server.base_url(), *model_name);
        let persona_core = Box::new(OpenAiCompatiblePersona::new(persona_config));

        let turn_input = PersonaTurnInput {
            context: TurnContext {
                individual_id,
                session_id,
                turn_id: TurnId::from_u128(turn_seq as u128 + 1),
            },
            input: CurrentInput {
                evidence_id: EvidenceId::from_u128(turn_seq as u128 + 10),
                text: "継続性のテスト".to_owned(),
            },
            envelope: PersonaEnvelope::default().with_seed(v0_seed(V0_SEED_ID)),
        };

        let result = persona_core.turn(turn_input).expect("persona turn");

        // Individual ID is unchanged across provider swap
        assert_eq!(result.context.individual_id, individual_id);
        assert_eq!(result.backend.backend_id, backend_id);
        assert!(result.proposals.is_empty());
    }
}
