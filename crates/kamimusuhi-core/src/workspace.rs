//! Global Workspace v0: a typed envelope, not a prompt builder.
//!
//! Everything Kamimusuhi is thinking with at one moment lands here — its own
//! durable memory, external Library material, the result of a resource call,
//! the current utterance. The point of the module is that they stay *told
//! apart* after they land: each item keeps its domain, its source IDs and its
//! authority class, so nothing has to parse text to know what it is looking
//! at (plan §10).
//!
//! Two consequences are load-bearing:
//!
//! - assembly is deterministic. Same inputs, same workspace, in the same
//!   order, so a cognition bug is reproducible.
//! - being in the workspace grants nothing. A workspace is not durable
//!   memory, is never persisted as canonical state, and does not advance the
//!   continuity head. Library text and resource output sit next to canonical
//!   memory without becoming it — see [`AuthorityClass`].

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::c0::{OperativeParams, SelfSnapshot};
use crate::continuity::ContinuityHead;
use crate::digest::json_digest;
use crate::evidence::{EvidenceKind, EvidenceRecord};
use crate::ids::{
    CommitId, EvidenceId, IndividualId, LibraryArtifactId, LibraryChunkId, MemoryId,
    ResourceCallId, ResourceId,
};
use crate::library::LibraryHit;
use crate::memory::AttributedMemory;
use crate::mutation::MutationDomain;
use crate::persona::CurrentInput;
use crate::resources::AttributedResult;
use crate::time::UtcTimestamp;

/// Where an item came from, as a domain rather than as prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WorkspaceDomain {
    /// The individual's lineage position. Context, never content.
    CurrentContinuityState,
    /// What the interlocutor just said.
    CurrentInput,
    /// Activated relationship state about another person.
    RelationshipMemory,
    /// Activated episodic state.
    EpisodicMemory,
    /// A retrieved Library chunk. External material.
    LibraryEvidence,
    /// The output of a cognitive resource call. External material.
    ExternalResourceResult,
    /// The individual's structured self model from the C0 derived lane:
    /// what was accepted about itself, with provenance. Derived state, not
    /// canonical — it arrives through an activation, never through a write.
    SelfMemory,
    /// Canonical utterance records recalled by retrieval. Raw first-party
    /// record, distinct from activated episodic *beliefs* about the past.
    RecalledEvidence,
    /// The operative policy in force for this turn, as an overlay. Derived
    /// lane content: it modulates behaviour but is not a belief.
    ActivePolicy,
}

impl WorkspaceDomain {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CurrentContinuityState => "CURRENT_CONTINUITY_STATE",
            Self::CurrentInput => "CURRENT_INPUT",
            Self::RelationshipMemory => "RELATIONSHIP_MEMORY",
            Self::EpisodicMemory => "EPISODIC_MEMORY",
            Self::LibraryEvidence => "LIBRARY_EVIDENCE",
            Self::ExternalResourceResult => "EXTERNAL_RESOURCE_RESULT",
            Self::SelfMemory => "SELF_MEMORY",
            Self::RecalledEvidence => "RECALLED_EVIDENCE",
            Self::ActivePolicy => "ACTIVE_POLICY",
        }
    }

    /// Fixed assembly rank. Ordering is part of the contract, so it lives
    /// here rather than in whatever happens to build a workspace.
    const fn rank(self) -> u8 {
        match self {
            Self::CurrentContinuityState => 0,
            Self::CurrentInput => 1,
            Self::RelationshipMemory => 2,
            Self::EpisodicMemory => 3,
            Self::LibraryEvidence => 4,
            Self::ExternalResourceResult => 5,
            // C0 additions keep the existing ranks stable: prompt order is
            // fixed by the envelope's section order, not by this rank.
            Self::SelfMemory => 6,
            Self::RecalledEvidence => 7,
            Self::ActivePolicy => 8,
        }
    }
}

impl fmt::Display for WorkspaceDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How much standing an item has. No class authorizes anything — the
/// distinction records what kind of thing this is, so that a later stage
/// cannot quietly treat imported text as the individual's own position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityClass {
    /// Activated canonical state of this individual, and its lineage.
    CanonicalState,
    /// First-party testimony from the current turn.
    DirectInput,
    /// Material from outside the individual: Library text, resource output.
    ExternalMaterial,
}

impl AuthorityClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CanonicalState => "canonical_state",
            Self::DirectInput => "direct_input",
            Self::ExternalMaterial => "external_material",
        }
    }

    /// Always false. Being in the workspace is not a permission: a mutation
    /// is judged by the policy against evidence provenance, never by what an
    /// item's class is.
    pub const fn authorizes_mutation(self) -> bool {
        false
    }
}

impl fmt::Display for AuthorityClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The stable IDs behind an item, typed by what kind of source it is.
///
/// A caller asks the variant, never the text: "is this Library material" is
/// answered by matching [`SourceRef::Library`], not by looking for a citation
/// marker inside a string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum SourceRef {
    Continuity {
        commit_id: CommitId,
        generation: u64,
    },
    Input {
        evidence_id: EvidenceId,
    },
    Memory {
        state_record_id: MemoryId,
        domain: MutationDomain,
        subject_key: Option<String>,
        /// Evidence the durable record rests on. Provenance survives the trip
        /// into the workspace.
        evidence_refs: Vec<EvidenceId>,
    },
    Library {
        artifact_id: LibraryArtifactId,
        chunk_id: LibraryChunkId,
        ordinal: u32,
        source_uri: Option<String>,
    },
    Resource {
        resource_id: ResourceId,
        resource_call_id: ResourceCallId,
    },
    /// An activated C0 self-model section, by field name.
    SelfState {
        field: String,
    },
    /// The operative parameter snapshot in force, identified by the
    /// activation that produced it. `0` means defaults — never activated.
    OperativePolicy {
        activation_seq: u64,
    },
    /// A raw canonical evidence record recalled by retrieval. Not a belief
    /// about the past — the record itself.
    Evidence {
        evidence_id: EvidenceId,
        kind: EvidenceKind,
    },
}

/// Why the assembler included an item. Deterministic and inspectable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InclusionReason {
    /// The turn's own input.
    DirectInput,
    /// Part of the current head, included as orientation.
    ContinuityContext,
    /// An active durable record retrieved for this turn.
    ActiveMemory,
    /// A chunk that matched the Library query.
    LibraryMatch,
    /// The result of a resource call made for this turn.
    ResourceInvocation,
    /// A self-model entry from the active derived-lane snapshot.
    SelfState,
    /// The operative parameters in force, surfaced as an overlay.
    OperativePolicy,
    /// A canonical record that matched this turn's recall query.
    EvidenceRecall,
}

impl InclusionReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DirectInput => "direct_input",
            Self::ContinuityContext => "continuity_context",
            Self::ActiveMemory => "active_memory",
            Self::LibraryMatch => "library_match",
            Self::ResourceInvocation => "resource_invocation",
            Self::SelfState => "self_state",
            Self::OperativePolicy => "operative_policy",
            Self::EvidenceRecall => "evidence_recall",
        }
    }
}

/// When the underlying thing came to exist, so a later stage can weigh age
/// without re-reading storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Freshness {
    /// Creation time of the source record, when it has one.
    pub source_time: Option<UtcTimestamp>,
    pub assembled_at: UtcTimestamp,
}

/// Item content. Structured where the source is structured; the text variant
/// exists for Library spans, which genuinely are text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "form", rename_all = "snake_case")]
pub enum WorkspaceContent {
    Structured { value: serde_json::Value },
    Text { text: String },
}

impl WorkspaceContent {
    pub fn structured(value: serde_json::Value) -> Self {
        Self::Structured { value }
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }
}

/// One attributed item in the workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceItem {
    /// Position in the assembled workspace, 0-based. The workspace is not
    /// durable, so an item is identified by its position and its source refs
    /// rather than by a minted ID that would only add non-determinism.
    pub position: u32,
    pub domain: WorkspaceDomain,
    pub source_ref: SourceRef,
    pub content: WorkspaceContent,
    pub authority: AuthorityClass,
    pub freshness: Freshness,
    pub inclusion_reason: InclusionReason,
}

impl WorkspaceItem {
    /// True when the item came from outside the individual.
    pub const fn is_external_material(&self) -> bool {
        matches!(self.authority, AuthorityClass::ExternalMaterial)
    }
}

/// Per-domain item caps. A fixed count, not a token optimizer: W3 needs the
/// assembly to be bounded and reproducible, nothing more (plan §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkspaceBudget {
    pub relationship_memory: usize,
    pub episodic_memory: usize,
    pub library: usize,
    pub external_resource: usize,
    /// One item per non-empty self-model field.
    pub self_memory: usize,
    /// Recalled raw utterance records.
    pub recalled_evidence: usize,
    /// The operative-policy overlay. One item suffices; the budget exists so
    /// the cap is an explicit choice rather than an accident.
    pub active_policy: usize,
}

impl Default for WorkspaceBudget {
    fn default() -> Self {
        Self {
            relationship_memory: 8,
            episodic_memory: 8,
            library: 4,
            external_resource: 4,
            self_memory: 16,
            recalled_evidence: 8,
            active_policy: 4,
        }
    }
}

/// One assembled workspace. Held in memory for the duration of a turn and
/// never written to canonical storage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub individual_id: IndividualId,
    pub assembled_at: UtcTimestamp,
    pub items: Vec<WorkspaceItem>,
}

impl Workspace {
    pub fn items_in(&self, domain: WorkspaceDomain) -> Vec<&WorkspaceItem> {
        self.items.iter().filter(|i| i.domain == domain).collect()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Stable digest of the assembled workspace.
    ///
    /// Lets a trace or a test say "the same workspace was built" without
    /// copying its contents anywhere. Two assemblies from identical inputs
    /// have identical digests, because assembly is a pure function.
    pub fn digest(&self) -> String {
        json_digest(&serde_json::to_value(self).unwrap_or(serde_json::Value::Null))
    }

    /// Domains present, in assembled order.
    pub fn domains(&self) -> Vec<WorkspaceDomain> {
        let mut seen = Vec::new();
        for item in &self.items {
            if !seen.contains(&item.domain) {
                seen.push(item.domain);
            }
        }
        seen
    }
}

/// Deterministic bounded assembly.
///
/// The builder takes already-ordered inputs — memory retrieval and Library
/// retrieval both return a total order — truncates each domain to its budget
/// and emits them in [`WorkspaceDomain::rank`] order. No scoring, no
/// salience, no learned ranking: those belong to the full #5 workspace.
#[derive(Debug)]
pub struct WorkspaceBuilder {
    individual_id: IndividualId,
    assembled_at: UtcTimestamp,
    budget: WorkspaceBudget,
    staged: Vec<(WorkspaceDomain, StagedItem)>,
}

#[derive(Debug)]
struct StagedItem {
    source_ref: SourceRef,
    content: WorkspaceContent,
    authority: AuthorityClass,
    source_time: Option<UtcTimestamp>,
    inclusion_reason: InclusionReason,
}

impl WorkspaceBuilder {
    pub fn new(individual_id: IndividualId, assembled_at: UtcTimestamp) -> Self {
        Self {
            individual_id,
            assembled_at,
            budget: WorkspaceBudget::default(),
            staged: Vec::new(),
        }
    }

    pub fn with_budget(mut self, budget: WorkspaceBudget) -> Self {
        self.budget = budget;
        self
    }

    /// The individual's lineage position, as orientation.
    pub fn with_continuity(mut self, head: &ContinuityHead) -> Self {
        self.staged.push((
            WorkspaceDomain::CurrentContinuityState,
            StagedItem {
                source_ref: SourceRef::Continuity {
                    commit_id: head.commit_id,
                    generation: head.generation.0,
                },
                content: WorkspaceContent::structured(serde_json::json!({
                    "generation": head.generation,
                    "writer_epoch": head.writer_epoch,
                })),
                authority: AuthorityClass::CanonicalState,
                source_time: Some(head.updated_at),
                inclusion_reason: InclusionReason::ContinuityContext,
            },
        ));
        self
    }

    pub fn with_current_input(mut self, input: &CurrentInput) -> Self {
        self.staged.push((
            WorkspaceDomain::CurrentInput,
            StagedItem {
                source_ref: SourceRef::Input {
                    evidence_id: input.evidence_id,
                },
                content: WorkspaceContent::text(input.text.clone()),
                authority: AuthorityClass::DirectInput,
                source_time: None,
                inclusion_reason: InclusionReason::DirectInput,
            },
        ));
        self
    }

    /// Durable memory, routed to the workspace domain matching its mutation
    /// domain so relationship and episodic state stay distinguishable.
    pub fn with_memories(mut self, memories: &[AttributedMemory]) -> Self {
        for memory in memories {
            let domain = match memory.record.domain {
                MutationDomain::Relationship => WorkspaceDomain::RelationshipMemory,
                MutationDomain::Episodic => WorkspaceDomain::EpisodicMemory,
                // The self domain has no implementation, so nothing can
                // produce such a record; skipping is the fail-closed choice.
                MutationDomain::SelfModel => continue,
            };
            self.staged.push((
                domain,
                StagedItem {
                    source_ref: SourceRef::Memory {
                        state_record_id: memory.record.state_record_id,
                        domain: memory.record.domain,
                        subject_key: memory.record.subject_key.clone(),
                        evidence_refs: memory.record.evidence_refs.clone(),
                    },
                    content: WorkspaceContent::structured(memory.record.payload.clone()),
                    authority: AuthorityClass::CanonicalState,
                    source_time: Some(memory.record.created_at),
                    inclusion_reason: InclusionReason::ActiveMemory,
                },
            ));
        }
        self
    }

    pub fn with_library_hits(mut self, hits: &[LibraryHit]) -> Self {
        for hit in hits {
            self.staged.push((
                WorkspaceDomain::LibraryEvidence,
                StagedItem {
                    source_ref: SourceRef::Library {
                        artifact_id: hit.artifact_id(),
                        chunk_id: hit.chunk_id(),
                        ordinal: hit.chunk.ordinal,
                        source_uri: hit.artifact.source_uri.clone(),
                    },
                    content: WorkspaceContent::text(hit.chunk.text.clone()),
                    authority: AuthorityClass::ExternalMaterial,
                    source_time: Some(hit.artifact.imported_at),
                    inclusion_reason: InclusionReason::LibraryMatch,
                },
            ));
        }
        self
    }

    pub fn with_resource_results(mut self, results: &[AttributedResult]) -> Self {
        for attributed in results {
            self.staged.push((
                WorkspaceDomain::ExternalResourceResult,
                StagedItem {
                    source_ref: SourceRef::Resource {
                        resource_id: attributed.result.resource_id,
                        resource_call_id: attributed.call.resource_call_id,
                    },
                    content: WorkspaceContent::structured(attributed.result.content.clone()),
                    authority: AuthorityClass::ExternalMaterial,
                    source_time: Some(attributed.call.completed_at),
                    inclusion_reason: InclusionReason::ResourceInvocation,
                },
            ));
        }
        self
    }

    /// The C0 self model: one item per non-empty field, carrying each entry's
    /// provenance. Derived-lane content — the snapshot an activation wrote,
    /// attributed to the field it belongs to.
    pub fn with_self_state(mut self, self_model: &SelfSnapshot) -> Self {
        for (field, entries) in &self_model.fields {
            if entries.is_empty() {
                continue;
            }
            self.staged.push((
                WorkspaceDomain::SelfMemory,
                StagedItem {
                    source_ref: SourceRef::SelfState {
                        field: field.clone(),
                    },
                    content: WorkspaceContent::structured(serde_json::json!({
                        "field": field,
                        "entries": entries,
                    })),
                    // Activated derived state: held about the individual, by
                    // the individual's own accepted proposals.
                    authority: AuthorityClass::CanonicalState,
                    source_time: None,
                    inclusion_reason: InclusionReason::SelfState,
                },
            ));
        }
        self
    }

    /// The operative parameters in force, as one overlay item. Presented as
    /// policy, not as fact: the backend may honour or ignore it, which is
    /// what evaluation exists to observe.
    pub fn with_active_policy(mut self, params: &OperativeParams, activation_seq: u64) -> Self {
        self.staged.push((
            WorkspaceDomain::ActivePolicy,
            StagedItem {
                source_ref: SourceRef::OperativePolicy { activation_seq },
                content: WorkspaceContent::structured(
                    serde_json::to_value(params).unwrap_or(serde_json::Value::Null),
                ),
                authority: AuthorityClass::CanonicalState,
                source_time: None,
                inclusion_reason: InclusionReason::OperativePolicy,
            },
        ));
        self
    }

    /// Raw canonical utterance records surfaced by recall. Each carries its
    /// evidence id and kind; retrieval decided *which* records, not what they
    /// say.
    pub fn with_recalled_evidence(mut self, records: &[EvidenceRecord]) -> Self {
        for record in records {
            let content = match record.payload.get("text").and_then(|t| t.as_str()) {
                Some(text) => WorkspaceContent::text(text.to_owned()),
                None => WorkspaceContent::structured(record.payload.clone()),
            };
            self.staged.push((
                WorkspaceDomain::RecalledEvidence,
                StagedItem {
                    source_ref: SourceRef::Evidence {
                        evidence_id: record.evidence_id,
                        kind: record.kind,
                    },
                    content,
                    authority: AuthorityClass::CanonicalState,
                    source_time: Some(record.created_at),
                    inclusion_reason: InclusionReason::EvidenceRecall,
                },
            ));
        }
        self
    }

    /// Emit the workspace. Stable sort by domain rank keeps each domain's
    /// caller-supplied order, so the result is a pure function of the inputs.
    pub fn build(self) -> Workspace {
        let mut staged = self.staged;
        staged.sort_by_key(|(domain, _)| domain.rank());

        let mut counts = DomainCounts::default();
        let mut items = Vec::new();
        for (domain, staged_item) in staged {
            if !counts.admit(domain, &self.budget) {
                continue;
            }
            items.push(WorkspaceItem {
                position: u32::try_from(items.len()).unwrap_or(u32::MAX),
                domain,
                source_ref: staged_item.source_ref,
                content: staged_item.content,
                authority: staged_item.authority,
                freshness: Freshness {
                    source_time: staged_item.source_time,
                    assembled_at: self.assembled_at,
                },
                inclusion_reason: staged_item.inclusion_reason,
            });
        }

        Workspace {
            individual_id: self.individual_id,
            assembled_at: self.assembled_at,
            items,
        }
    }
}

#[derive(Default)]
struct DomainCounts {
    relationship: usize,
    episodic: usize,
    library: usize,
    resource: usize,
    self_memory: usize,
    recalled_evidence: usize,
    active_policy: usize,
}

impl DomainCounts {
    fn admit(&mut self, domain: WorkspaceDomain, budget: &WorkspaceBudget) -> bool {
        let (used, cap) = match domain {
            WorkspaceDomain::RelationshipMemory => {
                (&mut self.relationship, budget.relationship_memory)
            }
            WorkspaceDomain::EpisodicMemory => (&mut self.episodic, budget.episodic_memory),
            WorkspaceDomain::LibraryEvidence => (&mut self.library, budget.library),
            WorkspaceDomain::ExternalResourceResult => {
                (&mut self.resource, budget.external_resource)
            }
            WorkspaceDomain::SelfMemory => (&mut self.self_memory, budget.self_memory),
            WorkspaceDomain::RecalledEvidence => {
                (&mut self.recalled_evidence, budget.recalled_evidence)
            }
            WorkspaceDomain::ActivePolicy => (&mut self.active_policy, budget.active_policy),
            // The head and the current input are never budgeted away: a turn
            // without its own input is not a smaller turn, it is a broken one.
            WorkspaceDomain::CurrentContinuityState | WorkspaceDomain::CurrentInput => {
                return true;
            }
        };
        if *used >= cap {
            return false;
        }
        *used += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::continuity::{Generation, WriterEpoch};
    use crate::memory::{LifecycleState, StateRecord};

    fn now() -> UtcTimestamp {
        UtcTimestamp::from_unix_millis(1_000)
    }

    fn head() -> ContinuityHead {
        ContinuityHead {
            individual_id: IndividualId::from_u128(1),
            commit_id: CommitId::from_u128(10),
            generation: Generation(3),
            writer_epoch: WriterEpoch(2),
            updated_at: UtcTimestamp::from_unix_millis(500),
        }
    }

    fn memory(id: u128, domain: MutationDomain) -> AttributedMemory {
        AttributedMemory {
            record: StateRecord {
                state_record_id: MemoryId::from_u128(id),
                individual_id: IndividualId::from_u128(1),
                domain,
                subject_key: matches!(domain, MutationDomain::Relationship)
                    .then(|| "user-fixture".to_owned()),
                kind: "fact".to_owned(),
                payload: serde_json::json!({ "preference": "ほうじ茶" }),
                lifecycle_state: LifecycleState::Active,
                created_commit_id: CommitId::from_u128(10),
                supersedes_state_record_id: None,
                superseded_by_state_record_id: None,
                evidence_refs: vec![EvidenceId::from_u128(50)],
                created_at: UtcTimestamp::from_unix_millis(400),
            },
            independent_evidence_count: 1,
            root_evidence: vec![EvidenceId::from_u128(50)],
        }
    }

    fn builder() -> WorkspaceBuilder {
        WorkspaceBuilder::new(IndividualId::from_u128(1), now())
    }

    #[test]
    fn domains_are_emitted_in_rank_order_regardless_of_staging_order() {
        let workspace = builder()
            .with_memories(&[
                memory(70, MutationDomain::Episodic),
                memory(71, MutationDomain::Relationship),
            ])
            .with_continuity(&head())
            .build();
        assert_eq!(
            workspace.domains(),
            vec![
                WorkspaceDomain::CurrentContinuityState,
                WorkspaceDomain::RelationshipMemory,
                WorkspaceDomain::EpisodicMemory,
            ]
        );
        for (index, item) in workspace.items.iter().enumerate() {
            assert_eq!(item.position as usize, index);
        }
    }

    #[test]
    fn assembly_is_a_pure_function_of_its_inputs() {
        let build = || {
            builder()
                .with_continuity(&head())
                .with_memories(&[memory(70, MutationDomain::Relationship)])
                .build()
        };
        assert_eq!(build(), build());
    }

    #[test]
    fn memory_keeps_its_domain_subject_and_evidence_refs() {
        let workspace = builder()
            .with_memories(&[memory(70, MutationDomain::Relationship)])
            .build();
        let item = workspace.items_in(WorkspaceDomain::RelationshipMemory)[0];
        let SourceRef::Memory {
            state_record_id,
            domain,
            subject_key,
            evidence_refs,
        } = &item.source_ref
        else {
            panic!("relationship memory must carry a memory source ref");
        };
        assert_eq!(*state_record_id, MemoryId::from_u128(70));
        assert_eq!(*domain, MutationDomain::Relationship);
        assert_eq!(subject_key.as_deref(), Some("user-fixture"));
        assert_eq!(evidence_refs, &vec![EvidenceId::from_u128(50)]);
        assert_eq!(item.authority, AuthorityClass::CanonicalState);
    }

    #[test]
    fn self_domain_records_are_never_staged() {
        // Nothing can produce one today; if that ever changes, the workspace
        // must not be the place it first appears.
        let workspace = builder()
            .with_memories(&[memory(70, MutationDomain::SelfModel)])
            .build();
        assert!(workspace.is_empty());
    }

    #[test]
    fn budget_truncates_a_domain_without_dropping_input_or_head() {
        let budget = WorkspaceBudget {
            relationship_memory: 1,
            ..WorkspaceBudget::default()
        };
        let workspace = builder()
            .with_budget(budget)
            .with_continuity(&head())
            .with_memories(&[
                memory(70, MutationDomain::Relationship),
                memory(71, MutationDomain::Relationship),
                memory(72, MutationDomain::Relationship),
            ])
            .build();
        assert_eq!(
            workspace
                .items_in(WorkspaceDomain::RelationshipMemory)
                .len(),
            1
        );
        assert_eq!(
            workspace
                .items_in(WorkspaceDomain::CurrentContinuityState)
                .len(),
            1
        );
        // Truncation keeps the caller's order: the first record survives.
        let SourceRef::Memory {
            state_record_id, ..
        } = &workspace.items_in(WorkspaceDomain::RelationshipMemory)[0].source_ref
        else {
            unreachable!()
        };
        assert_eq!(*state_record_id, MemoryId::from_u128(70));
    }

    #[test]
    fn no_authority_class_authorizes_a_mutation() {
        for class in [
            AuthorityClass::CanonicalState,
            AuthorityClass::DirectInput,
            AuthorityClass::ExternalMaterial,
        ] {
            assert!(!class.authorizes_mutation(), "{class}");
        }
    }
}
