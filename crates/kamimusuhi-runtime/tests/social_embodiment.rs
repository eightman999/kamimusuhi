//! Runtime tests for Kamimusuhi Social Embodiment (v0.1):
//! - Two-phase propose -> verify_delivery persistence (no AgentUtterance before verify)
//! - Two-phase propose -> abort_proposal (failed delivery leaves no AgentUtterance)
//! - Initiative turns record SystemEvent observations rather than fake UserUtterance
//! - Public vs private Social Memory isolation and draft filtering

use kamimusuhi_core::evidence::{
    EvidenceKind, EvidenceSource, EvidenceStore, NewEvidence, RetentionClass,
};
use kamimusuhi_core::ids::{EvidenceId, ProposalId};
use kamimusuhi_core::memory::{MemoryQuery, MemoryRepository, MemoryUsage};
use kamimusuhi_core::mutation::{
    MutationDomain, MutationOperation, MutationPolicyV0, MutationProposal, OriginClass,
};
use kamimusuhi_core::persona::{ConversationRole, ProposalDraft};
use kamimusuhi_core::routing::PrivacyConstraint;
use kamimusuhi_runtime::dialogue::DialogueSession;
use kamimusuhi_runtime::llm_jev::{MockLanguageProvider, RuleBasedDecisionProvider};
use kamimusuhi_runtime::social::{
    DeliveryVerification, InitiativeInput, SocialContext, SocialVisibility, TurnSocialOptions,
    abort_proposal, filter_social_drafts, list_pending_proposals, verify_delivery,
};
use kamimusuhi_runtime::{ResourceImplementation, Runtime, RuntimeOptions};

fn init_runtime(seed: u64) -> (tempfile::TempDir, Runtime) {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Runtime::init(
        dir.path(),
        RuntimeOptions::deterministic(seed),
        ResourceImplementation::FakeA,
    )
    .unwrap();
    (dir, runtime)
}

#[test]
fn deferred_delivery_waits_for_verify_before_persisting_utterance_or_memories() {
    let (_dir, mut runtime) = init_runtime(101);
    let mut session = DialogueSession::start_with_providers(
        &mut runtime,
        "eightman",
        PrivacyConstraint::LocalOnly,
        Box::new(RuleBasedDecisionProvider),
        Some(Box::new(MockLanguageProvider)),
    )
    .unwrap();

    let opts = TurnSocialOptions {
        defer_delivery: true,
        social_context: Some(SocialContext {
            platform: "discord".to_owned(),
            guild_id: Some(10),
            channel_id: 30,
            message_id: Some(1001),
            user_id: Some(20),
            reply_to_message_id: None,
            visibility: SocialVisibility::Private,
            is_operator: true,
        }),
        initiative: None,
    };

    let reply = session
        .turn_with_social(&mut runtime, "こんにちは、澪", opts, |_| Ok(()))
        .unwrap();

    assert_eq!(
        reply.delivery_state.as_deref(),
        Some("proposed_pending_verification")
    );
    assert!(reply.response_digest.is_some());
    assert!(reply.c0.is_none());

    // Proposal file exists; no AgentUtterance exists yet.
    let pending = list_pending_proposals(&runtime).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].turn_id, reply.turn_id);
    let turn_records = runtime
        .store()
        .evidence_for_turn(runtime.individual_id(), reply.turn_id)
        .unwrap();
    assert!(
        turn_records
            .iter()
            .any(|r| r.kind == EvidenceKind::UserUtterance)
    );
    assert!(
        !turn_records
            .iter()
            .any(|r| r.kind == EvidenceKind::AgentUtterance)
    );
    let before_history = session.history_for_display(&runtime).unwrap();
    assert!(
        before_history
            .iter()
            .all(|m| m.role != ConversationRole::Assistant)
    );

    // Verify delivery commits AgentUtterance, writeback_verified, and cleans up proposal file.
    let mut writer = None;
    let verification = DeliveryVerification {
        discord_message_id: 5001,
        channel_id: 30,
        guild_id: Some(10),
        reply_to_message_id: Some(1001),
        delivered_text_digest: reply.response_digest.clone(),
        visibility: Some(SocialVisibility::Private),
    };
    let persisted =
        verify_delivery(&mut runtime, reply.turn_id, &verification, &mut writer).unwrap();
    assert_eq!(persisted["state"], serde_json::json!("persisted"));
    assert!(list_pending_proposals(&runtime).unwrap().is_empty());

    let turn_records = runtime
        .store()
        .evidence_for_turn(runtime.individual_id(), reply.turn_id)
        .unwrap();
    let agent = turn_records
        .iter()
        .find(|r| r.kind == EvidenceKind::AgentUtterance)
        .expect("AgentUtterance persisted after verify");
    assert_eq!(
        agent.payload["delivery"],
        serde_json::json!("discord_gateway_verified")
    );
    assert_eq!(
        agent.payload["discord"]["message_id"],
        serde_json::json!(5001)
    );
    assert!(turn_records.iter().any(|r| {
        r.kind == EvidenceKind::SystemEvent
            && r.payload.get("event") == Some(&serde_json::json!("memory_writeback_verified"))
    }));
    assert!(
        session
            .history_for_display(&runtime)
            .unwrap()
            .iter()
            .any(|m| m.role == ConversationRole::Assistant)
    );

    // Idempotent retry succeeds without duplicating AgentUtterance.
    let retry = verify_delivery(&mut runtime, reply.turn_id, &verification, &mut writer).unwrap();
    assert_eq!(retry["idempotent"], serde_json::json!(true));
    assert_eq!(
        runtime
            .store()
            .evidence_for_turn(runtime.individual_id(), reply.turn_id)
            .unwrap()
            .iter()
            .filter(|r| r.kind == EvidenceKind::AgentUtterance)
            .count(),
        1
    );
}

#[test]
fn aborted_proposal_records_system_event_without_agent_utterance() {
    let (_dir, mut runtime) = init_runtime(102);
    let mut session = DialogueSession::start_with_providers(
        &mut runtime,
        "eightman",
        PrivacyConstraint::LocalOnly,
        Box::new(RuleBasedDecisionProvider),
        Some(Box::new(MockLanguageProvider)),
    )
    .unwrap();

    let reply = session
        .turn_with_social(
            &mut runtime,
            "送信失敗テスト",
            TurnSocialOptions {
                defer_delivery: true,
                social_context: Some(SocialContext {
                    platform: "discord".to_owned(),
                    guild_id: Some(10),
                    channel_id: 30,
                    message_id: Some(1002),
                    user_id: Some(21),
                    reply_to_message_id: None,
                    visibility: SocialVisibility::Private,
                    is_operator: false,
                }),
                initiative: None,
            },
            |_| Ok(()),
        )
        .unwrap();

    let aborted = abort_proposal(&mut runtime, reply.turn_id, "discord_send_failed").unwrap();
    assert_eq!(aborted["state"], serde_json::json!("aborted"));
    assert!(list_pending_proposals(&runtime).unwrap().is_empty());

    let records = runtime
        .store()
        .evidence_for_turn(runtime.individual_id(), reply.turn_id)
        .unwrap();
    assert!(
        !records
            .iter()
            .any(|r| r.kind == EvidenceKind::AgentUtterance)
    );
    assert!(
        session
            .history_for_display(&runtime)
            .unwrap()
            .iter()
            .all(|m| m.role != ConversationRole::Assistant)
    );
    assert!(records.iter().any(|r| {
        r.kind == EvidenceKind::SystemEvent
            && r.payload.get("event") == Some(&serde_json::json!("social_proposal_aborted"))
    }));
    // Non-operator utterance is recorded with Ephemeral retention.
    let user_rec = records
        .iter()
        .find(|r| r.kind == EvidenceKind::UserUtterance)
        .unwrap();
    assert_eq!(user_rec.retention_class, RetentionClass::Ephemeral);
}

#[test]
fn initiative_turn_records_system_event_instead_of_fake_user_utterance() {
    let (_dir, mut runtime) = init_runtime(103);
    let mut session = DialogueSession::start_with_providers(
        &mut runtime,
        "eightman",
        PrivacyConstraint::LocalOnly,
        Box::new(RuleBasedDecisionProvider),
        Some(Box::new(MockLanguageProvider)),
    )
    .unwrap();

    let reply = session
        .turn_with_social(
            &mut runtime,
            "目標の進捗を簡潔に報告してください",
            TurnSocialOptions {
                defer_delivery: true,
                social_context: Some(SocialContext {
                    platform: "discord".to_owned(),
                    guild_id: Some(10),
                    channel_id: 30,
                    message_id: None,
                    user_id: None,
                    reply_to_message_id: None,
                    visibility: SocialVisibility::Private,
                    is_operator: true,
                }),
                initiative: Some(InitiativeInput {
                    trigger_id: "commit:c1:done".to_owned(),
                    trigger_kind: "commitment_outcome".to_owned(),
                    reason: "目標 c1 が完了したため".to_owned(),
                    source_ref: "commitment:c1".to_owned(),
                    evidence_summary: "検証成功".to_owned(),
                }),
            },
            |_| Ok(()),
        )
        .unwrap();

    let records_before = runtime
        .store()
        .evidence_for_turn(runtime.individual_id(), reply.turn_id)
        .unwrap();
    assert!(
        !records_before
            .iter()
            .any(|r| r.kind == EvidenceKind::UserUtterance)
    );
    assert!(records_before.iter().any(|r| {
        r.kind == EvidenceKind::SystemEvent
            && r.payload.get("event") == Some(&serde_json::json!("social_initiative_observed"))
    }));

    let mut writer = None;
    verify_delivery(
        &mut runtime,
        reply.turn_id,
        &DeliveryVerification {
            discord_message_id: 6001,
            channel_id: 30,
            guild_id: Some(10),
            reply_to_message_id: None,
            delivered_text_digest: reply.response_digest.clone(),
            visibility: Some(SocialVisibility::Private),
        },
        &mut writer,
    )
    .unwrap();

    let records_after = runtime
        .store()
        .evidence_for_turn(runtime.individual_id(), reply.turn_id)
        .unwrap();
    assert!(
        records_after
            .iter()
            .any(|r| r.kind == EvidenceKind::AgentUtterance)
    );
    assert!(records_after.iter().any(|r| {
        r.kind == EvidenceKind::SystemEvent
            && r.payload.get("event") == Some(&serde_json::json!("memory_writeback_verified"))
            && r.payload.get("initiative") == Some(&serde_json::json!(true))
    }));
}

#[test]
fn public_channel_scopes_out_relationship_and_private_memories() {
    let (_dir, mut runtime) = init_runtime(104);
    let individual_id = runtime.individual_id();
    let writer = runtime.claim_writer().unwrap();
    let head = runtime.head().unwrap();

    let private_ev = EvidenceId::from_u128(0xABCD01);
    runtime
        .store()
        .append(NewEvidence {
            evidence_id: private_ev,
            individual_id,
            session_id: None,
            turn_id: None,
            kind: EvidenceKind::UserUtterance,
            origin_class: OriginClass::Reported,
            payload: serde_json::json!({
                "text": "秘密の好み: 紅茶",
                "discord": {"visibility": "private", "channel_id": 30, "user_id": 20, "is_operator": true}
            }),
            source: EvidenceSource {
                source_id: Some("text-chat:eightman".to_owned()),
                source_sequence: Some(0),
                content_digest: None,
            },
            retention_class: RetentionClass::Standard,
        })
        .unwrap();

    // Activate a Relationship record citing private_ev
    runtime
        .kernel()
        .submit(&MutationProposal {
            proposal_id: ProposalId::from_u128(0xABCD02),
            individual_id,
            domain: MutationDomain::Relationship,
            operation: MutationOperation::Fact,
            subject_key: Some("eightman".to_owned()),
            candidate: serde_json::json!({"preference": "紅茶"}),
            expected_head: head.expected(),
            evidence_refs: vec![private_ev],
            supersedes: None,
            origin_class: OriginClass::Reported,
            requested_by: writer,
            policy_version: MutationPolicyV0::VERSION,
            idempotency_key: "rel-private-1".to_owned(),
            created_at: runtime.now(),
        })
        .unwrap();
    let rel_records = MemoryRepository::retrieve(
        runtime.store(),
        &MemoryQuery::current(individual_id)
            .in_domain(MutationDomain::Relationship)
            .about("eightman")
            .limited(8),
    )
    .unwrap();
    let rel_id = rel_records[0].record.state_record_id;
    runtime.store().mark_used(&[rel_id], runtime.now());

    // Activate an Episodic record citing private_ev
    let head2 = runtime.head().unwrap();
    runtime
        .kernel()
        .submit(&MutationProposal {
            proposal_id: ProposalId::from_u128(0xABCD03),
            individual_id,
            domain: MutationDomain::Episodic,
            operation: MutationOperation::Capture,
            subject_key: Some("eightman".to_owned()),
            candidate: serde_json::json!({"summary": "紅茶の話"}),
            expected_head: head2.expected(),
            evidence_refs: vec![private_ev],
            supersedes: None,
            origin_class: OriginClass::Reported,
            requested_by: writer,
            policy_version: MutationPolicyV0::VERSION,
            idempotency_key: "ep-private-1".to_owned(),
            created_at: runtime.now(),
        })
        .unwrap();

    let mut session = DialogueSession::start_with_providers(
        &mut runtime,
        "eightman",
        PrivacyConstraint::LocalOnly,
        Box::new(RuleBasedDecisionProvider),
        Some(Box::new(MockLanguageProvider)),
    )
    .unwrap();
    session.set_debug_context(true);

    let public_reply = session
        .turn_with_social(
            &mut runtime,
            "紅茶について公開チャンネルで話す",
            TurnSocialOptions {
                defer_delivery: true,
                social_context: Some(SocialContext {
                    platform: "discord".to_owned(),
                    guild_id: Some(10),
                    channel_id: 99,
                    message_id: Some(2001),
                    user_id: Some(22),
                    reply_to_message_id: None,
                    visibility: SocialVisibility::Public,
                    is_operator: false,
                }),
                initiative: None,
            },
            |_| Ok(()),
        )
        .unwrap();

    assert_eq!(public_reply.relationship_recalled, 0);
    assert_eq!(public_reply.episodic_recalled, 0);
    assert_eq!(public_reply.history_messages, 0);
    assert_eq!(
        public_reply.social_memory.as_ref().unwrap()["visibility"],
        serde_json::json!("public")
    );
    let ctx = public_reply.debug_context.unwrap();
    assert_eq!(ctx["relationship_memories"], serde_json::json!([]));
    assert_eq!(ctx["episodic_memories"], serde_json::json!([]));

    // filter_social_drafts drops Relationship drafts for non-operator public turns and profiling drafts.
    let drafts = vec![
        ProposalDraft {
            domain: MutationDomain::Relationship,
            operation: MutationOperation::Fact,
            subject_key: Some("someone".to_owned()),
            candidate: serde_json::json!({"trait": "kind"}),
            evidence_refs: vec![private_ev],
            supersedes: None,
            origin_class: OriginClass::Reported,
        },
        ProposalDraft {
            domain: MutationDomain::Episodic,
            operation: MutationOperation::Capture,
            subject_key: Some("eightman".to_owned()),
            candidate: serde_json::json!({"relationship_graph": {"a": "b"}}),
            evidence_refs: vec![private_ev],
            supersedes: None,
            origin_class: OriginClass::Reported,
        },
        ProposalDraft {
            domain: MutationDomain::Episodic,
            operation: MutationOperation::Capture,
            subject_key: Some("eightman".to_owned()),
            candidate: serde_json::json!({"topic": "public release"}),
            evidence_refs: vec![private_ev],
            supersedes: None,
            origin_class: OriginClass::Reported,
        },
    ];
    let filtered = filter_social_drafts(
        drafts,
        Some(&SocialContext {
            platform: "discord".to_owned(),
            guild_id: Some(10),
            channel_id: 99,
            message_id: None,
            user_id: Some(22),
            reply_to_message_id: None,
            visibility: SocialVisibility::Public,
            is_operator: false,
        }),
    );
    assert_eq!(filtered.len(), 1);
    assert_eq!(
        filtered[0].candidate["topic"],
        serde_json::json!("public release")
    );
}
