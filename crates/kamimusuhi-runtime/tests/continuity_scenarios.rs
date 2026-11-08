//! The five continuity contracts: four live-runtime failure/restart scenarios
//! here, plus paired behavior comparison in continuity_eval.rs. No paid model.
use kamimusuhi_core::c0::{ImprovementKind, ImprovementProposal, ProposalStatus, SelfField};
use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceRelation, EvidenceSource, EvidenceStore, NewEvidence, NewEvidenceLink,
    RetentionClass,
};
use kamimusuhi_core::ids::{C0ProposalId, EvidenceId};
use kamimusuhi_core::mutation::OriginClass;
use kamimusuhi_runtime::dialogue::DialogueSession;
use kamimusuhi_runtime::llm_jev::{
    ConversationError, LanguageProvider, LanguageRequest, LanguageResult,
};
use kamimusuhi_runtime::llm_jev::{MockLanguageProvider, RuleBasedDecisionProvider};
use kamimusuhi_runtime::{
    ResourceImplementation, Runtime, RuntimeOptions, c0, memory_writeback, self_context,
};
use serde_json::{Value, json};

fn init(path: &std::path::Path) -> Runtime {
    Runtime::init(
        path,
        RuntimeOptions::deterministic(0xC017),
        ResourceImplementation::FakeA,
    )
    .unwrap()
}
fn session(runtime: &mut Runtime) -> DialogueSession {
    DialogueSession::start_with_providers(
        runtime,
        "alice",
        Default::default(),
        Box::new(RuleBasedDecisionProvider),
        Some(Box::new(ContextReader)),
    )
    .unwrap()
}
/// Deterministic consumer of the actual language input, not an LLM competence claim.
struct ContextReader;
impl LanguageProvider for ContextReader {
    fn descriptor(&self) -> kamimusuhi_core::persona::PersonaBackendDescriptor {
        MockLanguageProvider.descriptor()
    }
    fn generate(&self, request: &LanguageRequest) -> Result<LanguageResult, ConversationError> {
        let mut result = MockLanguageProvider.generate(request)?;
        let envelope = &request.persona_input.envelope;
        result.persona.response_intent=format!("確認した自己記憶: {}",serde_json::to_string(&json!({"input":request.persona_input.input.text,"core":envelope.durable_self,"notes":envelope.self_notes,"history":envelope.conversation_history,"recall":envelope.recalled_evidence,"goals":envelope.continuity_context.as_ref().map(|v|&v["active_goals"])})).unwrap());
        Ok(result)
    }
}
fn source(runtime: &Runtime, kind: EvidenceKind, text: &str) -> EvidenceId {
    let id = EvidenceId::generate(runtime.ids().as_ref());
    runtime
        .store()
        .append(NewEvidence {
            evidence_id: id,
            individual_id: runtime.individual_id(),
            session_id: None,
            turn_id: None,
            kind,
            origin_class: OriginClass::Observed,
            payload: json!({"text":text}),
            source: EvidenceSource::default(),
            retention_class: RetentionClass::Standard,
        })
        .unwrap();
    id
}
fn propose(
    runtime: &Runtime,
    field: &str,
    key: &str,
    value: Value,
    refs: Vec<EvidenceId>,
) -> C0ProposalId {
    let id = C0ProposalId::generate(runtime.ids().as_ref());
    let view = c0::operative(runtime).unwrap().view;
    let old = view
        .self_model
        .fields
        .get(field)
        .and_then(|es| es.iter().find(|e| e.key.as_deref() == Some(key)))
        .map(|e| e.value.clone());
    runtime
        .store()
        .c0_insert_proposal(&ImprovementProposal {
            proposal_id: id,
            individual_id: runtime.individual_id(),
            kind: ImprovementKind::SelfUpdate,
            target: field.to_owned(),
            target_key: Some(key.to_owned()),
            old_value: old,
            proposed_value: value,
            evidence_refs: refs,
            expected_effect: None,
            risk: None,
            confidence: Some(0.8),
            status: ProposalStatus::Pending,
            rejection_reason: None,
            reflection_id: None,
            created_at: runtime.now(),
            decided_at: None,
            activation_seq: None,
        })
        .unwrap();
    id
}
fn activate(runtime: &Runtime, field: &str, key: &str, value: Value, refs: Vec<EvidenceId>) {
    let id = propose(runtime, field, key, value, refs);
    c0::activate(runtime, id, None, None).unwrap();
}

#[test]
fn continuity_01_corrected_root_stops_old_self_claim_on_the_next_actual_turn() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());
    let root = source(
        &runtime,
        EvidenceKind::AgentUtterance,
        "I can finish the task without access",
    );
    let derived = source(&runtime, EvidenceKind::Reflection, "I have access");
    runtime
        .store()
        .link(NewEvidenceLink {
            from_evidence_id: derived,
            to_evidence_id: root,
            relation: EvidenceRelation::DerivedFrom,
        })
        .unwrap();
    activate(
        &runtime,
        "capabilities",
        "access",
        json!("OLD_UNSUPPORTED_ACCESS"),
        vec![derived],
    );
    kamimusuhi_runtime::self_notes::append(
        dir.path(),
        kamimusuhi_core::self_note::SelfNoteKind::Lesson,
        "OLD_UNSUPPORTED_ACCESS",
        vec![derived.to_string()],
        runtime.now(),
    )
    .unwrap();
    let mut chat = session(&mut runtime);
    let reply = chat
        .turn(&mut runtime, "今できる作業を教えて", |_| Ok(()))
        .unwrap();
    assert!(reply.response.contains("OLD_UNSUPPORTED_ACCESS"));
    assert!(
        chat.last_context().unwrap()["self_state"]
            .to_string()
            .contains("OLD_UNSUPPORTED_ACCESS")
    );
    let correction = source(&runtime, EvidenceKind::Correction, "Access claim was wrong");
    runtime
        .store()
        .link(NewEvidenceLink {
            from_evidence_id: correction,
            to_evidence_id: root,
            relation: EvidenceRelation::Corrects,
        })
        .unwrap();
    let reply = chat
        .turn(&mut runtime, "今できる作業を教えて", |_| Ok(()))
        .unwrap();
    assert!(!reply.response.contains("OLD_UNSUPPORTED_ACCESS"));
    assert!(
        !chat.last_context().unwrap()["recent_context"]
            .to_string()
            .contains("OLD_UNSUPPORTED_ACCESS")
    );
    assert!(
        c0::recall_evidence(
            runtime.store(),
            runtime.individual_id(),
            "text-chat:alice",
            "OLD_UNSUPPORTED_ACCESS",
            &c0::operative(&runtime).unwrap().view.params,
            &Default::default()
        )
        .unwrap()
        .iter()
        .all(|r| !r.payload.to_string().contains("OLD_UNSUPPORTED_ACCESS"))
    );
    let ctx = chat.last_context().unwrap();
    assert!(
        !ctx["self_state"]
            .to_string()
            .contains("OLD_UNSUPPORTED_ACCESS")
    );
    assert_eq!(
        ctx["continuity_context"]["self_projection"]["excluded"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        c0::operative(&runtime)
            .unwrap()
            .view
            .self_model
            .entry_count(),
        1,
        "history retained"
    );
    let stale = propose(
        &runtime,
        "capabilities",
        "again",
        json!("RESURRECTED"),
        vec![derived],
    );
    assert!(c0::activate(&runtime, stale, None, None).is_err());
    drop(chat);
    drop(runtime);
    let mut reopened = Runtime::open(dir.path(), RuntimeOptions::deterministic(0xC018)).unwrap();
    let mut chat = session(&mut reopened);
    chat.turn(&mut reopened, "今できる作業を教えて", |_| Ok(()))
        .unwrap();
    assert!(
        !chat.last_context().unwrap()["self_state"]
            .to_string()
            .contains("OLD_UNSUPPORTED_ACCESS")
    );
}

#[cfg(unix)]
#[test]
fn writeback_monitor_does_not_report_a_dangling_symlink_as_healthy() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = init(dir.path());
    std::os::unix::fs::symlink(
        dir.path().join("missing"),
        dir.path().join("memory-writeback"),
    )
    .unwrap();
    assert!(memory_writeback::status(&runtime).is_err());
}

#[test]
fn corrected_history_only_echo_chain_cannot_reenter_the_next_language_context() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());
    let mut chat = session(&mut runtime);
    let original = chat
        .turn(&mut runtime, "OLD_HISTORY_ONLY_CLAIM", |_| Ok(()))
        .unwrap();
    let echo = chat.turn(&mut runtime, "もう一度", |_| Ok(())).unwrap();
    assert!(echo.response.contains("OLD_HISTORY_ONLY_CLAIM"));
    let correction = source(
        &runtime,
        EvidenceKind::Correction,
        "The original claim was wrong",
    );
    runtime
        .store()
        .link(NewEvidenceLink {
            from_evidence_id: correction,
            to_evidence_id: original.input_evidence_id,
            relation: EvidenceRelation::Corrects,
        })
        .unwrap();
    let next = chat
        .turn(&mut runtime, "今の情報を確認して", |_| Ok(()))
        .unwrap();
    assert!(!next.response.contains("OLD_HISTORY_ONLY_CLAIM"));
    assert!(
        !chat.last_context().unwrap()["recent_context"]
            .to_string()
            .contains("OLD_HISTORY_ONLY_CLAIM")
    );
}

#[test]
fn failed_lineage_write_cannot_leave_an_unlinked_emitted_answer_in_history() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());
    let mut chat = session(&mut runtime);
    let sql = rusqlite::Connection::open(dir.path().join("kamimusuhi.sqlite")).unwrap();
    sql.execute_batch("CREATE TRIGGER injected_link_stop BEFORE INSERT ON evidence_links WHEN NEW.relation = 'derived_from' BEGIN SELECT RAISE(FAIL,'injected link stop'); END;").unwrap();
    let mut delivered = false;
    assert!(
        chat.turn(&mut runtime, "OLD_PARTIAL_LINK", |_| {
            delivered = true;
            Ok(())
        })
        .is_err()
    );
    assert!(delivered);
    let agents: i64 = sql
        .query_row(
            "SELECT count(*) FROM evidence_records WHERE kind='agent_utterance'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        agents, 0,
        "emitted record and links must roll back together"
    );
    assert_eq!(memory_writeback::status(&runtime).unwrap()["unresolved"], 1);
}

#[test]
fn continuity_02_restart_keeps_promises_in_always_present_index_without_keyword_match() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = init(dir.path());
    let evidence = source(
        &runtime,
        EvidenceKind::AgentUtterance,
        "I will verify the backup before release",
    );
    activate(
        &runtime,
        "active_goals",
        "release",
        json!("VERIFY_BACKUP_BEFORE_RELEASE"),
        vec![evidence],
    );
    let individual = runtime.individual_id();
    drop(runtime);
    // Independent executable reads the same persisted individual.
    let child = std::process::Command::new(env!("CARGO_BIN_EXE_kamimusuhi-runtime"))
        .args([
            "memory",
            "--dir",
            dir.path().to_str().unwrap(),
            "--subject",
            "alice",
        ])
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "{}",
        String::from_utf8_lossy(&child.stderr)
    );
    let report: Value = serde_json::from_slice(&child.stdout).unwrap();
    assert!(
        report["self_model"]
            .to_string()
            .contains("VERIFY_BACKUP_BEFORE_RELEASE")
    );
    let mut runtime = Runtime::open(dir.path(), RuntimeOptions::deterministic(0xC019)).unwrap();
    assert_eq!(runtime.individual_id(), individual);
    let mut chat = session(&mut runtime);
    let reply = chat.turn(&mut runtime, "こんにちは", |_| Ok(())).unwrap();
    let index = &chat.last_context().unwrap()["continuity_context"];
    assert!(
        index["active_goals"]
            .to_string()
            .contains("VERIFY_BACKUP_BEFORE_RELEASE")
    );
    assert!(reply.response.contains("VERIFY_BACKUP_BEFORE_RELEASE"));
    assert_eq!(
        index["prior_writeback"]["unresolved"], 0,
        "current turn is not a previous persistence failure"
    );
    assert_eq!(index["self_projection"]["surfaced_entries"], 1);
    assert_eq!(reply.c0.unwrap().memory_writeback["admission"], "no_drafts");
}

#[test]
fn continuity_03_self_update_cannot_drop_lessons_and_keeps_prior_entries_in_context() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());
    let evidence = source(
        &runtime,
        EvidenceKind::Reflection,
        "Lessons from observed work",
    );
    activate(
        &runtime,
        "values",
        "work",
        json!(["QUOTE_SOURCE", "VERIFY_LANDING"]),
        vec![evidence],
    );
    activate(
        &runtime,
        "limitations",
        "uncertainty",
        json!("Report unverified claims"),
        vec![evidence],
    );
    let before = c0::operative(&runtime).unwrap();
    let bad = propose(
        &runtime,
        "values",
        "work",
        json!(["QUOTE_SOURCE"]),
        vec![evidence],
    );
    assert!(c0::activate(&runtime, bad, None, None).is_err());
    assert_eq!(c0::operative(&runtime).unwrap().view, before.view);
    activate(
        &runtime,
        "values",
        "work",
        json!(["QUOTE_SOURCE", "VERIFY_LANDING", "CHECK_ORIGIN"]),
        vec![evidence],
    );
    let mut chat = session(&mut runtime);
    chat.turn(&mut runtime, "次の作業へ進めて", |_| Ok(()))
        .unwrap();
    let core = chat.last_context().unwrap()["self_state"].to_string();
    for text in [
        "QUOTE_SOURCE",
        "VERIFY_LANDING",
        "CHECK_ORIGIN",
        "Report unverified claims",
    ] {
        assert!(core.contains(text));
    }
    assert_eq!(
        c0::operative(&runtime)
            .unwrap()
            .view
            .self_model
            .field(SelfField::Limitations)
            .len(),
        1
    );
}

#[test]
fn continuity_04_delivered_response_with_failed_evidence_write_is_detected_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let mut runtime = init(dir.path());
    let sql = rusqlite::Connection::open(dir.path().join("kamimusuhi.sqlite")).unwrap();
    sql.execute_batch("CREATE TRIGGER injected_writeback_stop BEFORE INSERT ON evidence_records WHEN NEW.kind = 'agent_utterance' BEGIN SELECT RAISE(FAIL, 'injected writeback stop'); END;").unwrap();
    let mut chat = session(&mut runtime);
    let mut delivered = false;
    assert!(
        chat.turn(&mut runtime, "こんにちは", |_| {
            delivered = true;
            Ok(())
        })
        .is_err()
    );
    assert!(delivered, "surface acceptance must differ from persistence");
    assert_eq!(memory_writeback::status(&runtime).unwrap()["unresolved"], 1);
    drop(chat);
    drop(runtime);
    sql.execute_batch("DROP TRIGGER injected_writeback_stop;")
        .unwrap();
    drop(sql);
    let mut runtime = Runtime::open(dir.path(), RuntimeOptions::deterministic(0xC020)).unwrap();
    let report = kamimusuhi_runtime::inspect::memory_report(&runtime, Some("alice"), 10).unwrap();
    assert_eq!(
        report["memory_writeback"]["state"],
        "incomplete_or_in_flight"
    );
    let mut chat = session(&mut runtime);
    let reply = chat.turn(&mut runtime, "こんにちは", |_| Ok(())).unwrap();
    assert_eq!(
        reply.c0.unwrap().memory_writeback["event"],
        "memory_writeback_verified"
    );
    assert_eq!(
        memory_writeback::status(&runtime).unwrap()["unresolved"],
        1,
        "later success cannot erase the failed turn"
    );
}

#[test]
fn legacy_over_budget_self_projection_reports_omission_instead_of_silent_loss() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = init(dir.path());
    let evidence = source(&runtime, EvidenceKind::Reflection, "legacy evidence");
    let mut snapshot = kamimusuhi_core::c0::SelfSnapshot::default();
    for i in 0..30 {
        snapshot.apply(
            SelfField::Values,
            kamimusuhi_core::c0::SelfEntry {
                key: Some(format!("{i}")),
                value: json!("x".repeat(1500)),
                confidence: None,
                evidence_refs: vec![evidence],
                proposal_id: None,
            },
        );
    }
    let projection = self_context::prepare(&runtime, &snapshot).unwrap();
    assert!(!projection.audit["excluded"].as_array().unwrap().is_empty());
    assert_eq!(snapshot.entry_count(), 30);
    assert!(
        serde_json::to_vec(&projection.snapshot).unwrap().len()
            <= kamimusuhi_core::c0::SELF_SNAPSHOT_MAX_BYTES
    );
}
