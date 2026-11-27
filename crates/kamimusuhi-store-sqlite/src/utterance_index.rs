//! Derived full-text index over canonical utterances (schema v7).
//!
//! The index is a rebuildable projection of `evidence_records`: it never
//! holds utterance text, never decides what is true, and is only ever read to
//! *offer* an old utterance to the relevance judge. Two operations:
//!
//! - [`SqliteStore::utterance_index_sync`] catches the index up with evidence
//!   appended since the last call. It writes only the derived tables.
//! - [`SqliteStore::utterance_index_search`] is read-only: it picks the rarest
//!   query bigrams, matches them with FTS5 and returns the best BM25 rows
//!   inside one individual and one source.
//!
//! The method follows FIO's transcript index (character bigrams, BM25,
//! common-gram cut), which was measured to find an exchange 77 turns back in
//! a few tens of milliseconds. Freshness weighting and the relative score
//! floor are the runtime's job; this module only bounds and ranks the work.
//!
//! Only `standard`-retention utterances are indexed. An `ephemeral` utterance
//! is deliberately left out so the index cannot extend how long it stays
//! reachable.

use std::collections::BTreeSet;

use kamimusuhi_core::c0::C0Error;
use kamimusuhi_core::evidence::EvidenceRecord;
use kamimusuhi_core::ids::IndividualId;
use rusqlite::{TransactionBehavior, params, params_from_iter};

use crate::evidence::{build_evidence, read_evidence_row};
use crate::store::SqliteStore;

/// Most query bigrams one search matches on, rarest first (FIO: 24).
pub const MAX_QUERY_GRAMS: usize = 24;
/// A bigram present in more than this share of indexed utterances is too
/// common to discriminate and is dropped from the query (FIO: 0.30).
pub const COMMON_GRAM_RATIO: f64 = 0.30;
/// The common-gram cut only applies once this many utterances are indexed;
/// below it every gram looks common (FIO: 50).
pub const COMMON_GRAM_MIN_DOCS: u64 = 50;
/// Bound on the query bigrams looked up in the vocabulary. An input at the
/// dialogue byte limit stays far below this.
const MAX_VOCAB_LOOKUPS: usize = 4096;

/// What one [`SqliteStore::utterance_index_sync`] call did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UtteranceIndexSync {
    /// Utterances examined and recorded in the map by this call.
    pub indexed: usize,
    /// Whether the row bound stopped the call with utterances still unindexed.
    pub pending: bool,
}

/// One search hit: the canonical record and SQLite's raw `bm25()` value.
/// More negative is a better match.
#[derive(Debug, Clone)]
pub struct UtteranceHit {
    pub record: EvidenceRecord,
    pub bm25: f64,
}

/// Character bigrams of `text`, lowercased, over letters and digits only.
///
/// Whitespace, punctuation and symbols are removed *before* pairing, so a
/// bigram never straddles a token boundary the FTS tokenizer would split on,
/// and index and query always produce identical terms. Duplicates are kept in
/// order: term frequency is part of BM25.
pub fn utterance_grams(text: &str) -> Vec<String> {
    let chars: Vec<char> = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();
    chars
        .windows(2)
        .map(|pair| pair.iter().collect::<String>())
        .collect()
}

fn backend(err: rusqlite::Error) -> C0Error {
    C0Error::Backend {
        message: err.to_string(),
    }
}

impl SqliteStore {
    /// Index up to `max_rows` standard-retention utterances that have no map
    /// row yet, oldest first, in one transaction.
    ///
    /// Idempotent: a second call with nothing new does nothing. Writes only
    /// the derived index tables — never `evidence_records`.
    pub fn utterance_index_sync(&self, max_rows: usize) -> Result<UtteranceIndexSync, C0Error> {
        if max_rows == 0 {
            return Ok(UtteranceIndexSync::default());
        }
        let mut conn = self.conn().map_err(|e| C0Error::Backend {
            message: e.to_string(),
        })?;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(backend)?;
        // One row past the bound tells us whether more remain. The WHERE
        // clause repeats the partial index's predicate so the scan stays on
        // `evidence_indexable_utterances`.
        let pending: Vec<(String, String)> = {
            let mut stmt = tx
                .prepare(
                    "SELECT e.evidence_id, e.payload_json FROM evidence_records e \
                     WHERE e.kind IN ('user_utterance', 'agent_utterance') \
                       AND e.retention_class = 'standard' \
                       AND NOT EXISTS (SELECT 1 FROM utterance_index_map m \
                                       WHERE m.evidence_id = e.evidence_id) \
                     ORDER BY e.created_at, e.evidence_id LIMIT ?1",
                )
                .map_err(backend)?;
            stmt.query_map(params![(max_rows as i64).saturating_add(1)], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(backend)?
            .collect::<Result<_, _>>()
            .map_err(backend)?
        };
        let more = pending.len() > max_rows;
        let mut indexed = 0;
        for (evidence_id, payload) in pending.into_iter().take(max_rows) {
            tx.execute(
                "INSERT INTO utterance_index_map(evidence_id) VALUES (?1)",
                params![evidence_id],
            )
            .map_err(backend)?;
            let fts_rowid = tx.last_insert_rowid();
            // A payload without string text still gets its map row, so it is
            // not re-examined on every later sync.
            let grams = serde_json::from_str::<serde_json::Value>(&payload)
                .ok()
                .and_then(|value| {
                    value
                        .get("text")
                        .and_then(|t| t.as_str())
                        .map(utterance_grams)
                })
                .unwrap_or_default();
            if !grams.is_empty() {
                tx.execute(
                    "INSERT INTO utterance_index(rowid, grams) VALUES (?1, ?2)",
                    params![fts_rowid, grams.join(" ")],
                )
                .map_err(backend)?;
            }
            indexed += 1;
        }
        tx.commit().map_err(backend)?;
        Ok(UtteranceIndexSync {
            indexed,
            pending: more,
        })
    }

    /// Utterances examined so far (map rows), for traces and tests.
    pub fn utterance_index_len(&self) -> Result<u64, C0Error> {
        let conn = self.conn().map_err(|e| C0Error::Backend {
            message: e.to_string(),
        })?;
        conn.query_row("SELECT COUNT(*) FROM utterance_index_map", [], |row| {
            row.get::<_, i64>(0)
        })
        .map(|count| count.max(0) as u64)
        .map_err(backend)
    }

    /// The best-matching indexed utterances of one individual inside one
    /// source, best BM25 first, at most `limit`. Read-only.
    ///
    /// Query bigrams absent from the index are dropped, as are — once the
    /// index is large enough to tell — bigrams common to more than
    /// [`COMMON_GRAM_RATIO`] of it. The rarest [`MAX_QUERY_GRAMS`] of what is
    /// left form an OR match. No usable bigram means no hits, never a scan.
    pub fn utterance_index_search(
        &self,
        individual_id: IndividualId,
        source_id: &str,
        query_text: &str,
        limit: usize,
    ) -> Result<Vec<UtteranceHit>, C0Error> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut seen = BTreeSet::new();
        let grams: Vec<String> = utterance_grams(query_text)
            .into_iter()
            .filter(|gram| seen.insert(gram.clone()))
            .take(MAX_VOCAB_LOOKUPS)
            .collect();
        if grams.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn().map_err(|e| C0Error::Backend {
            message: e.to_string(),
        })?;
        let docs: u64 = conn
            .query_row("SELECT COUNT(*) FROM utterance_index_map", [], |row| {
                row.get::<_, i64>(0)
            })
            .map(|count| count.max(0) as u64)
            .map_err(backend)?;
        if docs == 0 {
            return Ok(Vec::new());
        }

        // Document frequency of each query bigram, in query order.
        let placeholders = vec!["?"; grams.len()].join(",");
        let mut frequency: Vec<(String, u64)> = {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT term, doc FROM utterance_index_vocab WHERE term IN ({placeholders})"
                ))
                .map_err(backend)?;
            stmt.query_map(params_from_iter(grams.iter()), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(backend)?
            .map(|row| row.map(|(term, doc)| (term, doc.max(0) as u64)))
            .collect::<Result<_, _>>()
            .map_err(backend)?
        };
        let common_above =
            (docs >= COMMON_GRAM_MIN_DOCS).then_some(COMMON_GRAM_RATIO * docs as f64);
        frequency.retain(|(_, doc)| {
            *doc > 0 && common_above.is_none_or(|ceiling| (*doc as f64) <= ceiling)
        });
        // Rarest first; ties keep the order the bigrams appear in the query.
        let position = |term: &str| grams.iter().position(|g| g == term).unwrap_or(usize::MAX);
        frequency.sort_by(|a, b| a.1.cmp(&b.1).then(position(&a.0).cmp(&position(&b.0))));
        frequency.truncate(MAX_QUERY_GRAMS);
        if frequency.is_empty() {
            return Ok(Vec::new());
        }
        let expression = frequency
            .iter()
            .map(|(term, _)| format!("\"{}\"", term.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" OR ");

        let mut stmt = conn
            .prepare(
                "SELECT e.evidence_id, e.individual_id, e.session_id, e.turn_id, e.kind, \
                        e.origin_class, e.payload_json, e.source_id, e.source_sequence, \
                        e.content_digest, e.retention_class, e.created_at, \
                        bm25(utterance_index) AS score \
                 FROM utterance_index \
                 JOIN utterance_index_map m ON m.fts_rowid = utterance_index.rowid \
                 JOIN evidence_records e ON e.evidence_id = m.evidence_id \
                 WHERE utterance_index MATCH ?1 \
                   AND e.individual_id = ?2 AND e.source_id = ?3 \
                 ORDER BY score, e.created_at DESC, e.evidence_id DESC \
                 LIMIT ?4",
            )
            .map_err(backend)?;
        stmt.query_map(
            params![
                expression,
                individual_id.to_string(),
                source_id,
                limit as i64
            ],
            |row| Ok((read_evidence_row(row)?, row.get::<_, f64>(12)?)),
        )
        .map_err(backend)?
        .map(|row| {
            row.and_then(|(evidence, bm25)| {
                build_evidence(evidence)
                    .map(|record| UtteranceHit { record, bm25 })
                    .map_err(|e| rusqlite::Error::InvalidParameterName(e.to_string()))
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(backend)
    }
}

#[cfg(test)]
mod tests {
    use super::utterance_grams;

    #[test]
    fn grams_pair_letters_and_digits_across_removed_separators() {
        assert_eq!(utterance_grams("通勤 手段！"), vec!["通勤", "勤手", "手段"]);
        // Lowercased, punctuation and whitespace dropped before pairing.
        assert_eq!(utterance_grams("Ab-C 1"), vec!["ab", "bc", "c1"]);
        // Repeats stay: term frequency is part of the ranking.
        assert_eq!(utterance_grams("ああああ"), vec!["ああ", "ああ", "ああ"]);
        assert!(utterance_grams("。").is_empty());
        assert!(utterance_grams("a").is_empty());
    }
}
