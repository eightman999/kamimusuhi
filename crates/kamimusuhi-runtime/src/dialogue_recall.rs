//! Bounded, read-only recall candidates for a dialogue turn.
//!
//! Relationship memory stays on the existing retrieval path. Episodic memory
//! and raw utterances may be judged for relevance, but retain their original
//! records and provenance. Nothing here promotes an utterance into a belief.

use std::collections::{BTreeMap, BTreeSet};

use kamimusuhi_core::c0::OperativeParams;
use kamimusuhi_core::evidence::{EvidenceKind, EvidenceRecord};
use kamimusuhi_core::ids::{EvidenceId, IndividualId};
use kamimusuhi_core::memory::{AttributedMemory, MemoryQuery, MemoryRepository};
use kamimusuhi_core::mutation::MutationDomain;
use kamimusuhi_store_sqlite::SqliteStore;
use serde_json::{Value, json};

use crate::llm_jev::{ConversationError, RecallCandidate, RecallRelevance};
use crate::{RuntimeError, c0};

const EPISODIC_POOL_LIMIT: usize = 64;
const CANDIDATES_PER_DOMAIN: usize = 4;
const NEW_CANDIDATE_SLOTS: usize = 2;
/// Bound the complete serialized candidate content, never a truncated view.
const MAX_CANDIDATE_BYTES: usize = 4 * 1024;

#[derive(Debug)]
struct OfferedRecord<T> {
    id: String,
    content: Value,
    baseline: bool,
    record: T,
}

#[derive(Debug)]
struct RecallGroup<T> {
    baseline: Vec<(String, T)>,
    offered: Vec<OfferedRecord<T>>,
    top_k: usize,
}

/// One turn's immutable retrieval snapshot. Provider answers refer only to
/// the IDs returned by [`Self::candidates`], not arbitrary store records.
#[derive(Debug)]
pub struct RecallPool {
    relationships: Vec<AttributedMemory>,
    episodic: RecallGroup<AttributedMemory>,
    evidence: RecallGroup<EvidenceRecord>,
}

struct RecallScope<'a> {
    individual_id: IndividualId,
    subject: &'a str,
    source_id: &'a str,
    exclude: &'a BTreeSet<EvidenceId>,
}

impl RecallScope<'_> {
    fn admits_memory(&self, memory: &AttributedMemory) -> bool {
        memory.record.individual_id == self.individual_id
            && memory.record.subject_key.as_deref() == Some(self.subject)
            && memory.is_current()
    }

    fn admits_evidence(&self, record: &EvidenceRecord) -> bool {
        record.individual_id == self.individual_id
            && record.source.source_id.as_deref() == Some(self.source_id)
            && matches!(
                record.kind,
                EvidenceKind::UserUtterance | EvidenceKind::AgentUtterance
            )
            && !self.exclude.contains(&record.evidence_id)
            && record.payload.get("text").is_some_and(Value::is_string)
    }
}

impl RecallPool {
    /// Read the existing lexical baseline and a bounded, identically scoped
    /// pool. The additional records need no lexical overlap, so a paraphrase
    /// that the baseline misses can still reach the relevance judge.
    pub fn load(
        store: &SqliteStore,
        individual_id: IndividualId,
        subject: &str,
        source_id: &str,
        query: &str,
        params: &OperativeParams,
        exclude: &BTreeSet<EvidenceId>,
    ) -> Result<Self, RuntimeError> {
        let baseline_memories =
            c0::retrieve_memories(store, individual_id, subject, query, params)?;
        let baseline_evidence =
            c0::recall_evidence(store, individual_id, source_id, query, params, exclude)?;
        let episodic_pool = if params.retrieval.episodic_top_k == 0 {
            Vec::new()
        } else {
            MemoryRepository::retrieve(
                store,
                &MemoryQuery::current(individual_id)
                    .in_domain(MutationDomain::Episodic)
                    .about(subject)
                    .limited(EPISODIC_POOL_LIMIT),
            )?
        };
        let evidence_pool = if params.retrieval.evidence_top_k == 0 {
            Vec::new()
        } else {
            store.c0_evidence_pool(individual_id, params.retrieval.evidence_pool)?
        };
        Self::from_records(
            baseline_memories,
            baseline_evidence,
            episodic_pool,
            evidence_pool,
            &RecallScope {
                individual_id,
                subject,
                source_id,
                exclude,
            },
            params,
        )
    }

    fn from_records(
        baseline_memories: Vec<AttributedMemory>,
        baseline_evidence: Vec<EvidenceRecord>,
        episodic_pool: Vec<AttributedMemory>,
        evidence_pool: Vec<EvidenceRecord>,
        scope: &RecallScope<'_>,
        params: &OperativeParams,
    ) -> Result<Self, RuntimeError> {
        let mut seen_memories = BTreeSet::new();
        let memories: Vec<_> = baseline_memories
            .into_iter()
            .filter(|memory| {
                scope.admits_memory(memory) && seen_memories.insert(memory.record.state_record_id)
            })
            .collect();
        let relationships: Vec<_> = memories
            .iter()
            .filter(|memory| memory.record.domain == MutationDomain::Relationship)
            .take(params.retrieval.relationship_top_k)
            .cloned()
            .collect();
        let relationship_ids: BTreeSet<_> = relationships
            .iter()
            .map(|memory| memory.record.state_record_id)
            .collect();
        let episodic_baseline: Vec<AttributedMemory> = memories
            .into_iter()
            .filter(|memory| memory.record.domain == MutationDomain::Episodic)
            .collect();
        let episodic_pool: Vec<AttributedMemory> = episodic_pool
            .into_iter()
            .take(EPISODIC_POOL_LIMIT)
            .filter(|memory| {
                scope.admits_memory(memory)
                    && memory.record.domain == MutationDomain::Episodic
                    && !relationship_ids.contains(&memory.record.state_record_id)
            })
            .collect();
        let baseline_evidence: Vec<EvidenceRecord> = baseline_evidence
            .into_iter()
            .filter(|record| scope.admits_evidence(record))
            .collect();
        let evidence_pool: Vec<EvidenceRecord> = evidence_pool
            .into_iter()
            .take(params.retrieval.evidence_pool)
            .filter(|record| scope.admits_evidence(record))
            .collect();
        Ok(Self {
            relationships,
            episodic: RecallGroup::build(
                episodic_baseline,
                episodic_pool,
                params.retrieval.episodic_top_k,
                |memory| format!("memory_{}", memory.record.state_record_id),
                |memory| {
                    Ok(json!({
                        "source_type": "durable_episodic_memory",
                        "memory": memory,
                    }))
                },
            )?,
            evidence: RecallGroup::build(
                baseline_evidence,
                evidence_pool,
                params.retrieval.evidence_top_k,
                |record| format!("evidence_{}", record.evidence_id),
                |record| {
                    Ok(json!({
                        "source_type": "raw_utterance",
                        "speaker": match record.kind {
                            EvidenceKind::UserUtterance => "user",
                            EvidenceKind::AgentUtterance => "assistant",
                            _ => "unknown",
                        },
                        "is_durable_belief": false,
                        "evidence": record,
                    }))
                },
            )?,
        })
    }

    /// Existing retrieval, including records too large to send for judgment.
    pub fn baseline(&self) -> (Vec<AttributedMemory>, Vec<EvidenceRecord>) {
        let mut memories = self.relationships.clone();
        memories.extend(
            self.episodic
                .baseline
                .iter()
                .map(|(_, record)| record.clone()),
        );
        let evidence = self
            .evidence
            .baseline
            .iter()
            .map(|(_, record)| record.clone())
            .collect();
        (memories, evidence)
    }

    /// At most eight complete records, with up to two slots in each domain
    /// reserved for new candidates. Remaining slots prefer the baseline.
    /// Within each group, new records retain the store's newest-first order.
    pub fn candidates(&self) -> Vec<RecallCandidate> {
        self.episodic
            .offered
            .iter()
            .map(OfferedRecord::candidate)
            .chain(self.evidence.offered.iter().map(OfferedRecord::candidate))
            .collect()
    }

    /// Apply a complete, exact-ID response without changing any stored data.
    ///
    /// Each domain first reserves room for baseline records not sent to Jev
    /// (oversize or beyond the candidate budget). The remaining slots prefer
    /// `Relevant`, then baseline `Uncertain`, never new `Uncertain`. Thus a
    /// relevant newcomer may displace an uncertain *judged* baseline, but
    /// never an unjudged baseline. Relationship records are not judged.
    /// Ties keep candidate/baseline order; returned relevant records precede
    /// the retained baseline. An all-uncertain response preserves the baseline.
    pub fn apply(
        &self,
        relevance: &BTreeMap<String, RecallRelevance>,
    ) -> Result<(Vec<AttributedMemory>, Vec<EvidenceRecord>), RuntimeError> {
        let expected: BTreeSet<_> = self
            .episodic
            .offered
            .iter()
            .map(|record| record.id.as_str())
            .chain(
                self.evidence
                    .offered
                    .iter()
                    .map(|record| record.id.as_str()),
            )
            .collect();
        if relevance.len() != expected.len()
            || relevance.keys().any(|id| !expected.contains(id.as_str()))
        {
            return Err(ConversationError::InvalidDecision(
                "recall relevance keys must exactly match the offered candidate IDs".to_owned(),
            )
            .into());
        }
        let mut memories = self.relationships.clone();
        memories.extend(self.episodic.apply(relevance));
        Ok((memories, self.evidence.apply(relevance)))
    }
}

impl<T: Clone> RecallGroup<T> {
    fn build(
        baseline: Vec<T>,
        pool: Vec<T>,
        top_k: usize,
        id_of: impl Fn(&T) -> String,
        content_of: impl Fn(&T) -> Result<Value, RuntimeError>,
    ) -> Result<Self, RuntimeError> {
        let mut seen = BTreeSet::new();
        let baseline: Vec<_> = baseline
            .into_iter()
            .map(|record| (id_of(&record), record))
            .filter(|(id, _)| seen.insert(id.clone()))
            .take(top_k)
            .collect();
        if top_k == 0 {
            return Ok(Self {
                baseline,
                offered: Vec::new(),
                top_k,
            });
        }
        let offer = |id: String, record: &T, baseline: bool| {
            let content = content_of(record)?;
            Ok::<_, RuntimeError>((content.to_string().len() <= MAX_CANDIDATE_BYTES).then(|| {
                OfferedRecord {
                    id,
                    content,
                    baseline,
                    record: record.clone(),
                }
            }))
        };
        let mut baseline_offers = Vec::new();
        for (id, record) in &baseline {
            if let Some(offered) = offer(id.clone(), record, true)? {
                baseline_offers.push(offered);
            }
        }
        let mut new_offers = Vec::new();
        for record in pool {
            let id = id_of(&record);
            if seen.insert(id.clone())
                && let Some(offered) = offer(id, &record, false)?
            {
                new_offers.push(offered);
                if new_offers.len() == CANDIDATES_PER_DOMAIN {
                    break;
                }
            }
        }
        let reserved = new_offers.len().min(NEW_CANDIDATE_SLOTS);
        let mut offered: Vec<_> = baseline_offers
            .into_iter()
            .take(CANDIDATES_PER_DOMAIN - reserved)
            .collect();
        let remaining = CANDIDATES_PER_DOMAIN - offered.len();
        offered.extend(new_offers.into_iter().take(remaining));
        Ok(Self {
            baseline,
            offered,
            top_k,
        })
    }

    fn apply(&self, relevance: &BTreeMap<String, RecallRelevance>) -> Vec<T> {
        let offered_ids: BTreeSet<_> = self
            .offered
            .iter()
            .map(|record| record.id.as_str())
            .collect();
        let mut selected: BTreeSet<_> = self
            .baseline
            .iter()
            .filter(|(id, _)| !offered_ids.contains(id.as_str()))
            .map(|(id, _)| id.as_str())
            .collect();
        for record in &self.offered {
            if selected.len() == self.top_k {
                break;
            }
            if matches!(relevance.get(&record.id), Some(RecallRelevance::Relevant)) {
                selected.insert(record.id.as_str());
            }
        }
        for (id, _) in &self.baseline {
            if selected.len() == self.top_k {
                break;
            }
            if matches!(relevance.get(id), Some(RecallRelevance::Uncertain)) {
                selected.insert(id.as_str());
            }
        }
        let mut records = Vec::new();
        for record in &self.offered {
            if matches!(relevance.get(&record.id), Some(RecallRelevance::Relevant))
                && selected.remove(record.id.as_str())
            {
                records.push(record.record.clone());
            }
        }
        for (id, record) in &self.baseline {
            if selected.remove(id.as_str()) {
                records.push(record.clone());
            }
        }
        records
    }
}

impl<T> OfferedRecord<T> {
    fn candidate(&self) -> RecallCandidate {
        RecallCandidate {
            id: self.id.clone(),
            content: self.content.clone(),
            baseline: self.baseline,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kamimusuhi_core::evidence::{EvidenceSource, RetentionClass};
    use kamimusuhi_core::ids::{CommitId, MemoryId};
    use kamimusuhi_core::memory::{LifecycleState, StateRecord};
    use kamimusuhi_core::mutation::OriginClass;
    use kamimusuhi_core::time::UtcTimestamp;

    fn memory(id: u128, domain: MutationDomain, text: &str) -> AttributedMemory {
        AttributedMemory {
            record: StateRecord {
                state_record_id: MemoryId::from_u128(id),
                individual_id: IndividualId::from_u128(1),
                domain,
                subject_key: Some("alice".to_owned()),
                kind: "episode".to_owned(),
                payload: json!({"text": text}),
                lifecycle_state: LifecycleState::Active,
                created_commit_id: CommitId::from_u128(2),
                supersedes_state_record_id: None,
                superseded_by_state_record_id: None,
                evidence_refs: vec![EvidenceId::from_u128(id + 1000)],
                created_at: UtcTimestamp::from_unix_millis(id as i64),
            },
            independent_evidence_count: 1,
            root_evidence: vec![EvidenceId::from_u128(id + 1000)],
        }
    }

    fn episode(id: u128, text: &str) -> AttributedMemory {
        memory(id, MutationDomain::Episodic, text)
    }

    fn evidence(id: u128, text: &str) -> EvidenceRecord {
        EvidenceRecord {
            evidence_id: EvidenceId::from_u128(id),
            individual_id: IndividualId::from_u128(1),
            session_id: None,
            turn_id: None,
            kind: EvidenceKind::UserUtterance,
            origin_class: OriginClass::Reported,
            payload: json!({"text": text}),
            source: EvidenceSource {
                source_id: Some("text-chat:alice".to_owned()),
                source_sequence: Some(id as u64),
                content_digest: Some(format!("original-{id}")),
            },
            retention_class: RetentionClass::Standard,
            created_at: UtcTimestamp::from_unix_millis(id as i64),
        }
    }

    fn pool(
        baseline_memories: Vec<AttributedMemory>,
        baseline_evidence: Vec<EvidenceRecord>,
        episodic: Vec<AttributedMemory>,
        evidence: Vec<EvidenceRecord>,
        params: &OperativeParams,
    ) -> RecallPool {
        RecallPool::from_records(
            baseline_memories,
            baseline_evidence,
            episodic,
            evidence,
            &RecallScope {
                individual_id: IndividualId::from_u128(1),
                subject: "alice",
                source_id: "text-chat:alice",
                exclude: &BTreeSet::from([EvidenceId::from_u128(99)]),
            },
            params,
        )
        .unwrap()
    }

    fn uncertain(pool: &RecallPool) -> BTreeMap<String, RecallRelevance> {
        pool.candidates()
            .into_iter()
            .map(|candidate| (candidate.id, RecallRelevance::Uncertain))
            .collect()
    }

    #[test]
    fn zero_lexical_overlap_can_be_selected_without_changing_provenance() {
        let text = "会社には毎朝ロードバイクで通っています。";
        assert_eq!(c0::lexical_score("いつもの通勤手段は？", text), 0.0);
        let episode = episode(10, text);
        let mut utterance = evidence(11, text);
        utterance.kind = EvidenceKind::AgentUtterance;
        let pool = pool(
            Vec::new(),
            Vec::new(),
            vec![episode.clone()],
            vec![utterance.clone()],
            &OperativeParams::default(),
        );
        let candidates = pool.candidates();
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().all(|candidate| !candidate.baseline));
        assert_eq!(candidates[0].content["memory"], json!(episode));
        assert_eq!(candidates[1].content["evidence"], json!(utterance));
        assert_eq!(candidates[1].content["speaker"], "assistant");
        assert_eq!(candidates[1].content["is_durable_belief"], false);
        let relevance = candidates
            .into_iter()
            .map(|candidate| (candidate.id, RecallRelevance::Relevant))
            .collect();
        assert_eq!(
            pool.apply(&relevance).unwrap(),
            (vec![episode], vec![utterance])
        );
        assert_eq!(pool.baseline(), (Vec::new(), Vec::new()));
    }

    #[test]
    fn pools_exclude_other_subjects_individuals_noncurrent_and_nonutterance_records() {
        let valid = episode(10, "current episode");
        let mut other_subject = episode(11, "other subject");
        other_subject.record.subject_key = Some("bob".to_owned());
        let mut other_individual = episode(12, "other individual");
        other_individual.record.individual_id = IndividualId::from_u128(2);
        let mut invalidated = episode(13, "invalidated");
        invalidated.record.lifecycle_state = LifecycleState::Invalidated;
        let mut superseded = episode(14, "superseded");
        superseded.record.lifecycle_state = LifecycleState::Superseded;
        let relationship = memory(15, MutationDomain::Relationship, "relationship stays");
        let self_model = memory(16, MutationDomain::SelfModel, "self must not enter");
        let memory_inputs = vec![
            valid.clone(),
            other_subject,
            other_individual,
            invalidated,
            superseded,
            relationship.clone(),
            self_model,
        ];
        let valid_raw = evidence(20, "raw utterance");
        let mut other_source = evidence(21, "other source");
        other_source.source.source_id = Some("text-chat:bob".to_owned());
        let mut wrong_individual = evidence(22, "wrong owner");
        wrong_individual.individual_id = IndividualId::from_u128(2);
        let mut derived = evidence(23, "summary is not a raw utterance");
        derived.kind = EvidenceKind::Summary;
        let mut no_text = evidence(24, "not available");
        no_text.payload = json!({"text": 42});
        let raw_inputs = vec![
            valid_raw.clone(),
            other_source,
            wrong_individual,
            derived,
            no_text,
            evidence(99, "already in visible history"),
        ];
        let pool = pool(
            memory_inputs.clone(),
            raw_inputs.clone(),
            memory_inputs,
            raw_inputs,
            &OperativeParams::default(),
        );
        assert_eq!(pool.candidates().len(), 2);
        assert_eq!(
            pool.baseline(),
            (vec![relationship.clone(), valid], vec![valid_raw])
        );
        let relevance = pool
            .candidates()
            .into_iter()
            .map(|candidate| (candidate.id, RecallRelevance::Irrelevant))
            .collect();
        assert_eq!(
            pool.apply(&relevance).unwrap(),
            (vec![relationship], Vec::new())
        );
    }

    #[test]
    fn uncertain_keeps_only_the_baseline_and_rejects_unknown_or_missing_ids() {
        let old = episode(10, "baseline");
        let raw = evidence(11, "baseline utterance");
        let pool = pool(
            vec![old.clone()],
            vec![raw.clone()],
            vec![episode(12, "new")],
            vec![evidence(13, "new utterance")],
            &OperativeParams::default(),
        );
        let relevance = uncertain(&pool);
        assert_eq!(pool.apply(&relevance).unwrap(), (vec![old], vec![raw]));
        let mut missing = relevance.clone();
        missing.pop_first();
        assert!(pool.apply(&missing).is_err());
        let mut unknown = relevance.clone();
        unknown.insert("invented".to_owned(), RecallRelevance::Relevant);
        assert!(pool.apply(&unknown).is_err());
        missing.insert("invented".to_owned(), RecallRelevance::Relevant);
        assert!(
            pool.apply(&missing).is_err(),
            "equal counts must not hide unknown IDs"
        );
    }

    #[test]
    fn relevant_is_prioritized_with_separate_top_k_and_no_utterance_promotion() {
        let first = episode(10, "baseline retained when uncertain");
        let rejected = episode(11, "baseline rejected");
        let newcomer = episode(12, "relevant newcomer");
        let new_raw = evidence(22, "relevant new utterance");
        let second_raw = evidence(23, "another relevant utterance");
        let pool = pool(
            vec![first.clone(), rejected.clone()],
            vec![evidence(20, "old one"), evidence(21, "old two")],
            vec![newcomer.clone(), episode(13, "uncertain new memory")],
            vec![new_raw.clone(), second_raw.clone()],
            &OperativeParams::default(),
        );
        let mut relevance = uncertain(&pool);
        relevance.insert(
            format!("memory_{}", rejected.record.state_record_id),
            RecallRelevance::Irrelevant,
        );
        relevance.insert(
            format!("memory_{}", newcomer.record.state_record_id),
            RecallRelevance::Relevant,
        );
        relevance.insert(
            format!("evidence_{}", new_raw.evidence_id),
            RecallRelevance::Relevant,
        );
        relevance.insert(
            format!("evidence_{}", second_raw.evidence_id),
            RecallRelevance::Relevant,
        );
        assert_eq!(
            pool.apply(&relevance).unwrap(),
            (vec![newcomer, first], vec![new_raw, second_raw])
        );
    }

    #[test]
    fn candidate_budget_reserves_new_slots_and_protects_unjudged_baseline() {
        let mut params = OperativeParams::default();
        params.retrieval.episodic_top_k = 5;
        params.retrieval.evidence_top_k = 5;
        let memories: Vec<_> = (10..15).map(|id| episode(id, "baseline")).collect();
        let raw: Vec<_> = (20..25).map(|id| evidence(id, "baseline")).collect();
        let pool = pool(
            memories.clone(),
            raw.clone(),
            (30..36).map(|id| episode(id, "new")).collect(),
            (40..46).map(|id| evidence(id, "new")).collect(),
            &params,
        );
        let candidates = pool.candidates();
        assert_eq!(candidates.len(), 8);
        assert_eq!(
            candidates
                .iter()
                .filter(|candidate| candidate.baseline)
                .count(),
            4
        );
        assert_eq!(pool.apply(&uncertain(&pool)).unwrap(), pool.baseline());
        let relevance = candidates
            .into_iter()
            .map(|candidate| {
                let answer = if candidate.baseline {
                    RecallRelevance::Irrelevant
                } else {
                    RecallRelevance::Relevant
                };
                (candidate.id, answer)
            })
            .collect();
        let (selected_memories, selected_raw) = pool.apply(&relevance).unwrap();
        assert_eq!(selected_memories.len(), 5);
        assert_eq!(selected_raw.len(), 5);
        assert_eq!(&selected_memories[2..], &memories[2..]);
        assert_eq!(&selected_raw[2..], &raw[2..]);
    }

    #[test]
    fn oversize_baseline_is_never_truncated_sent_or_displaced() {
        let mut params = OperativeParams::default();
        params.retrieval.episodic_top_k = 1;
        params.retrieval.evidence_top_k = 1;
        let huge = "あ".repeat(MAX_CANDIDATE_BYTES);
        let old = episode(10, &huge);
        let raw = evidence(11, &huge);
        let pool = pool(
            vec![old.clone()],
            vec![raw.clone()],
            vec![episode(12, &huge), episode(13, "small\ncomplete text")],
            vec![
                evidence(14, &huge),
                evidence(15, "small\ncomplete utterance"),
            ],
            &params,
        );
        let candidates = pool.candidates();
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().all(|candidate| !candidate.baseline));
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.content.to_string().len() <= MAX_CANDIDATE_BYTES)
        );
        assert_eq!(
            candidates[0].content["memory"]["record"]["payload"]["text"],
            "small\ncomplete text"
        );
        let relevance = candidates
            .into_iter()
            .map(|candidate| (candidate.id, RecallRelevance::Relevant))
            .collect();
        assert_eq!(pool.apply(&relevance).unwrap(), (vec![old], vec![raw]));
    }

    #[test]
    fn zero_top_k_disables_that_domain_and_duplicates_remain_unique() {
        let baseline_memory = episode(10, "same memory");
        let raw = evidence(11, "same evidence");
        let relationship = memory(12, MutationDomain::Relationship, "relationship");
        let recall_pool = pool(
            vec![
                baseline_memory.clone(),
                baseline_memory.clone(),
                relationship.clone(),
            ],
            vec![raw.clone(), raw.clone()],
            vec![baseline_memory.clone(), baseline_memory.clone()],
            vec![raw.clone(), raw.clone()],
            &OperativeParams::default(),
        );
        assert_eq!(recall_pool.candidates().len(), 2);
        assert_eq!(
            recall_pool.apply(&uncertain(&recall_pool)).unwrap(),
            (
                vec![relationship.clone(), baseline_memory.clone()],
                vec![raw.clone()]
            )
        );
        let mut params = OperativeParams::default();
        params.retrieval.episodic_top_k = 0;
        params.retrieval.evidence_top_k = 0;
        let disabled = pool(
            vec![baseline_memory.clone(), relationship.clone()],
            vec![raw.clone()],
            vec![baseline_memory],
            vec![raw],
            &params,
        );
        assert!(disabled.candidates().is_empty());
        assert_eq!(
            disabled.apply(&BTreeMap::new()).unwrap(),
            (vec![relationship], Vec::new())
        );
        assert!(
            disabled
                .apply(&BTreeMap::from([(
                    "invented".to_owned(),
                    RecallRelevance::Relevant,
                )]))
                .is_err()
        );
    }
}
