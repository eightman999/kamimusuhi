//! Text interaction over the existing individual and canonical evidence.
//! Raw conversation is recallable, but never activated as a belief. Generated
//! text and a successful write to the output surface have separate records.

use std::collections::BTreeSet;
use std::time::Instant;

use kamimusuhi_core::continuity::WriterIdentity;
use kamimusuhi_core::digest::{content_digest, json_digest};
use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceSource, EvidenceStore, NewEvidence, NewSession, NewTurn, RetentionClass,
};
use kamimusuhi_core::ids::{EvidenceId, IndividualId, SessionId, TurnId};
use kamimusuhi_core::mutation::OriginClass;
use kamimusuhi_core::persona::{
    ConversationMessage, ConversationRole, CurrentInput, PersonaBackendDescriptor,
    PersonaTurnInput, SessionWorkingState, TurnContext,
};
use kamimusuhi_core::routing::PrivacyConstraint;
use kamimusuhi_core::trace::{TraceCorrelation, TraceEventKind};
use kamimusuhi_core::workspace::WorkspaceBuilder;
use serde::Serialize;

use crate::c0::{self, eval};
use crate::research::ResearchCatalog;
use crate::{Runtime, RuntimeError};

pub const MAX_INPUT_BYTES: usize = 8_192;
const HISTORY_BYTES: usize = 16_384;

/// Per-turn C0 bookkeeping, surfaced for inspection.
#[derive(Debug, Clone, Serialize)]
pub struct TurnC0 {
    /// Derived-lane head this turn ran under.
    pub activation_seq: u64,
    /// Memory items surfaced into the workspace.
    pub memories_surfaced: usize,
    /// Canonical drafts submitted and how many were activated.
    pub drafts_submitted: usize,
    pub drafts_activated: usize,
    /// Turn metrics recorded by the deterministic evaluator.
    pub metrics: kamimusuhi_core::c0::TurnMetrics,
}

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub c0: Option<TurnC0>,
    /// The assembled context exactly as the backend received it. Only
    /// populated when debug-context output was requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug_context: Option<serde_json::Value>,
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
    /// Writer epoch, claimed lazily on the first canonical draft submission.
    /// A conversation that only reads never takes the epoch.
    writer: Option<WriterIdentity>,
    /// Emit the assembled context on each turn (`--debug-context`).
    debug_context: bool,
    /// The last assembled workspace, for `/context` inspection.
    last_context: Option<serde_json::Value>,
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
            writer: None,
            debug_context: false,
            last_context: None,
        })
    }

    /// Surface the assembled workspace on each turn (`--debug-context`).
    pub fn set_debug_context(&mut self, enabled: bool) {
        self.debug_context = enabled;
    }

    /// The workspace assembled for the most recent turn, as inspectable JSON.
    pub fn last_context(&self) -> Option<&serde_json::Value> {
        self.last_context.as_ref()
    }

    pub fn session_id(&self) -> SessionId {
        self.session_id
    }

    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn turn_count(&self) -> u64 {
        self.sequence
    }

    /// The writer cache, for reflection cycles run inside this session.
    pub fn writer_cache(&mut self) -> &mut Option<WriterIdentity> {
        &mut self.writer
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
        // The operative view in force this turn: retrieval knobs, conversation
        // policy and the self model all come from the derived lane's head.
        let operative = c0::operative(runtime)?;
        let history = self.history(runtime, operative.view.params.retrieval.history_messages)?;
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
        // Retrieval is subject-bound everywhere: relationship and episodic
        // records are scoped to this interlocutor, and recalled evidence is
        // filtered to this channel's source id.
        let memories = c0::retrieve_memories(
            runtime.store(),
            self.individual_id,
            &self.subject,
            text,
            &operative.view.params,
        )?;
        let mut exclude: BTreeSet<EvidenceId> = history.iter().map(|m| m.evidence_id).collect();
        exclude.insert(input_evidence_id);
        let recalled_evidence = c0::recall_evidence(
            runtime.store(),
            self.individual_id,
            &self.source_id,
            text,
            &operative.view.params,
            &exclude,
        )?;
        let head = runtime.head()?;
        let workspace = WorkspaceBuilder::new(self.individual_id, runtime.now())
            .with_continuity(&head)
            .with_current_input(&current_input)
            .with_memories(&memories)
            .with_self_state(&operative.view.self_model)
            .with_active_policy(&operative.view.params, operative.activation_seq)
            .with_recalled_evidence(&recalled_evidence)
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
        input.envelope.conversation_history = history.clone();
        input.envelope.observed_runtime = Some(observed.clone());
        input.envelope.mio_observation = mio_context.clone();
        input.envelope.research_findings = Some(research_context.clone());
        let context_digest = json_digest(&serde_json::json!(input));
        let history_messages = input.envelope.conversation_history.len();
        // The inspectable context: what the model was actually shown, in the
        // shape the C0 spec asks `--debug-context` to expose.
        let workspace_context = serde_json::json!({
            "activation_seq": operative.activation_seq,
            "recent_context": input.envelope.conversation_history,
            "relationship_memories": input.envelope.relationship,
            "episodic_memories": input.envelope.episodic,
            "recalled_evidence": input.envelope.recalled_evidence,
            "self_state": input.envelope.durable_self,
            "active_policy": input.envelope.active_policy,
            "open_threads": self.open_threads(runtime, &operative.view)?,
            "body_state": input.envelope.body_state,
            "workspace_digest": workspace.digest(),
        });
        self.last_context = Some(workspace_context.clone());
        runtime.trace().record_with(TraceEventKind::PersonaInvoked, TraceCorrelation {
            persona_backend_id: Some(backend.backend_id), ..TraceCorrelation::default()
        }, serde_json::json!({"input_digest": context_digest, "history_messages": history_messages}));
        let result = persona.turn(input)?;
        // Persona drafts are proposals, not state: submit each through the
        // canonical kernel so the mutation policy decides. The writer epoch
        // is claimed lazily here — a turn with no drafts never takes it.
        let draft_outcomes = c0::submit_drafts(
            runtime,
            Some(self.session_id),
            Some(turn_id),
            &mut self.writer,
            result.proposals.clone(),
        )?;
        let drafts_activated = draft_outcomes.iter().filter(|o| o.activated).count();
        runtime.trace().record_with(
            TraceEventKind::PersonaCompleted,
            TraceCorrelation {
                persona_backend_id: Some(result.backend.backend_id),
                ..TraceCorrelation::default()
            },
            serde_json::json!({
                "draft_count": draft_outcomes.len(),
                "drafts_activated": drafts_activated,
            }),
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
            c0: None,
            debug_context: None,
        };
        emit(&reply)
            .map_err(|error| RuntimeError::Usage(format!("response output failed: {error}")))?;
        let emitted_evidence_id = self.append(
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
        // Deterministic per-turn metrics, recorded in the lane — the
        // reflection cycle reads them, and they are the before/after record.
        let metrics = eval::measure_turn(
            text,
            &reply.response,
            &history,
            &workspace,
            operative.view.params.conversation.max_response_chars,
        );
        runtime
            .store()
            .c0_record_evaluation(&kamimusuhi_store_sqlite::c0::C0Evaluation {
                evaluation_id: kamimusuhi_core::ids::C0EvaluationId::generate(
                    runtime.ids().as_ref(),
                ),
                individual_id: self.individual_id,
                scope: "turn".to_owned(),
                subject_key: Some(self.subject.clone()),
                turn_id: Some(turn_id),
                proposal_id: None,
                metrics: serde_json::to_value(&metrics).unwrap_or(serde_json::Value::Null),
                evaluator: kamimusuhi_core::c0::EVALUATOR_KIND.to_owned(),
                created_at: runtime.now(),
            })?;
        let _ = emitted_evidence_id;
        Ok(DialogueReply {
            c0: Some(TurnC0 {
                activation_seq: operative.activation_seq,
                memories_surfaced: metrics.memories_surfaced,
                drafts_submitted: draft_outcomes.len(),
                drafts_activated,
                metrics,
            }),
            debug_context: self.debug_context.then_some(workspace_context),
            ..reply
        })
    }

    /// Open threads for the inspectable context: pending derived-lane
    /// proposals plus the self model's open questions. Deterministic — read
    /// from state, not inferred from prose.
    fn open_threads(
        &self,
        runtime: &Runtime,
        view: &kamimusuhi_core::c0::OperativeView,
    ) -> Result<serde_json::Value, RuntimeError> {
        let pending = runtime.store().c0_proposals(
            self.individual_id,
            Some(kamimusuhi_core::c0::ProposalStatus::Pending),
        )?;
        let open_questions: Vec<serde_json::Value> = view
            .self_model
            .field(kamimusuhi_core::c0::SelfField::OpenQuestions)
            .iter()
            .map(|e| e.value.clone())
            .collect();
        Ok(serde_json::json!({
            "pending_proposals": pending
                .iter()
                .map(|p| serde_json::json!({"proposal_id": p.proposal_id, "target": p.target}))
                .collect::<Vec<_>>(),
            "open_questions": open_questions,
        }))
    }

    fn history(
        &self,
        runtime: &Runtime,
        limit: usize,
    ) -> Result<Vec<ConversationMessage>, RuntimeError> {
        let records = runtime.store().recent_conversation(
            self.individual_id,
            &self.source_id,
            limit.clamp(1, 128),
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
