//! `WorkspaceBuilder` assembling durable memory, Library material and a
//! resource result into one Global Workspace (issue #16 Wave 3, plan §10).
//!
//! The point of the module: after assembly, the three kinds of material stay
//! distinguishable by type — `WorkspaceDomain` and the `SourceRef` variant —
//! never by inspecting the item's text. Assembly is also read-only: it must
//! not move the continuity head or write anything durable.

mod common;

use std::sync::Arc;

use common::*;
use kamimusuhi_core::continuity::{ActivationOutcome, ContinuityStore};
use kamimusuhi_core::ids::{LibraryArtifactId, ResourceCallId};
use kamimusuhi_core::library::{
    LibraryMediaType, LibraryQuery, LibraryRepository, NewLibraryArtifact,
};
use kamimusuhi_core::memory::MemoryRepository;
use kamimusuhi_core::mutation::MutationDomain;
use kamimusuhi_core::resources::{ResourceRegistry, ResourceRequest, ResourceSlot};
use kamimusuhi_core::time::UtcTimestamp;
use kamimusuhi_core::workspace::{AuthorityClass, SourceRef, WorkspaceBuilder, WorkspaceDomain};
use kamimusuhi_testkit::FakeResource;
use kamimusuhi_testkit::fake_resource::FAKE_A_RESOURCE_ID;

const ARTIFACT: LibraryArtifactId = LibraryArtifactId::from_u128(0xF101);
const MARKDOWN: &str = "# お茶の淹れ方\n\nほうじ茶は高温で淹れる。\n";

fn assembled_at() -> UtcTimestamp {
    UtcTimestamp::from_unix_millis(2_000)
}

#[test]
fn one_workspace_keeps_memory_library_and_resource_material_distinguishable_by_type() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    let boot = bootstrap(&store, BOOT_A);
    let kernel = kernel(store);

    // Durable relationship memory, activated through the kernel.
    let proposal = relationship_fact(100, boot.head.expected(), boot.writer, "ほうじ茶");
    let ActivationOutcome::Activated(_) = kernel.submit(&proposal).unwrap() else {
        panic!("expected activation");
    };
    let head = kernel.store().load_head(INDIVIDUAL).unwrap();
    let memories = MemoryRepository::retrieve(
        kernel.store(),
        &kamimusuhi_core::memory::MemoryQuery::current(INDIVIDUAL)
            .in_domain(MutationDomain::Relationship),
    )
    .unwrap();
    assert_eq!(memories.len(), 1);

    // A Library document, imported and retrieved for one hit.
    kernel
        .store()
        .import(NewLibraryArtifact {
            artifact_id: ARTIFACT,
            source_uri: Some("file:///tea-notes.md".to_owned()),
            title: Some("お茶ノート".to_owned()),
            media_type: LibraryMediaType::Markdown,
            content: MARKDOWN.to_owned(),
        })
        .unwrap();
    let hits = LibraryRepository::retrieve(kernel.store(), &LibraryQuery::new("高温")).unwrap();
    assert_eq!(hits.len(), 1);

    // A resource call through Fake A.
    let mut registry = ResourceRegistry::new();
    let slot = ResourceSlot::new("reasoning");
    registry
        .register(slot.clone(), Arc::new(FakeResource::a()))
        .unwrap();
    let request = ResourceRequest::new(
        INDIVIDUAL,
        "summarize-turn",
        serde_json::json!({ "text": "お茶の話" }),
    );
    let attributed = registry
        .invoke(
            &slot,
            &request,
            kernel.store(),
            ResourceCallId::from_u128(1),
            UtcTimestamp::from_unix_millis(1_000),
            UtcTimestamp::from_unix_millis(1_500),
        )
        .unwrap();

    let head_before = head;
    let counts_before = row_counts(&db.path);

    let build = || {
        WorkspaceBuilder::new(INDIVIDUAL, assembled_at())
            .with_continuity(&head)
            .with_memories(&memories)
            .with_library_hits(&hits)
            .with_resource_results(std::slice::from_ref(&attributed))
            .build()
    };

    let workspace = build();

    // Determinism: same inputs, same workspace.
    assert_eq!(workspace, build());

    // Positions are 0..n-1 and domains come out in rank order.
    for (index, item) in workspace.items.iter().enumerate() {
        assert_eq!(item.position as usize, index);
    }
    assert_eq!(
        workspace.domains(),
        vec![
            WorkspaceDomain::CurrentContinuityState,
            WorkspaceDomain::RelationshipMemory,
            WorkspaceDomain::LibraryEvidence,
            WorkspaceDomain::ExternalResourceResult,
        ]
    );

    // The relationship item exposes state_record_id and evidence_refs.
    let relationship_items = workspace.items_in(WorkspaceDomain::RelationshipMemory);
    assert_eq!(relationship_items.len(), 1);
    let SourceRef::Memory {
        state_record_id,
        evidence_refs,
        ..
    } = &relationship_items[0].source_ref
    else {
        panic!("relationship memory must carry a Memory source ref");
    };
    assert_eq!(*state_record_id, memories[0].record.state_record_id);
    assert_eq!(evidence_refs, &memories[0].record.evidence_refs);
    assert_eq!(
        relationship_items[0].authority,
        AuthorityClass::CanonicalState
    );

    // The Library item exposes artifact_id, chunk_id and ordinal.
    let library_items = workspace.items_in(WorkspaceDomain::LibraryEvidence);
    assert_eq!(library_items.len(), 1);
    let SourceRef::Library {
        artifact_id,
        chunk_id,
        ordinal,
        ..
    } = &library_items[0].source_ref
    else {
        panic!("library hit must carry a Library source ref");
    };
    assert_eq!(*artifact_id, ARTIFACT);
    assert_eq!(*chunk_id, hits[0].chunk_id());
    assert_eq!(*ordinal, hits[0].chunk.ordinal);
    assert_eq!(library_items[0].authority, AuthorityClass::ExternalMaterial);

    // The resource item exposes resource_id and resource_call_id.
    let resource_items = workspace.items_in(WorkspaceDomain::ExternalResourceResult);
    assert_eq!(resource_items.len(), 1);
    let SourceRef::Resource {
        resource_id,
        resource_call_id,
    } = &resource_items[0].source_ref
    else {
        panic!("resource result must carry a Resource source ref");
    };
    assert_eq!(*resource_id, FAKE_A_RESOURCE_ID);
    assert_eq!(*resource_call_id, attributed.call.resource_call_id);
    assert_eq!(
        resource_items[0].authority,
        AuthorityClass::ExternalMaterial
    );

    // The continuity item is also canonical state.
    let continuity_items = workspace.items_in(WorkspaceDomain::CurrentContinuityState);
    assert_eq!(
        continuity_items[0].authority,
        AuthorityClass::CanonicalState
    );

    // No authority class in the assembled workspace authorizes a mutation.
    for item in &workspace.items {
        assert!(!item.authority.authorizes_mutation());
    }

    // Assembly moved nothing durable.
    let head_after = kernel.store().load_head(INDIVIDUAL).unwrap();
    let counts_after = row_counts(&db.path);
    assert_eq!(head_before, head_after);
    assert_eq!(counts_before, counts_after);
}
