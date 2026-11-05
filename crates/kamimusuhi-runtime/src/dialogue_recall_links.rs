//! Declared-metadata recall: producer-declared `links`/`tags` and usage
//! recency expand the episodic candidate set beyond the lexical baseline.
//!
//! Two frames run over the already scoped episodic pool. The link frame
//! follows outbound `payload["links"]` entries — `state:` links fetch the
//! named record when the pool does not hold it, `evidence:` links resolve
//! through a reverse index of each record's `evidence_refs`. The same
//! index also gives each target's in-degree: a record at least
//! `KNOT_MIN_IN_DEGREE` distinct records point at earns the
//! `recall_knot_bonus` multiplier, and `recall_reverse_decay` optionally
//! walks edges backwards at a weaker score. The tag frame
//! picks pool records sharing a declared tag with a seed. Everything a
//! frame selects is only a *candidate*: it is offered to the turn's
//! relevance judge under a `memory_<id>` id next to the lexical pool, and
//! the judge still decides what enters the workspace.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use kamimusuhi_core::c0::RetrievalParams;
use kamimusuhi_core::ids::{EvidenceId, MemoryId};
use kamimusuhi_core::memory::{
    AttributedMemory, MemoryLink, MemoryRepository, MemoryUsage, StateRecord,
};
use kamimusuhi_core::mutation::MutationDomain;
use kamimusuhi_core::time::UtcTimestamp;

use crate::c0;

/// Out-of-pool state records one hop may pull in while traversing.
const MAX_LINK_FETCH: usize = 64;

/// In-degree at which a traversed record counts as a knot — the FIO rule
/// that a distant memory surfaces only when several paths point at it.
pub(crate) const KNOT_MIN_IN_DEGREE: usize = 2;

/// Index one record's links into both traversal directions.
///
/// `cited_by` stays the forward index `evidence:` links resolve through —
/// a link to evidence reaches every record citing it. `in_edges` is the
/// reverse adjacency over the same declared links: target -> the records
/// pointing at it. Both halves of each evidence edge are added here —
/// a record declaring `evidence:<e>` becomes an in-edge of every known
/// citer, and every known declarer becomes an in-edge of the record when
/// it cites `<e>` — so the index stays correct whatever order pool and
/// fetched records arrive in. Sources are distinct record ids: duplicate
/// links and self-loops count once.
fn index_links(
    memory: &AttributedMemory,
    cited_by: &mut BTreeMap<EvidenceId, Vec<MemoryId>>,
    declares_evidence: &mut BTreeMap<EvidenceId, BTreeSet<MemoryId>>,
    in_edges: &mut BTreeMap<MemoryId, BTreeSet<MemoryId>>,
) {
    let id = memory.record.state_record_id;
    for link in memory.record.links() {
        match link {
            MemoryLink::State(target) => {
                in_edges.entry(target).or_default().insert(id);
            }
            MemoryLink::Evidence(evidence_id) => {
                declares_evidence.entry(evidence_id).or_default().insert(id);
                for target in cited_by.get(&evidence_id).into_iter().flatten() {
                    in_edges.entry(*target).or_default().insert(id);
                }
            }
        }
    }
    for evidence_id in &memory.record.evidence_refs {
        cited_by.entry(*evidence_id).or_default().push(id);
        for source in declares_evidence.get(evidence_id).into_iter().flatten() {
            in_edges.entry(id).or_default().insert(*source);
        }
    }
}

/// A record the declared path selected, plus how it was reached.
#[derive(Debug)]
pub(crate) struct DeclaredHit {
    pub record: AttributedMemory,
    /// `link_hop_<n>` for the link frame, `tag` for the tag frame.
    pub via: String,
    /// How many distinct records declare a link resolving to this one — the
    /// FIO "knot" degree. A self-loop or two links from the same record
    /// count once. In-degree ≥ 2 earns the `recall_knot_bonus` multiplier.
    pub in_degree: usize,
}

/// The declared-path picks for one turn: link-frame hits and tag-frame hits,
/// best-first within each frame.
#[derive(Debug, Default)]
pub(crate) struct DeclaredSelection {
    pub link: Vec<DeclaredHit>,
    pub tag: Vec<DeclaredHit>,
}

/// Expand `pool` through declared metadata.
///
/// `admit` applies the same individual/subject/lifecycle/validity gates the
/// pool itself passed, so a record fetched mid-traversal is followed and
/// offered only when the turn's scope admits it. The traversal is
/// label-correcting: a record reached by a better-scoring later path keeps
/// its best score and the hop that produced it.
pub(crate) fn select_declared(
    repository: &dyn MemoryRepository,
    usage: &dyn MemoryUsage,
    pool: &[AttributedMemory],
    admit: impl Fn(&StateRecord) -> bool,
    query: &str,
    params: &RetrievalParams,
    now: UtcTimestamp,
) -> DeclaredSelection {
    if params.recall_declared_budget == 0 || pool.is_empty() {
        return DeclaredSelection::default();
    }

    // Seeds anchor both frames. Lexical seeds are exactly the records the
    // lexical path would pick (strength 1.0). Hot seeds are records served
    // inside the hot window, freshness-scaled linearly from 1.0 down to 0.5.
    let mut seeds: BTreeMap<MemoryId, f64> =
        c0::rank_episodic(pool.to_vec(), query, params.min_score)
            .into_iter()
            .take(params.episodic_top_k)
            .map(|(_, memory)| (memory.record.state_record_id, 1.0))
            .collect();
    let window_ms = i64::try_from(params.recall_hot_window_secs)
        .unwrap_or(i64::MAX)
        .saturating_mul(1_000);
    if window_ms > 0 {
        let ids: Vec<MemoryId> = pool
            .iter()
            .map(|memory| memory.record.state_record_id)
            .collect();
        for (id, record_usage) in usage.usages(&ids) {
            let age_ms = (now.unix_millis() - record_usage.last_used_at.unix_millis()).max(0);
            if age_ms <= window_ms {
                let freshness = 1.0 - 0.5 * age_ms as f64 / window_ms as f64;
                seeds
                    .entry(id)
                    .and_modify(|strength| *strength = strength.max(freshness))
                    .or_insert(freshness);
            }
        }
    }

    // Records the frame can address: the pool plus what the traversal
    // fetches. `index_links` keeps `cited_by` (the forward evidence index)
    // and `in_edges` (the reverse adjacency used for knot in-degree and
    // reverse traversal) covering every record the frame holds.
    let mut records: BTreeMap<MemoryId, AttributedMemory> = BTreeMap::new();
    let mut cited_by: BTreeMap<EvidenceId, Vec<MemoryId>> = BTreeMap::new();
    let mut declares_evidence: BTreeMap<EvidenceId, BTreeSet<MemoryId>> = BTreeMap::new();
    let mut in_edges: BTreeMap<MemoryId, BTreeSet<MemoryId>> = BTreeMap::new();
    for memory in pool {
        index_links(memory, &mut cited_by, &mut declares_evidence, &mut in_edges);
        records.insert(memory.record.state_record_id, memory.clone());
    }

    // `best` is the label-correcting frontier: id -> (best score, hop that
    // produced it). Seeds sit at hop 0; a node re-expands only when a later
    // path improved its score.
    let mut best: BTreeMap<MemoryId, (f64, usize)> = seeds
        .iter()
        .map(|(id, strength)| (*id, (*strength, 0)))
        .collect();
    let mut frontier: Vec<MemoryId> = seeds.keys().copied().collect();
    for hop in 1..=params.recall_link_depth {
        // Resolve `state:` targets this frontier names but the frame does
        // not hold yet, in one batch per hop. An absent or unadmitted id is
        // a dead end, never an error.
        let missing: Vec<MemoryId> = frontier
            .iter()
            .filter_map(|id| records.get(id))
            .flat_map(|memory| memory.record.links())
            .filter_map(|link| match link {
                MemoryLink::State(target) if !records.contains_key(&target) => Some(target),
                _ => None,
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .take(MAX_LINK_FETCH)
            .collect();
        for record in repository.get_state_records(&missing) {
            if !admit(&record) {
                continue;
            }
            let memory = AttributedMemory {
                record,
                // Linked traversal carries no support snapshot;
                // provenance stays in the record's evidence_refs.
                independent_evidence_count: 0,
                root_evidence: Vec::new(),
            };
            let id = memory.record.state_record_id;
            index_links(
                &memory,
                &mut cited_by,
                &mut declares_evidence,
                &mut in_edges,
            );
            records.insert(id, memory);
        }

        let mut next = BTreeSet::new();
        for id in &frontier {
            let Some(memory) = records.get(id) else {
                continue;
            };
            let strength = best[id].0;
            let mut targets = Vec::new();
            for link in memory.record.links() {
                match link {
                    MemoryLink::State(target) => targets.push((target, params.recall_hop_decay)),
                    MemoryLink::Evidence(evidence_id) => targets.extend(
                        cited_by
                            .get(&evidence_id)
                            .into_iter()
                            .flatten()
                            .map(|target| (*target, params.recall_hop_decay)),
                    ),
                }
            }
            // Reverse edges reach records that point *at* this one — our
            // addition to FIO's outbound-only traversal, kept weaker via
            // `recall_reverse_decay`. 0 disables reverse traversal.
            if params.recall_reverse_decay > 0.0 {
                targets.extend(
                    in_edges
                        .get(id)
                        .into_iter()
                        .flatten()
                        .map(|source| (*source, params.recall_reverse_decay)),
                );
            }
            for (target, decay) in targets {
                if !records.contains_key(&target) {
                    continue;
                }
                let score = strength * decay;
                let improved = match best.get(&target) {
                    Some(&(s, h)) => score > s || (score == s && hop < h),
                    None => true,
                };
                if improved {
                    best.insert(target, (score, hop));
                    next.insert(target);
                }
            }
        }
        frontier = next.into_iter().collect();
    }

    // Link frame: every non-seed record a path reached, best score first.
    // Ties go to the shallower hop, then the store's newest-first order.
    // The budget splits links:tags 2:1. A `state:` link may name a record in
    // another domain — the offer keeps the record's own domain; only
    // SelfModel is excluded, matching the workspace's own skip.
    let link_budget = params.recall_declared_budget * 2 / 3;
    let mut link_hits: Vec<(f64, usize, usize, AttributedMemory)> = best
        .iter()
        .filter(|(id, (_, hop))| {
            *hop >= 1
                && !seeds.contains_key(id)
                && records[*id].record.domain != MutationDomain::SelfModel
        })
        .map(|(id, (score, hop))| {
            // The knot rule: a record several distinct records point at —
            // across the whole indexed set, not just the paths this turn
            // walked — floats higher by `recall_knot_bonus` per pointer.
            let in_degree = in_edges.get(id).map_or(0, BTreeSet::len);
            let score = if in_degree >= KNOT_MIN_IN_DEGREE {
                *score * (1.0 + params.recall_knot_bonus * in_degree.min(5) as f64)
            } else {
                *score
            };
            (score, *hop, in_degree, records[id].clone())
        })
        .collect();
    link_hits.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(Ordering::Equal)
            .then(a.1.cmp(&b.1))
            .then(b.3.record.created_at.cmp(&a.3.record.created_at))
            .then(b.3.record.state_record_id.cmp(&a.3.record.state_record_id))
    });
    let link: Vec<DeclaredHit> = link_hits
        .into_iter()
        .take(link_budget)
        .map(|(_, hop, in_degree, record)| DeclaredHit {
            via: format!("link_hop_{hop}"),
            record,
            in_degree,
        })
        .collect();

    // Tag frame: pool records sharing a declared tag with any seed. Score
    // is the shared-tag count; seeds and link-frame picks never re-enter.
    let seed_tags: BTreeSet<String> = seeds
        .keys()
        .filter_map(|id| records.get(id))
        .flat_map(|memory| memory.record.tags())
        .collect();
    let linked: BTreeSet<MemoryId> = link
        .iter()
        .map(|hit| hit.record.record.state_record_id)
        .collect();
    let mut tag_hits: Vec<(usize, AttributedMemory)> = pool
        .iter()
        .filter(|memory| {
            let id = memory.record.state_record_id;
            !seeds.contains_key(&id) && !linked.contains(&id)
        })
        .filter_map(|memory| {
            let shared = memory
                .record
                .tags()
                .iter()
                .filter(|tag| seed_tags.contains(*tag))
                .count();
            (shared > 0).then_some((shared, memory.clone()))
        })
        .collect();
    tag_hits.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.1.record.created_at.cmp(&a.1.record.created_at))
            .then(b.1.record.state_record_id.cmp(&a.1.record.state_record_id))
    });
    let tag = tag_hits
        .into_iter()
        .take(params.recall_declared_budget - link_budget)
        .map(|(_, record)| {
            let in_degree = in_edges
                .get(&record.record.state_record_id)
                .map_or(0, BTreeSet::len);
            DeclaredHit {
                via: "tag".to_owned(),
                record,
                in_degree,
            }
        })
        .collect();

    DeclaredSelection { link, tag }
}

/// Which of the surfaced records a reply actually referenced: the 32-hex id
/// must appear in the haystack — the response text plus each tool call's
/// serialized arguments and result.
///
/// FIO rule: a record warms only on reference, never on injection.
/// Injected-but-unreferenced items get no mark, matching FIO's measured
/// frozen-attractor failure where prompt injection itself re-warmed a
/// memory.
pub(crate) fn used_ids(haystack: &str, ids: &[MemoryId]) -> Vec<MemoryId> {
    // One mark per id even when a record surfaced through two lanes — e.g. a
    // relationship baseline *and* a declared link hit.
    let mut seen = BTreeSet::new();
    ids.iter()
        .filter(|id| haystack.contains(&id.to_string()) && seen.insert(**id))
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use kamimusuhi_core::ids::{CommitId, IndividualId};
    use kamimusuhi_core::memory::{
        LifecycleState, MemoryError, MemoryQuery, RecordUsage, StateLookup, StateRecordFacts,
    };
    use serde_json::{Value, json};

    use super::*;

    #[derive(Default)]
    struct Fixture {
        records: BTreeMap<MemoryId, StateRecord>,
        usages: BTreeMap<MemoryId, RecordUsage>,
    }

    impl Fixture {
        fn with(records: Vec<StateRecord>) -> Self {
            Self {
                records: records
                    .into_iter()
                    .map(|record| (record.state_record_id, record))
                    .collect(),
                usages: BTreeMap::new(),
            }
        }
    }

    impl StateLookup for Fixture {
        fn state_facts(
            &self,
            state_record_id: MemoryId,
        ) -> Result<Option<StateRecordFacts>, MemoryError> {
            Ok(self
                .records
                .get(&state_record_id)
                .map(|record| StateRecordFacts {
                    state_record_id: record.state_record_id,
                    individual_id: record.individual_id,
                    domain: record.domain,
                    subject_key: record.subject_key.clone(),
                    lifecycle_state: record.lifecycle_state,
                }))
        }
    }

    impl MemoryRepository for Fixture {
        fn retrieve(&self, _: &MemoryQuery) -> Result<Vec<AttributedMemory>, MemoryError> {
            unimplemented!("the fixture answers get_state_record only")
        }

        fn get_state_record(
            &self,
            state_record_id: MemoryId,
        ) -> Result<Option<StateRecord>, MemoryError> {
            Ok(self.records.get(&state_record_id).cloned())
        }
    }

    impl MemoryUsage for Fixture {
        fn mark_used(&self, _: &[MemoryId], _: UtcTimestamp) {
            unimplemented!("the fixture answers usages only")
        }

        fn usage(&self, id: &MemoryId) -> Option<RecordUsage> {
            self.usages.get(id).cloned()
        }

        fn usages(&self, ids: &[MemoryId]) -> Vec<(MemoryId, RecordUsage)> {
            ids.iter()
                .filter_map(|id| self.usages.get(id).map(|u| (*id, u.clone())))
                .collect()
        }
    }

    const NOW: i64 = 1_788_825_600_000;
    const QUERY: &str = "いつもの通勤手段は？";

    fn now() -> UtcTimestamp {
        UtcTimestamp::from_unix_millis(NOW)
    }

    fn record(id: u128, payload: Value) -> StateRecord {
        StateRecord {
            state_record_id: MemoryId::from_u128(id),
            individual_id: IndividualId::from_u128(1),
            domain: MutationDomain::Episodic,
            subject_key: Some("alice".to_owned()),
            kind: "episode".to_owned(),
            payload,
            lifecycle_state: LifecycleState::Active,
            created_commit_id: CommitId::from_u128(2),
            supersedes_state_record_id: None,
            superseded_by_state_record_id: None,
            evidence_refs: vec![],
            created_at: UtcTimestamp::from_unix_millis(id as i64 * 1_000),
        }
    }

    fn memory(id: u128, payload: Value) -> AttributedMemory {
        AttributedMemory {
            record: record(id, payload),
            independent_evidence_count: 1,
            root_evidence: vec![],
        }
    }

    fn state_link(id: u128) -> String {
        format!("state:{}", MemoryId::from_u128(id))
    }

    fn admit(record: &StateRecord) -> bool {
        record.individual_id == IndividualId::from_u128(1)
            && record.subject_key.as_deref() == Some("alice")
            && record.lifecycle_state.is_current()
    }

    fn select(
        fixture: &Fixture,
        pool: &[AttributedMemory],
        params: &RetrievalParams,
    ) -> DeclaredSelection {
        select_declared(fixture, fixture, pool, admit, QUERY, params, now())
    }

    #[test]
    fn link_frame_follows_state_links_with_decay_and_a_depth_cap() {
        // Seed -> 2 -> 3 -> 4. Depth 2 admits 2 (0.45) and 3 (0.2025),
        // never 4.
        let seed = memory(
            1,
            json!({
                "text": "通勤手段は毎朝ロードバイクです",
                "links": [state_link(2)],
            }),
        );
        let hop1 = record(2, json!({"text": "hop one", "links": [state_link(3)]}));
        let hop2 = record(3, json!({"text": "hop two", "links": [state_link(4)]}));
        let hop3 = record(4, json!({"text": "too deep"}));
        let fixture = Fixture::with(vec![hop1, hop2, hop3]);
        let params = RetrievalParams {
            recall_link_depth: 2,
            ..RetrievalParams::default()
        };
        let selection = select(&fixture, &[seed], &params);
        assert!(selection.tag.is_empty());
        let picks: Vec<(MemoryId, &str)> = selection
            .link
            .iter()
            .map(|hit| (hit.record.record.state_record_id, hit.via.as_str()))
            .collect();
        assert_eq!(
            picks,
            &[
                (MemoryId::from_u128(2), "link_hop_1"),
                (MemoryId::from_u128(3), "link_hop_2")
            ]
        );
    }

    #[test]
    fn link_frame_keeps_the_best_score_and_shallowest_hop() {
        // Record 2 is reached at hop 1 and hop 2: hop 1 wins.
        let mut deeper = record(5, json!({"text": "other"}));
        deeper.payload["links"] = json!([state_link(2)]);
        let seed = memory(
            1,
            json!({
                "text": "通勤手段は毎朝ロードバイクです",
                "links": [state_link(5), state_link(2)],
            }),
        );
        let fixture = Fixture::with(vec![record(2, json!({"text": "target"})), deeper]);
        let selection = select(&fixture, &[seed], &RetrievalParams::default());
        let hit = selection
            .link
            .iter()
            .find(|hit| hit.record.record.state_record_id == MemoryId::from_u128(2))
            .unwrap();
        assert_eq!(hit.via, "link_hop_1");
    }

    #[test]
    fn evidence_links_reach_pool_records_citing_that_evidence() {
        let cited = EvidenceId::from_u128(0xE1);
        let seed = memory(
            1,
            json!({
                "text": "通勤手段は毎朝ロードバイクです",
                "links": [format!("evidence:{cited}")],
            }),
        );
        let mut target = memory(2, json!({"text": "unrelated content"}));
        target.record.evidence_refs = vec![cited];
        let fixture = Fixture::default();
        let selection = select(
            &fixture,
            &[seed, target.clone()],
            &RetrievalParams::default(),
        );
        assert_eq!(selection.link.len(), 1);
        assert_eq!(selection.link[0].record, target);
        assert_eq!(selection.link[0].via, "link_hop_1");
    }

    #[test]
    fn hot_records_seed_the_frame_and_stale_usage_does_not() {
        let seed = memory(1, json!({"text": "通勤手段は毎朝ロードバイクです"}));
        let hot = memory(7, json!({"text": "hot record", "links": [state_link(8)]}));
        let stale = memory(
            9,
            json!({"text": "stale record", "links": [state_link(10)]}),
        );
        let via_hot = record(8, json!({"text": "reached through hot"}));
        let via_stale = record(10, json!({"text": "reached through stale"}));
        let mut fixture = Fixture::with(vec![via_hot, via_stale]);
        fixture.usages.insert(
            MemoryId::from_u128(7),
            RecordUsage {
                last_used_at: UtcTimestamp::from_unix_millis(NOW - 3_600_000),
                use_count: 3,
            },
        );
        fixture.usages.insert(
            MemoryId::from_u128(9),
            RecordUsage {
                last_used_at: UtcTimestamp::from_unix_millis(NOW - 300_000_000),
                use_count: 1,
            },
        );
        let selection = select(&fixture, &[seed, hot, stale], &RetrievalParams::default());
        let ids: Vec<MemoryId> = selection
            .link
            .iter()
            .map(|hit| hit.record.record.state_record_id)
            .collect();
        assert_eq!(ids, &[MemoryId::from_u128(8)]);
    }

    #[test]
    fn tag_frame_picks_by_shared_tag_count_and_skips_seeds() {
        let seed = memory(
            1,
            json!({
                "text": "通勤手段は毎朝ロードバイクです",
                "tags": ["commute", "bike"],
            }),
        );
        let two_tags = memory(2, json!({"text": "x", "tags": ["commute", "bike"]}));
        let one_tag = memory(3, json!({"text": "y", "tags": ["commute"]}));
        let no_tags = memory(4, json!({"text": "z", "tags": ["other"]}));
        let fixture = Fixture::default();
        let selection = select(
            &fixture,
            &[seed, two_tags, one_tag, no_tags],
            &RetrievalParams::default(),
        );
        let picks: Vec<MemoryId> = selection
            .tag
            .iter()
            .map(|hit| hit.record.record.state_record_id)
            .collect();
        assert_eq!(picks, &[MemoryId::from_u128(2), MemoryId::from_u128(3)]);
        assert!(selection.tag.iter().all(|hit| hit.via == "tag"));
    }

    #[test]
    fn a_state_link_may_reach_a_record_in_another_domain() {
        // Relationship records are never in the episodic pool, so a declared
        // `state:` link is the only way this one reaches the judge.
        let mut related = record(5, json!({"text": "a relationship fact"}));
        related.domain = MutationDomain::Relationship;
        let seed = memory(
            1,
            json!({
                "text": "通勤手段は毎朝ロードバイクです",
                "links": [state_link(5)],
            }),
        );
        let fixture = Fixture::with(vec![related]);
        let selection = select(&fixture, &[seed], &RetrievalParams::default());
        assert_eq!(selection.link.len(), 1);
        assert_eq!(
            selection.link[0].record.record.domain,
            MutationDomain::Relationship
        );
        assert_eq!(selection.link[0].via, "link_hop_1");
    }

    #[test]
    fn budget_splits_links_to_tags_two_to_one() {
        let links: Vec<String> = (10..20).map(state_link).collect();
        let seed = memory(
            1,
            json!({
                "text": "通勤手段は毎朝ロードバイクです",
                "tags": ["shared"],
                "links": links,
            }),
        );
        let fetched: Vec<StateRecord> = (10..20)
            .map(|id| record(id, json!({"text": format!("link {id}")})))
            .collect();
        let tagged: Vec<AttributedMemory> = (20..25)
            .map(|id| memory(id, json!({"text": "t", "tags": ["shared"]})))
            .collect();
        let fixture = Fixture::with(fetched);
        let mut pool = vec![seed];
        pool.extend(tagged);
        let selection = select(&fixture, &pool, &RetrievalParams::default());
        assert_eq!(selection.link.len(), 4, "2/3 of the budget is link hits");
        assert_eq!(selection.tag.len(), 2, "1/3 of the budget is tag hits");
    }

    #[test]
    fn in_degree_counts_distinct_sources_and_a_self_loop_once() {
        // Record 2 is declared by the seed and by itself: in-degree 2, a
        // knot. Record 3 is declared only by the seed.
        let seed = memory(
            1,
            json!({
                "text": "通勤手段は毎朝ロードバイクです",
                "links": [state_link(2), state_link(3)],
            }),
        );
        let loopy = record(2, json!({"text": "self", "links": [state_link(2)]}));
        let plain = record(3, json!({"text": "plain"}));
        let fixture = Fixture::with(vec![loopy, plain]);
        let selection = select(&fixture, &[seed], &RetrievalParams::default());
        let degree = |id: u128| {
            selection
                .link
                .iter()
                .find(|hit| hit.record.record.state_record_id == MemoryId::from_u128(id))
                .map(|hit| hit.in_degree)
        };
        assert_eq!(degree(2), Some(2), "seed plus the self-loop");
        assert_eq!(degree(3), Some(1));
    }

    #[test]
    fn two_links_from_the_same_record_count_once_toward_in_degree() {
        // Record 4 declares the same target twice — directly and through
        // an evidence link the target cites. It is still one pointer, so
        // the target's in-degree is exactly 2 (seed plus pointer).
        let cited = EvidenceId::from_u128(0xE2);
        let mut target = record(2, json!({"text": "knotted"}));
        target.evidence_refs = vec![cited];
        let seed = memory(
            1,
            json!({
                "text": "通勤手段は毎朝ロードバイクです",
                "links": [state_link(2)],
            }),
        );
        let pointer = memory(
            4,
            json!({
                "text": "unrelated",
                "links": [state_link(2), format!("evidence:{cited}")],
            }),
        );
        let fixture = Fixture::with(vec![target]);
        let selection = select(&fixture, &[seed, pointer], &RetrievalParams::default());
        let hit = selection
            .link
            .iter()
            .find(|hit| hit.record.record.state_record_id == MemoryId::from_u128(2))
            .unwrap();
        assert_eq!(hit.in_degree, 2, "seed plus the one pointer, counted once");
    }

    #[test]
    fn the_knot_bonus_prefers_multi_pointer_records_and_zero_disables_it() {
        // The seed reaches 2 and 5 at hop 1 with equal scores; only 2 is
        // also declared by another pool record, so the bonus lifts it.
        let seed = memory(
            1,
            json!({
                "text": "通勤手段は毎朝ロードバイクです",
                "links": [state_link(2), state_link(5)],
            }),
        );
        let pointer = memory(3, json!({"text": "unrelated", "links": [state_link(2)]}));
        let fixture = Fixture::with(vec![
            record(2, json!({"text": "knotted"})),
            record(5, json!({"text": "plain"})),
        ]);
        let pool = vec![seed, pointer];
        let rank = |params: &RetrievalParams| -> Vec<MemoryId> {
            select(&fixture, &pool, params)
                .link
                .iter()
                .map(|hit| hit.record.record.state_record_id)
                .collect()
        };
        let knotted = rank(&RetrievalParams::default());
        let two = MemoryId::from_u128(2);
        let five = MemoryId::from_u128(5);
        assert!(
            knotted.iter().position(|id| *id == two).unwrap()
                < knotted.iter().position(|id| *id == five).unwrap(),
            "the knotted record must outrank the equal-score one: {knotted:?}"
        );
        let flat = rank(&RetrievalParams {
            recall_knot_bonus: 0.0,
            ..RetrievalParams::default()
        });
        assert!(
            flat.iter().position(|id| *id == five).unwrap()
                < flat.iter().position(|id| *id == two).unwrap(),
            "at zero bonus the newer record wins the score tie: {flat:?}"
        );
    }

    #[test]
    fn reverse_decay_reaches_records_that_only_point_at_a_seed() {
        // Record 2 declares `state:<seed>`; the seed names only 3. Outbound
        // traversal can never reach 2 — a reverse edge can, at the weaker
        // `recall_reverse_decay` score, so it ranks behind 3.
        let seed = memory(
            1,
            json!({
                "text": "通勤手段は毎朝ロードバイクです",
                "links": [state_link(3)],
            }),
        );
        let pointer = memory(2, json!({"text": "unrelated", "links": [state_link(1)]}));
        let fixture = Fixture::with(vec![record(3, json!({"text": "outbound"}))]);
        let pool = vec![seed, pointer];
        let selection = select(&fixture, &pool, &RetrievalParams::default());
        let picks: Vec<(MemoryId, &str)> = selection
            .link
            .iter()
            .map(|hit| (hit.record.record.state_record_id, hit.via.as_str()))
            .collect();
        assert_eq!(
            picks,
            &[
                (MemoryId::from_u128(3), "link_hop_1"),
                (MemoryId::from_u128(2), "link_hop_1"),
            ]
        );
        let flat = select(
            &fixture,
            &pool,
            &RetrievalParams {
                recall_reverse_decay: 0.0,
                ..RetrievalParams::default()
            },
        );
        let ids: Vec<MemoryId> = flat
            .link
            .iter()
            .map(|hit| hit.record.record.state_record_id)
            .collect();
        assert_eq!(ids, &[MemoryId::from_u128(3)]);
    }

    #[test]
    fn knot_and_reverse_hits_compete_inside_the_link_frame_budget() {
        // Five outbound picks at 0.45, one knotted (10, oldest) lifted to
        // 0.54, one reverse hit at 0.20: all six compete for the same 4
        // link slots, so the knot keeps a slot and the reverse hit drops.
        let links: Vec<String> = (10..15).map(state_link).collect();
        let seed = memory(
            1,
            json!({
                "text": "通勤手段は毎朝ロードバイクです",
                "tags": ["shared"],
                "links": links,
            }),
        );
        let fetched: Vec<StateRecord> = (10..15)
            .map(|id| record(id, json!({"text": format!("link {id}")})))
            .collect();
        let pointer = memory(6, json!({"text": "unrelated", "links": [state_link(10)]}));
        let reverse = memory(5, json!({"text": "unrelated", "links": [state_link(1)]}));
        let tagged: Vec<AttributedMemory> = (20..22)
            .map(|id| memory(id, json!({"text": "t", "tags": ["shared"]})))
            .collect();
        let fixture = Fixture::with(fetched);
        let pool: Vec<AttributedMemory> =
            [seed, pointer, reverse].into_iter().chain(tagged).collect();
        let selection = select(&fixture, &pool, &RetrievalParams::default());
        let picks: Vec<MemoryId> = selection
            .link
            .iter()
            .map(|hit| hit.record.record.state_record_id)
            .collect();
        assert_eq!(
            picks,
            &[
                MemoryId::from_u128(10),
                MemoryId::from_u128(14),
                MemoryId::from_u128(13),
                MemoryId::from_u128(12),
            ],
            "the knot takes a link-budget slot; the reverse hit loses it"
        );
        assert_eq!(selection.tag.len(), 2);
    }

    #[test]
    fn zero_budget_or_empty_pool_selects_nothing() {
        let seed = memory(
            1,
            json!({"text": "通勤手段は毎朝ロードバイクです", "links": [state_link(2)]}),
        );
        let fixture = Fixture::with(vec![record(2, json!({"text": "x"}))]);
        let params = RetrievalParams {
            recall_declared_budget: 0,
            ..RetrievalParams::default()
        };
        let selection = select(&fixture, std::slice::from_ref(&seed), &params);
        assert!(selection.link.is_empty() && selection.tag.is_empty());
        let selection = select(&fixture, &[], &RetrievalParams::default());
        assert!(selection.link.is_empty() && selection.tag.is_empty());
    }

    #[test]
    fn used_ids_marks_references_never_injection() {
        let id = MemoryId::from_u128(0xBEEF);
        let other = MemoryId::from_u128(0xF00D);
        // A reply naming the id counts as a reference — once, even when the
        // id was surfaced through two lanes.
        assert_eq!(
            used_ids(&format!("覚えています: {id}"), &[id, other, id]),
            vec![id]
        );
        // The id appearing only in the injected candidate content — never in
        // the reply — earns no mark. Injection alone must not warm a record.
        let injected = json!({"memory": {"record": {"state_record_id": id.to_string()}}});
        assert!(used_ids("そうですね", &[id, other]).is_empty());
        assert!(used_ids(&format!("{injected}"), &[other]).is_empty());
    }
}
