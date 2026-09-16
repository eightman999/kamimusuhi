//! Text interaction over the existing individual and canonical evidence.
//! Raw conversation is recallable, but never activated as a belief. Generated
//! text and a successful write to the output surface have separate records.

use std::time::Instant;

use kamimusuhi_core::digest::{content_digest, json_digest};
use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceSource, EvidenceStore, NewEvidence, NewSession, NewTurn, RetentionClass,
};
use kamimusuhi_core::ids::{EvidenceId, IndividualId, SessionId, TurnId};
use kamimusuhi_core::memory::{MemoryQuery, MemoryRepository};
use kamimusuhi_core::mutation::{MutationDomain, OriginClass};
use kamimusuhi_core::persona::{
    ConversationMessage, ConversationRole, CurrentInput, PersonaBackendDescriptor,
    PersonaTurnInput, SessionWorkingState, TurnContext,
};
use kamimusuhi_core::routing::PrivacyConstraint;
use kamimusuhi_core::trace::{TraceCorrelation, TraceEventKind};
use kamimusuhi_core::workspace::WorkspaceBuilder;
use serde::Serialize;

use crate::research::ResearchCatalog;
use crate::{Runtime, RuntimeError};

pub const MAX_INPUT_BYTES: usize = 8_192;
const HISTORY_MESSAGES: usize = 12;
const HISTORY_BYTES: usize = 16_384;

#[derive(Debug, Serialize)]
pub struct DialogueReply {
    pub individual_id: IndividualId,
    pub session_id: SessionId,
    pub turn_id: TurnId,
    pub input_evidence_id: EvidenceId,
    pub generated_evidence_id: EvidenceId,
    pub history_messages: usize,
    pub observed_runtime: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mio_observation: Option<serde_json::Value>,
    pub research_findings: serde_json::Value,
    pub persona_backend: PersonaBackendDescriptor,
    pub response: String,
}

pub struct DialogueSession {
    session_id: SessionId,
    individual_id: IndividualId,
    subject: String,
    source_id: String,
    sequence: u64,
    privacy: PrivacyConstraint,
    started: Instant,
    research: ResearchCatalog,
}

impl DialogueSession {
    pub fn start(
        runtime: &mut Runtime,
        subject: &str,
        privacy: PrivacyConstraint,
    ) -> Result<Self, RuntimeError> {
        if subject.is_empty()
            || subject.len() > 80
            || !subject
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        {
            return Err(RuntimeError::Usage(
                "--subject must be 1-80 ASCII letters, digits, '.', '_' or '-'".to_owned(),
            ));
        }
        // Configuration and destination failures precede any interaction write.
        runtime.config().persona.check_privacy(privacy)?;
        runtime.config().build_persona()?;
        runtime.config().persona_seed()?;
        if let Some(mio) = &runtime.config().mio {
            mio.validate()?;
        }
        let research = ResearchCatalog::bundled()?;
        let session_id = SessionId::generate(runtime.ids().as_ref());
        if runtime.store().session(session_id)?.is_some() {
            return Err(RuntimeError::IdCollision {
                kind: "session",
                id: session_id.to_string(),
            });
        }
        research.sync(runtime.store())?;
        runtime.store().open_session(NewSession {
            session_id,
            individual_id: runtime.individual_id(),
        })?;
        runtime.set_trace_base(TraceCorrelation {
            session_id: Some(session_id),
            turn_id: None,
            episode_id: None,
            ..runtime.trace().base()
        });
        runtime
            .trace()
            .record(TraceEventKind::SessionStarted, TraceCorrelation::default());
        Ok(Self {
            session_id,
            individual_id: runtime.individual_id(),
            subject: subject.to_owned(),
            source_id: format!("text-chat:{subject}"),
            sequence: 0,
            privacy,
            started: Instant::now(),
            research,
        })
    }

    /// `emit` must return only after the output surface has accepted/flushed
    /// the response. This is delivery to that surface, not human acknowledgement.
    pub fn turn(
        &mut self,
        runtime: &mut Runtime,
        text: &str,
        emit: impl FnOnce(&DialogueReply) -> std::io::Result<()>,
    ) -> Result<DialogueReply, RuntimeError> {
        if runtime.individual_id() != self.individual_id {
            return Err(RuntimeError::Usage(
                "dialogue belongs to another individual".to_owned(),
            ));
        }
        if text.trim().is_empty() || text.len() > MAX_INPUT_BYTES {
            return Err(RuntimeError::Usage(format!(
                "input must be nonempty and at most {MAX_INPUT_BYTES} UTF-8 bytes"
            )));
        }
        runtime.config().persona.check_privacy(self.privacy)?;
        let persona = runtime.config().build_persona()?;
        let backend = persona.descriptor();
        let history = self.history(runtime)?;
        let turn_id = TurnId::generate(runtime.ids().as_ref());
        runtime.store().record_turn(NewTurn {
            turn_id,
            session_id: self.session_id,
            individual_id: self.individual_id,
            sequence: self.sequence,
        })?;
        let sequence = self.sequence;
        self.sequence += 1;
        runtime.set_trace_base(TraceCorrelation {
            session_id: Some(self.session_id),
            turn_id: Some(turn_id),
            episode_id: None,
            ..runtime.trace().base()
        });
        runtime
            .trace()
            .record(TraceEventKind::TurnStarted, TraceCorrelation::default());
        let input_evidence_id = self.append(
            runtime,
            turn_id,
            EvidenceKind::UserUtterance,
            OriginClass::Reported,
            serde_json::json!({"text": text}),
        )?;
        let current_input = CurrentInput {
            evidence_id: input_evidence_id,
            text: text.to_owned(),
        };
        // Restrict both domains to this interlocutor. Unscoped episodes can
        // contain another person's material and are not admitted here.
        let mut memories = Vec::new();
        for domain in [MutationDomain::Relationship, MutationDomain::Episodic] {
            memories.extend(MemoryRepository::retrieve(
                runtime.store(),
                &MemoryQuery::current(self.individual_id)
                    .in_domain(domain)
                    .about(&self.subject)
                    .limited(8),
            )?);
        }
        let head = runtime.head()?;
        let workspace = WorkspaceBuilder::new(self.individual_id, runtime.now())
            .with_continuity(&head)
            .with_current_input(&current_input)
            .with_memories(&memories)
            .build();
        let mio = runtime
            .config()
            .mio
            .as_ref()
            .map(|binding| binding.observe(runtime.clocks().wall.as_ref()));
        let mio_context = if let Some(observation) = &mio {
            let value = serde_json::json!(observation);
            let evidence_id = self.append(
                runtime,
                turn_id,
                EvidenceKind::SystemEvent,
                OriginClass::Observed,
                serde_json::json!({"event": "mio_observed", "observation": value}),
            )?;
            Some(serde_json::json!({"evidence_id": evidence_id, "observation": value}))
        } else {
            None
        };
        // Reuse this turn's already validated MIO snapshot for recall cues.
        let mut research_query = text.to_owned();
        if ["mio", "状態", "調子", "個体"]
            .iter()
            .any(|term| text.to_lowercase().contains(term))
            && let Some(observation) = &mio
        {
            research_query.push_str(observation.research_topics());
        }
        let mut research_context =
            serde_json::json!(self.research.context(runtime.store(), &research_query)?);
        let recalled = research_context["selected"].as_array().map_or(0, Vec::len);
        if recalled > 0 {
            let evidence_id = self.append(
                runtime,
                turn_id,
                EvidenceKind::LibraryExcerpt,
                OriginClass::Reported,
                serde_json::json!({"event": "research_recalled", "context": research_context}),
            )?;
            research_context["evidence_id"] = serde_json::json!(evidence_id);
        }
        let observed = serde_json::json!({
            "observed_at": runtime.now(),
            "individual_id": self.individual_id,
            "continuity_generation": head.generation.0,
            "session_turn": sequence + 1,
            "session_uptime_seconds": self.started.elapsed().as_secs(),
            "os": std::env::consts::OS,
            "architecture": std::env::consts::ARCH,
            "interface": "text",
            "history_messages_available": history.len(),
            "retained_memory_records_available": memories.len(),
            "research_findings_recalled": recalled,
            "speech_output": "not_connected",
            "body_sensors": "not_connected_to_this_interface",
            "experimental_neural_state": match mio.as_ref().map(|m| m.connection.as_str()) {
                Some("connected") => "recorded_mio_evaluations_only",
                Some(_) => "mio_unavailable",
                None => "not_connected_to_this_interface",
            }
        });
        let mut input = PersonaTurnInput::with_workspace(
            TurnContext {
                individual_id: self.individual_id,
                session_id: self.session_id,
                turn_id,
            },
            current_input,
            &workspace,
            SessionWorkingState {
                turn_sequence: sequence,
                resumed: sequence == 0 && !history.is_empty(),
                delegations: 0,
            },
        );
        if let Some(seed) = runtime.config().persona_seed()? {
            input.envelope = input.envelope.with_seed(seed);
        }
        input.envelope.conversation_history = history;
        input.envelope.observed_runtime = Some(observed.clone());
        input.envelope.mio_observation = mio_context.clone();
        input.envelope.research_findings = Some(research_context.clone());
        let context_digest = json_digest(&serde_json::json!(input));
        let history_messages = input.envelope.conversation_history.len();
        runtime.trace().record_with(TraceEventKind::PersonaInvoked, TraceCorrelation {
            persona_backend_id: Some(backend.backend_id), ..TraceCorrelation::default()
        }, serde_json::json!({"input_digest": context_digest, "history_messages": history_messages}));
        let result = persona.turn(input)?;
        runtime.trace().record_with(
            TraceEventKind::PersonaCompleted,
            TraceCorrelation {
                persona_backend_id: Some(result.backend.backend_id),
                ..TraceCorrelation::default()
            },
            serde_json::json!({"draft_count": result.proposals.len(), "drafts_activated": 0}),
        );
        let generated_evidence_id = self.append(
            runtime,
            turn_id,
            EvidenceKind::SystemEvent,
            OriginClass::Observed,
            serde_json::json!({
                "event": "response_generated", "text": result.response_intent,
                "backend": result.backend, "input_evidence_id": input_evidence_id,
                "input_digest": context_digest, "observed_runtime": observed,
                "mio_observation": mio_context,
                "research_findings": research_context,
                "delivery": "not_yet_emitted"
            }),
        )?;
        let reply = DialogueReply {
            individual_id: self.individual_id,
            session_id: self.session_id,
            turn_id,
            input_evidence_id,
            generated_evidence_id,
            history_messages,
            observed_runtime: observed,
            mio_observation: mio_context,
            research_findings: research_context,
            persona_backend: result.backend,
            response: result.response_intent,
        };
        emit(&reply)
            .map_err(|error| RuntimeError::Usage(format!("response output failed: {error}")))?;
        self.append(
            runtime,
            turn_id,
            EvidenceKind::AgentUtterance,
            OriginClass::Observed,
            serde_json::json!({
                "text": reply.response, "generated_evidence_id": generated_evidence_id,
                "backend": reply.persona_backend, "delivery": "output_surface_accepted",
                "human_acknowledgement": false
            }),
        )?;
        runtime.trace().record_with(
            TraceEventKind::FinalExpression,
            TraceCorrelation {
                persona_backend_id: Some(reply.persona_backend.backend_id),
                ..TraceCorrelation::default()
            },
            serde_json::json!({"expression_digest": content_digest(reply.response.as_bytes())}),
        );
        runtime
            .trace()
            .record(TraceEventKind::ResponseEmitted, TraceCorrelation::default());
        Ok(reply)
    }

    fn history(&self, runtime: &Runtime) -> Result<Vec<ConversationMessage>, RuntimeError> {
        let records = runtime.store().recent_conversation(
            self.individual_id,
            &self.source_id,
            HISTORY_MESSAGES,
        )?;
        let mut bytes = 0;
        let mut messages = Vec::new();
        for record in records.into_iter().rev() {
            let Some(text) = record.payload.get("text").and_then(|v| v.as_str()) else {
                continue;
            };
            if bytes + text.len() > HISTORY_BYTES {
                break;
            }
            bytes += text.len();
            messages.push(ConversationMessage {
                evidence_id: record.evidence_id,
                role: if record.kind == EvidenceKind::UserUtterance {
                    ConversationRole::User
                } else {
                    ConversationRole::Assistant
                },
                text: text.to_owned(),
            });
        }
        messages.reverse();
        Ok(messages)
    }

    fn append(
        &self,
        runtime: &Runtime,
        turn_id: TurnId,
        kind: EvidenceKind,
        origin_class: OriginClass,
        payload: serde_json::Value,
    ) -> Result<EvidenceId, RuntimeError> {
        let evidence_id = EvidenceId::generate(runtime.ids().as_ref());
        let digest = json_digest(&payload);
        runtime.store().append(NewEvidence {
            evidence_id,
            individual_id: self.individual_id,
            session_id: Some(self.session_id),
            turn_id: Some(turn_id),
            kind,
            origin_class,
            payload,
            source: EvidenceSource {
                source_id: Some(self.source_id.clone()),
                source_sequence: None,
                content_digest: Some(digest.clone()),
            },
            retention_class: RetentionClass::Standard,
        })?;
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
}
