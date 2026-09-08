//! `LibraryRepository` on SQLite: provenance, deterministic chunking and
//! retrieval, identity/idempotency of import, and separation from canonical
//! state (issue #16 Wave 3, plan §6.4, §11.2).
//!
//! The Library shares a database with continuity, evidence and memory, but
//! nothing here should move any of that: these tests pin the boundary down
//! by reading raw rows through an independent connection, not just by
//! trusting the store's own bookkeeping.

mod common;

use common::*;
use kamimusuhi_core::continuity::ContinuityError;
use kamimusuhi_core::ids::LibraryArtifactId;
use kamimusuhi_core::library::{
    LibraryError, LibraryMediaType, LibraryQuery, LibraryRepository, NewLibraryArtifact,
};

const ARTIFACT_A: LibraryArtifactId = LibraryArtifactId::from_u128(0xF001);
const ARTIFACT_B: LibraryArtifactId = LibraryArtifactId::from_u128(0xF002);

const MARKDOWN: &str = "# お茶の淹れ方\n\nほうじ茶は高温で淹れる。\n\n玄米茶も高温で淹れる。\n\n## 抹茶\n\n抹茶は茶筅で点てる。\n";

fn markdown_import(artifact_id: LibraryArtifactId) -> NewLibraryArtifact {
    NewLibraryArtifact {
        artifact_id,
        source_uri: Some("file:///tea-notes.md".to_owned()),
        title: Some("お茶ノート".to_owned()),
        media_type: LibraryMediaType::Markdown,
        content: MARKDOWN.to_owned(),
    }
}

/// Row counts scoped to this file's concerns, read through a fresh
/// connection so the store's own view cannot mask what actually landed on
/// disk.
fn library_row_counts(path: &std::path::Path) -> (i64, i64) {
    let conn = rusqlite::Connection::open(path).unwrap();
    let artifacts: i64 = conn
        .query_row("SELECT COUNT(*) FROM library_artifacts", [], |r| r.get(0))
        .unwrap();
    let chunks: i64 = conn
        .query_row("SELECT COUNT(*) FROM library_chunks", [], |r| r.get(0))
        .unwrap();
    (artifacts, chunks)
}

#[test]
fn imported_artifact_round_trips_its_provenance() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);

    let artifact = store.import(markdown_import(ARTIFACT_A)).unwrap();
    assert_eq!(artifact.artifact_id, ARTIFACT_A);
    assert_eq!(artifact.source_uri.as_deref(), Some("file:///tea-notes.md"));
    assert_eq!(artifact.title.as_deref(), Some("お茶ノート"));
    assert_eq!(artifact.media_type, LibraryMediaType::Markdown);
    assert_eq!(artifact.chunk_count, 3);
    assert!(artifact.content_digest.starts_with("sha256:"));
    assert_eq!(artifact.chunker_version.0, 1);

    // get_artifact answers the same provenance back.
    let fetched = store.get_artifact(ARTIFACT_A).unwrap().unwrap();
    assert_eq!(fetched, artifact);
}

#[test]
fn chunks_come_back_in_ordinal_order_with_headings_and_correct_offsets() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);
    store.import(markdown_import(ARTIFACT_A)).unwrap();

    let chunks = store.chunks(ARTIFACT_A).unwrap();
    assert_eq!(chunks.len(), 3);
    for (index, chunk) in chunks.iter().enumerate() {
        assert_eq!(chunk.ordinal as usize, index);
        assert_eq!(chunk.artifact_id, ARTIFACT_A);
        // char_offset points at this chunk's own text in the original doc.
        let start = chunk.char_offset as usize;
        assert!(
            MARKDOWN[start..].starts_with(&chunk.text),
            "chunk {} offset {start} does not point at its text",
            chunk.ordinal
        );
    }
    assert_eq!(chunks[0].heading.as_deref(), Some("お茶の淹れ方"));
    assert_eq!(chunks[1].heading.as_deref(), Some("お茶の淹れ方"));
    assert_eq!(chunks[2].heading.as_deref(), Some("抹茶"));
    assert_eq!(chunks[2].text, "抹茶は茶筅で点てる。");
}

#[test]
fn retrieval_hits_identify_their_own_origin_and_rank_deterministically() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);
    store.import(markdown_import(ARTIFACT_A)).unwrap();
    let chunks = store.chunks(ARTIFACT_A).unwrap();

    // "高温" is in chunk 0 and chunk 1; tie broken by (artifact, ordinal).
    let query = LibraryQuery::new("高温");
    let first_run = store.retrieve(&query).unwrap();
    assert_eq!(first_run.len(), 2);
    assert_eq!(first_run[0].artifact_id(), ARTIFACT_A);
    assert_eq!(first_run[0].chunk_id(), chunks[0].chunk_id);
    assert_eq!(first_run[0].chunk.ordinal, 0);
    assert_eq!(first_run[1].chunk.ordinal, 1);

    // Repeating the same query yields the identical order: no salience, no
    // learned ranking, no hidden randomness.
    let second_run = store.retrieve(&query).unwrap();
    assert_eq!(first_run, second_run);

    // .limited(n) truncates the ranked list.
    let limited = store.retrieve(&query.clone().limited(1)).unwrap();
    assert_eq!(limited.len(), 1);
    assert_eq!(limited[0].chunk.ordinal, 0);

    // .in_artifact(..) scopes to one artifact.
    store.import(markdown_import(ARTIFACT_B)).unwrap();
    let scoped = store
        .retrieve(&LibraryQuery::new("高温").in_artifact(ARTIFACT_A))
        .unwrap();
    assert!(scoped.iter().all(|hit| hit.artifact_id() == ARTIFACT_A));

    // A query matching nothing returns empty, not an error.
    let nothing = store.retrieve(&LibraryQuery::new("コーヒー")).unwrap();
    assert!(nothing.is_empty());
}

#[test]
fn reimporting_the_identical_document_under_the_same_id_is_a_no_op() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);

    let first = store.import(markdown_import(ARTIFACT_A)).unwrap();
    let (artifacts_before, chunks_before) = library_row_counts(&db.path);

    let second = store.import(markdown_import(ARTIFACT_A)).unwrap();
    assert_eq!(first, second);

    let (artifacts_after, chunks_after) = library_row_counts(&db.path);
    assert_eq!(artifacts_before, artifacts_after);
    assert_eq!(chunks_before, chunks_after);
}

#[test]
fn the_same_text_under_a_different_artifact_id_is_a_second_artifact() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);

    let a = store.import(markdown_import(ARTIFACT_A)).unwrap();
    let b = store.import(markdown_import(ARTIFACT_B)).unwrap();

    assert_ne!(a.artifact_id, b.artifact_id);
    // A matching digest must not collapse the two into one artifact.
    assert_eq!(a.content_digest, b.content_digest);

    let chunks_a: Vec<_> = store
        .chunks(ARTIFACT_A)
        .unwrap()
        .into_iter()
        .map(|c| c.chunk_id)
        .collect();
    let chunks_b: Vec<_> = store
        .chunks(ARTIFACT_B)
        .unwrap()
        .into_iter()
        .map(|c| c.chunk_id)
        .collect();
    assert!(chunks_a.iter().all(|id| !chunks_b.contains(id)));
}

#[test]
fn the_same_artifact_id_with_different_content_is_refused_and_leaves_the_artifact_untouched() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);

    let original = store.import(markdown_import(ARTIFACT_A)).unwrap();

    let revised = NewLibraryArtifact {
        artifact_id: ARTIFACT_A,
        source_uri: Some("file:///tea-notes.md".to_owned()),
        title: Some("お茶ノート".to_owned()),
        media_type: LibraryMediaType::Markdown,
        content: "# 別の内容\n\n全く違う文章。\n".to_owned(),
    };
    let error = store.import(revised).unwrap_err();
    assert_eq!(error, LibraryError::AlreadyExists(ARTIFACT_A));

    // The originally stored artifact is untouched.
    let after = store.get_artifact(ARTIFACT_A).unwrap().unwrap();
    assert_eq!(after, original);
}

#[test]
fn import_and_retrieval_never_move_the_continuity_head_or_write_canonical_rows() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    bootstrap(&store, BOOT_A);

    let head_before =
        kamimusuhi_core::continuity::ContinuityStore::load_head(&store, INDIVIDUAL).unwrap();
    let counts_before = row_counts(&db.path);

    store.import(markdown_import(ARTIFACT_A)).unwrap();
    store.retrieve(&LibraryQuery::new("高温")).unwrap();
    store.chunks(ARTIFACT_A).unwrap();

    let head_after =
        kamimusuhi_core::continuity::ContinuityStore::load_head(&store, INDIVIDUAL).unwrap();
    let counts_after = row_counts(&db.path);

    assert_eq!(head_before, head_after);
    assert_eq!(counts_before, counts_after);
}

#[test]
fn import_works_without_ever_touching_any_individual() {
    let db = TempDb::new();
    let store = open(&db.path, 1);
    // Deliberately no bootstrap: the Library is not scoped to an individual.

    let artifact = store.import(markdown_import(ARTIFACT_A)).unwrap();
    assert_eq!(artifact.chunk_count, 3);

    let load_result = kamimusuhi_core::continuity::ContinuityStore::load_head(&store, INDIVIDUAL);
    assert!(matches!(
        load_result,
        Err(ContinuityError::IndividualNotFound(id)) if id == INDIVIDUAL
    ));

    let conn = rusqlite::Connection::open(&db.path).unwrap();
    let individuals: i64 = conn
        .query_row("SELECT COUNT(*) FROM individuals", [], |r| r.get(0))
        .unwrap();
    assert_eq!(individuals, 0);
}
