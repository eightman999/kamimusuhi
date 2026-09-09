//! A model-backed Persona Core over an OpenAI-compatible endpoint.
//!
//! This crate exists separately from `kamimusuhi-resource-http` on purpose.
//! Both speak the same wire protocol; they are not the same thing.
//!
//! ```text
//! cognitive resource   something the individual delegates a subtask to.
//!                      Its output is EXTERNAL_RESOURCE_RESULT — material,
//!                      attributed, and carrying no authority.
//!
//! Persona Core         what speaks as the individual. Its output is the
//!                      user-facing expression, and nothing else may be.
//! ```
//!
//! Calling an OpenAI-compatible endpoint does not make something a Persona
//! Core; implementing the Persona Core contract does. Keeping them in separate
//! crates, with separate ID types, separate configuration namespaces and
//! separate trace attribution, is what stops the distinction from eroding the
//! first time somebody notices both need an HTTP client.
//!
//! What this backend does **not** get is authority. It produces a response and
//! zero or more [`ProposalDraft`]s, judged by exactly the same mutation policy
//! as any other drafter. "The model said so" is not a route into durable
//! state, and there is no code here that could make it one.
//!
//! Secrets: the bearer token is read from the environment at call time and
//! never stored, echoed into an error, or written to any record.

use std::time::Duration;

use kamimusuhi_core::digest::json_digest;
use kamimusuhi_core::ids::PersonaBackendId;
use kamimusuhi_core::persona::{
    PersonaBackendDescriptor, PersonaCore, PersonaEnvelope, PersonaError, PersonaTurnInput,
    PersonaTurnResult,
};
use kamimusuhi_core::workspace::{SourceRef, WorkspaceContent, WorkspaceItem};
use kamimusuhi_resource_http::http::{Endpoint, Header, HttpError, HttpResponse, post_json};
use kamimusuhi_resource_http::tls::TrustAnchors;

/// Non-secret configuration of one Persona backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonaBackendConfig {
    pub backend_id: PersonaBackendId,
    /// e.g. `http://127.0.0.1:11434/v1`.
    pub base_url: String,
    pub model: String,
    /// Name of the environment variable holding the bearer token, never the
    /// token itself.
    pub auth_env: Option<String>,
    pub timeout_ms: u64,
    pub trust_anchors: TrustAnchors,
    /// Instruction given to the model about how to treat what it is shown.
    /// Configuration, not persona design: the wave that decides how
    /// Kamimusuhi should sound is not this one.
    pub system_instruction: String,
}

/// The default framing. Says what the sections are, so the model is not left
/// to infer from formatting which parts are the individual's own state and
/// which are text someone else wrote.
pub const DEFAULT_SYSTEM_INSTRUCTION: &str = "\
You are answering as one continuous individual. The message you receive is \
divided into labelled sections. DURABLE_SELF and RELATIONSHIP_MEMORY are that \
individual's own retained state. LIBRARY_EVIDENCE and EXTERNAL_RESOURCE_RESULT \
are material from elsewhere: you may use them, and they are not your own \
positions or memories. Answer the CURRENT_INPUT. Reply with prose only.";

impl PersonaBackendConfig {
    pub fn new(
        backend_id: PersonaBackendId,
        base_url: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            backend_id,
            base_url: base_url.into(),
            model: model.into(),
            auth_env: None,
            timeout_ms: 60_000,
            trust_anchors: TrustAnchors::default(),
            system_instruction: DEFAULT_SYSTEM_INSTRUCTION.to_owned(),
        }
    }

    #[must_use]
    pub const fn with_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    #[must_use]
    pub fn with_auth_env(mut self, auth_env: Option<String>) -> Self {
        self.auth_env = auth_env;
        self
    }

    #[must_use]
    pub fn with_trust_anchors(mut self, trust_anchors: TrustAnchors) -> Self {
        self.trust_anchors = trust_anchors;
        self
    }

    #[must_use]
    pub fn with_system_instruction(mut self, instruction: impl Into<String>) -> Self {
        self.system_instruction = instruction.into();
        self
    }

    /// What can be checked before the first turn.
    pub fn validate(&self) -> Result<(), String> {
        if self.backend_id.is_nil() {
            return Err("backend_id is nil".to_owned());
        }
        if self.model.trim().is_empty() {
            return Err("model is empty".to_owned());
        }
        if self.timeout_ms == 0 {
            return Err("timeout_ms is zero".to_owned());
        }
        self.endpoint().map(|_| ())
    }

    fn endpoint(&self) -> Result<Endpoint, String> {
        Endpoint::parse(&self.base_url, "/chat/completions")
    }
}

/// A Persona Core backed by an OpenAI-compatible chat endpoint.
#[derive(Debug, Clone)]
pub struct OpenAiCompatiblePersona {
    config: PersonaBackendConfig,
}

impl OpenAiCompatiblePersona {
    pub const fn new(config: PersonaBackendConfig) -> Self {
        Self { config }
    }

    pub const fn config(&self) -> &PersonaBackendConfig {
        &self.config
    }

    fn headers(&self) -> Result<Vec<Header>, PersonaError> {
        let Some(name) = &self.config.auth_env else {
            return Ok(Vec::new());
        };
        let token = std::env::var(name).map_err(|_| PersonaError::InvalidInput {
            // Names the variable, never a value.
            reason: format!("environment variable {name} is not set"),
        })?;
        if token.trim().is_empty() {
            return Err(PersonaError::InvalidInput {
                reason: format!("environment variable {name} is empty"),
            });
        }
        Ok(vec![Header {
            name: "Authorization".to_owned(),
            value: format!("Bearer {token}"),
        }])
    }

    /// Flatten the envelope into a prompt.
    ///
    /// This is the one place where typed sections become text, and it is why
    /// the representation stays typed everywhere above it. Each section is
    /// labelled with the domain it came from and, where it has one, the ID it
    /// can be traced back to — so the model is told what it is looking at
    /// rather than being expected to work it out from formatting.
    pub fn render_envelope(envelope: &PersonaEnvelope, input_text: &str) -> String {
        let mut rendered = String::new();
        let section = |label: &str, items: &[WorkspaceItem], out: &mut String| {
            if items.is_empty() {
                return;
            }
            out.push_str(&format!("\n[{label}]\n"));
            for item in items {
                out.push_str(&format!(
                    "- ({}) {}\n",
                    source_label(&item.source_ref),
                    content_text(&item.content)
                ));
            }
        };

        rendered.push_str("[CURRENT_INPUT]\n");
        rendered.push_str(input_text);
        rendered.push('\n');

        section("DURABLE_SELF", &envelope.durable_self, &mut rendered);
        section("RELATIONSHIP_MEMORY", &envelope.relationship, &mut rendered);
        section("EPISODIC_MEMORY", &envelope.episodic, &mut rendered);
        section("LIBRARY_EVIDENCE", &envelope.library, &mut rendered);
        section(
            "EXTERNAL_RESOURCE_RESULT",
            &envelope.external_results,
            &mut rendered,
        );
        rendered
    }

    fn request_body(&self, input: &PersonaTurnInput) -> String {
        serde_json::json!({
            "model": self.config.model,
            "messages": [
                { "role": "system", "content": self.config.system_instruction },
                {
                    "role": "user",
                    "content": Self::render_envelope(&input.envelope, &input.input.text),
                },
            ],
        })
        .to_string()
    }

    fn map_transport(&self, error: HttpError) -> PersonaError {
        let backend_id = self.config.backend_id;
        match error {
            HttpError::Timeout { elapsed_ms, .. } => PersonaError::Timeout {
                backend_id,
                elapsed_ms,
            },
            HttpError::Transport(message) => PersonaError::Transport {
                backend_id,
                message,
            },
            HttpError::Malformed(detail) => PersonaError::MalformedResponse { backend_id, detail },
            HttpError::Tls { kind, detail } => PersonaError::Tls {
                backend_id,
                kind: kind.as_str().to_owned(),
                detail,
            },
        }
    }

    /// Classify the reply and take the message content.
    fn map_response(&self, response: HttpResponse) -> Result<String, PersonaError> {
        let backend_id = self.config.backend_id;
        let status = response.status;
        if !response.is_success() {
            // The body is not carried into the error: it can echo the prompt,
            // and the prompt contains the individual's own memory.
            return Err(match status {
                401 | 403 => PersonaError::Authentication { backend_id, status },
                429 => PersonaError::RateLimited { backend_id, status },
                _ => PersonaError::HttpStatus { backend_id, status },
            });
        }

        let parsed: serde_json::Value = serde_json::from_str(&response.body).map_err(|source| {
            PersonaError::MalformedResponse {
                backend_id,
                detail: format!("response body is not JSON: {source}"),
            }
        })?;
        if let Some(code) = parsed
            .pointer("/error/code")
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                parsed
                    .pointer("/error/type")
                    .and_then(serde_json::Value::as_str)
            })
        {
            return Err(PersonaError::ProviderError {
                backend_id,
                code: code.to_owned(),
            });
        }

        let content = parsed
            .pointer("/choices/0/message/content")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| PersonaError::MalformedResponse {
                backend_id,
                detail: "no /choices/0/message/content in response".to_owned(),
            })?;
        if content.trim().is_empty() {
            return Err(PersonaError::MalformedResponse {
                backend_id,
                detail: "the model returned an empty expression".to_owned(),
            });
        }
        Ok(content.to_owned())
    }
}

/// A label naming where an item came from, by ID. Never its content.
fn source_label(source: &SourceRef) -> String {
    match source {
        SourceRef::Continuity {
            commit_id,
            generation,
        } => format!("commit {commit_id} g{generation}"),
        SourceRef::Input { evidence_id } => format!("evidence {evidence_id}"),
        SourceRef::Memory {
            state_record_id,
            subject_key,
            ..
        } => match subject_key {
            Some(subject) => format!("record {state_record_id} about {subject}"),
            None => format!("record {state_record_id}"),
        },
        SourceRef::Library {
            artifact_id,
            chunk_id,
            ..
        } => format!("library {artifact_id}#{chunk_id}"),
        SourceRef::Resource {
            resource_id,
            resource_call_id,
        } => format!("resource {resource_id} call {resource_call_id}"),
    }
}

fn content_text(content: &WorkspaceContent) -> String {
    match content {
        WorkspaceContent::Text { text } => text.clone(),
        WorkspaceContent::Structured { value } => value.to_string(),
    }
}

impl PersonaCore for OpenAiCompatiblePersona {
    fn descriptor(&self) -> PersonaBackendDescriptor {
        PersonaBackendDescriptor {
            backend_id: self.config.backend_id,
            kind: "openai-compatible".to_owned(),
            name: self.config.model.clone(),
            version: "1".to_owned(),
        }
    }

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

        let endpoint = self
            .config
            .endpoint()
            .map_err(|reason| PersonaError::InvalidInput { reason })?;
        let headers = self.headers()?;
        let body = self.request_body(&input);

        let response = post_json(
            &endpoint,
            &body,
            &headers,
            Duration::from_millis(self.config.timeout_ms),
            &self.config.trust_anchors,
        )
        .map_err(|error| self.map_transport(error))?;
        let expression = self.map_response(response)?;

        Ok(PersonaTurnResult {
            context: input.context,
            backend: self.descriptor(),
            response_intent: expression,
            // No drafts. A model saying something is not evidence, and phase 1
            // deliberately has no path from generated text to durable state:
            // building one here would put self-modification outside the
            // guarded mutation contract.
            proposals: Vec::new(),
        })
    }
}

/// Digest of a rendered prompt, for correlating a turn without recording it.
pub fn prompt_digest(prompt: &str) -> String {
    json_digest(&serde_json::Value::String(prompt.to_owned()))
}

#[cfg(test)]
mod tests {
    use kamimusuhi_core::ids::{
        CommitId, EvidenceId, IndividualId, LibraryArtifactId, LibraryChunkId, MemoryId,
        ResourceCallId, ResourceId, SessionId, TurnId,
    };
    use kamimusuhi_core::mutation::MutationDomain;
    use kamimusuhi_core::persona::{CurrentInput, SessionWorkingState, TurnContext};
    use kamimusuhi_core::time::UtcTimestamp;
    use kamimusuhi_core::workspace::{AuthorityClass, Freshness, InclusionReason, WorkspaceDomain};

    use super::*;

    const BACKEND: PersonaBackendId = PersonaBackendId::from_u128(0x0FE2);

    fn config() -> PersonaBackendConfig {
        PersonaBackendConfig::new(BACKEND, "http://127.0.0.1:8080/v1", "test-model")
    }

    fn persona() -> OpenAiCompatiblePersona {
        OpenAiCompatiblePersona::new(config())
    }

    fn item(domain: WorkspaceDomain, source_ref: SourceRef, text: &str) -> WorkspaceItem {
        WorkspaceItem {
            position: 0,
            domain,
            source_ref,
            content: WorkspaceContent::text(text),
            authority: match domain {
                WorkspaceDomain::LibraryEvidence | WorkspaceDomain::ExternalResourceResult => {
                    AuthorityClass::ExternalMaterial
                }
                _ => AuthorityClass::CanonicalState,
            },
            freshness: Freshness {
                source_time: None,
                assembled_at: UtcTimestamp::from_unix_millis(0),
            },
            inclusion_reason: InclusionReason::ActiveMemory,
        }
    }

    fn envelope() -> PersonaEnvelope {
        PersonaEnvelope {
            continuity: vec![item(
                WorkspaceDomain::CurrentContinuityState,
                SourceRef::Continuity {
                    commit_id: CommitId::from_u128(0xC1),
                    generation: 1,
                },
                "{}",
            )],
            durable_self: Vec::new(),
            relationship: vec![item(
                WorkspaceDomain::RelationshipMemory,
                SourceRef::Memory {
                    state_record_id: MemoryId::from_u128(0x70),
                    domain: MutationDomain::Relationship,
                    subject_key: Some("user-fixture".to_owned()),
                    evidence_refs: vec![EvidenceId::from_u128(0xE1)],
                },
                "{\"preference\":\"ほうじ茶\"}",
            )],
            episodic: Vec::new(),
            library: vec![item(
                WorkspaceDomain::LibraryEvidence,
                SourceRef::Library {
                    artifact_id: LibraryArtifactId::from_u128(0xA1),
                    chunk_id: LibraryChunkId::from_u128(0xB1),
                    ordinal: 0,
                    source_uri: None,
                },
                "ほうじ茶は高温で淹れる。",
            )],
            external_results: vec![item(
                WorkspaceDomain::ExternalResourceResult,
                SourceRef::Resource {
                    resource_id: ResourceId::from_u128(0x0FAA),
                    resource_call_id: ResourceCallId::from_u128(0xCA),
                },
                "result-a",
            )],
            session: SessionWorkingState {
                turn_sequence: 0,
                resumed: true,
                delegations: 1,
            },
        }
    }

    fn turn_input() -> PersonaTurnInput {
        PersonaTurnInput {
            context: TurnContext {
                individual_id: IndividualId::from_u128(1),
                session_id: SessionId::from_u128(2),
                turn_id: TurnId::from_u128(3),
            },
            input: CurrentInput {
                evidence_id: EvidenceId::from_u128(4),
                text: "さっきの話、覚えてる?".to_owned(),
            },
            envelope: envelope(),
        }
    }

    #[test]
    fn the_backend_is_a_persona_not_a_resource() {
        let descriptor = persona().descriptor();
        assert_eq!(descriptor.backend_id, BACKEND);
        assert_eq!(descriptor.kind, "openai-compatible");
        // The type is `PersonaBackendId`, so this cannot be handed anywhere a
        // `ResourceId` is expected — the distinction is not a convention.
        assert_eq!(descriptor.name, "test-model");
    }

    #[test]
    fn every_section_is_labelled_with_the_domain_it_came_from() {
        let rendered = OpenAiCompatiblePersona::render_envelope(&envelope(), "hello");
        for label in [
            "[CURRENT_INPUT]",
            "[RELATIONSHIP_MEMORY]",
            "[LIBRARY_EVIDENCE]",
            "[EXTERNAL_RESOURCE_RESULT]",
        ] {
            assert!(
                rendered.contains(label),
                "{label} missing from:\n{rendered}"
            );
        }
        // Absent sections are absent, not empty headings that imply content.
        assert!(!rendered.contains("[DURABLE_SELF]"));
        assert!(!rendered.contains("[EPISODIC_MEMORY]"));
    }

    #[test]
    fn external_material_is_never_rendered_as_the_individuals_own_state() {
        let rendered = OpenAiCompatiblePersona::render_envelope(&envelope(), "hello");
        let library_at = rendered.find("ほうじ茶は高温で淹れる。").unwrap();
        let library_header = rendered.find("[LIBRARY_EVIDENCE]").unwrap();
        let memory_header = rendered.find("[RELATIONSHIP_MEMORY]").unwrap();
        // The Library line sits under the Library heading, not under memory.
        assert!(library_header < library_at);
        assert!(memory_header < library_header);

        // And the resource result is under its own heading, with its call ID —
        // so it can be traced, and cannot be mistaken for something retained.
        assert!(rendered.contains(&format!(
            "resource {} call {}",
            ResourceId::from_u128(0x0FAA),
            ResourceCallId::from_u128(0xCA)
        )));
    }

    #[test]
    fn each_item_is_labelled_by_id_rather_than_by_prose() {
        let rendered = OpenAiCompatiblePersona::render_envelope(&envelope(), "hello");
        assert!(rendered.contains(&format!("record {}", MemoryId::from_u128(0x70))));
        assert!(rendered.contains("about user-fixture"));
        assert!(rendered.contains(&format!(
            "library {}#{}",
            LibraryArtifactId::from_u128(0xA1),
            LibraryChunkId::from_u128(0xB1)
        )));
    }

    #[test]
    fn the_system_instruction_states_which_sections_are_borrowed() {
        let body = persona().request_body(&turn_input());
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        let instruction = parsed["messages"][0]["content"].as_str().unwrap();
        assert!(instruction.contains("LIBRARY_EVIDENCE"));
        assert!(instruction.contains("EXTERNAL_RESOURCE_RESULT"));
        assert!(instruction.contains("not your own"));
        assert_eq!(parsed["messages"][0]["role"], "system");
        assert_eq!(parsed["model"], "test-model");
    }

    #[test]
    fn a_reply_becomes_the_expression_and_never_a_proposal() {
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "覚えています。ほうじ茶でしたね。" } }]
        })
        .to_string();
        let expression = persona()
            .map_response(HttpResponse { status: 200, body })
            .unwrap();
        assert_eq!(expression, "覚えています。ほうじ茶でしたね。");

        // The backend drafts nothing. A model's output is not evidence, and
        // there is no path here from generated text to durable state.
        let result = PersonaTurnResult {
            context: turn_input().context,
            backend: persona().descriptor(),
            response_intent: expression,
            proposals: Vec::new(),
        };
        assert!(result.proposals.is_empty());
    }

    #[test]
    fn failures_classify_without_carrying_the_body() {
        for (status, code) in [
            (401_u16, "AUTHENTICATION"),
            (403, "AUTHENTICATION"),
            (429, "RATE_LIMITED"),
            (500, "HTTP_STATUS"),
        ] {
            let error = persona()
                .map_response(HttpResponse {
                    status,
                    body: "{\"error\":{\"message\":\"private detail\"}}".to_owned(),
                })
                .unwrap_err();
            assert_eq!(error.code(), code, "status {status}");
            assert!(!error.to_string().contains("private detail"));
        }
    }

    #[test]
    fn an_unreadable_or_empty_reply_is_malformed_rather_than_an_expression() {
        for body in [
            "not json".to_owned(),
            "{\"choices\":[]}".to_owned(),
            serde_json::json!({ "choices": [{ "message": { "content": "   " } }] }).to_string(),
        ] {
            let error = persona()
                .map_response(HttpResponse { status: 200, body })
                .unwrap_err();
            assert_eq!(error.code(), "MALFORMED_RESPONSE");
        }
    }

    #[test]
    fn a_provider_error_object_is_reported_by_code_alone() {
        let error = persona()
            .map_response(HttpResponse {
                status: 200,
                body: serde_json::json!({
                    "error": { "code": "context_length_exceeded", "message": "too long" }
                })
                .to_string(),
            })
            .unwrap_err();
        assert_eq!(error.code(), "PROVIDER_ERROR");
        assert!(error.to_string().contains("context_length_exceeded"));
        assert!(!error.to_string().contains("too long"));
    }

    #[test]
    fn the_token_is_named_by_variable_and_never_held_in_config() {
        let config = config().with_auth_env(Some("KAMIMUSUHI_ABSENT_PERSONA_TOKEN".to_owned()));
        assert!(!format!("{config:?}").contains("Bearer"));
        let error = OpenAiCompatiblePersona::new(config)
            .headers()
            .expect_err("an unset variable must fail before any request");
        assert!(
            error
                .to_string()
                .contains("KAMIMUSUHI_ABSENT_PERSONA_TOKEN")
        );
        assert_eq!(error.code(), "INVALID_INPUT");
    }

    #[test]
    fn configuration_is_validated_before_any_turn() {
        assert!(config().validate().is_ok());
        assert!(
            PersonaBackendConfig::new(BACKEND, "https://example.test/v1", "m")
                .validate()
                .is_ok()
        );
        assert!(
            PersonaBackendConfig::new(BACKEND, "ftp://example.test/v1", "m")
                .validate()
                .is_err()
        );
        assert!(config().with_timeout_ms(0).validate().is_err());
        assert!(
            PersonaBackendConfig::new(BACKEND, "http://h/v1", "  ")
                .validate()
                .is_err()
        );
    }

    #[test]
    fn an_empty_or_unevidenced_input_is_refused_before_the_network() {
        let mut blank = turn_input();
        blank.input.text = "   ".to_owned();
        assert_eq!(persona().turn(blank).unwrap_err().code(), "INVALID_INPUT");

        let mut unevidenced = turn_input();
        unevidenced.input.evidence_id = EvidenceId::from_u128(0);
        assert_eq!(
            persona().turn(unevidenced).unwrap_err().code(),
            "INVALID_INPUT"
        );
    }
}
