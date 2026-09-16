//! Reviewed findings remain external Library material. Real SQLite tests
//! check whole-card retrieval, immutable revisions and source provenance.

use std::path::Path;

use kamimusuhi_core::digest::content_digest;
use kamimusuhi_core::ids::LibraryArtifactId;
use kamimusuhi_core::library::LibraryRepository;
use kamimusuhi_runtime::research::{MAX_CONTEXT_BYTES, MAX_SELECTED_FINDINGS, ResearchCatalog};
use kamimusuhi_runtime::{ResourceImplementation, Runtime, RuntimeOptions};
use serde_json::{Value, json};

fn finding(number: u128, id: &str, status: &str) -> Value {
    json!({
        "artifact_id": format!("{number:032x}"),
        "id": id,
        "revision": 1,
        "experiment": id,
        "status": status,
        "title": format!("{id}の実験結果"),
        "claim": "この設定では結果が得られた。",
        "limitations": "別の設定・実機・言語能力では未検証。",
        "keywords": ["恒常性", "負債", id],
        "sources": [{
            "path": "evidence.md",
            "digest": content_digest(b"first\nsecond\n"),
            "start_line": 1,
            "end_line": 2
        }],
        "applications": ["観測の説明に参照し、能力の獲得とは扱わない。"]
    })
}

fn document(findings: Vec<Value>) -> Value {
    json!({"schema_version": 1, "revision": 1, "findings": findings})
}

fn catalog(findings: Vec<Value>) -> ResearchCatalog {
    ResearchCatalog::from_json(&document(findings).to_string()).unwrap()
}

fn runtime() -> (tempfile::TempDir, Runtime) {
    let dir = tempfile::tempdir().unwrap();
    let runtime = Runtime::init(
        dir.path(),
        RuntimeOptions::deterministic(8_765),
        ResourceImplementation::FakeA,
    )
    .unwrap();
    (dir, runtime)
}

fn library_id(number: u128) -> LibraryArtifactId {
    LibraryArtifactId::from_u128(number)
}

#[test]
fn bundled_catalog_has_valid_source_hashes_and_line_ranges() {
    let bundled = ResearchCatalog::bundled().unwrap();
    assert!(!bundled.findings.is_empty());
    bundled
        .validate_sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
        .unwrap();
    let (_dir, runtime) = runtime();
    bundled.sync(runtime.store()).unwrap();
    let context = bundled
        .context(
            runtime.store(),
            "G0-v6の記憶・検索・重要度・関連・容量・出典・経験・学習の結果",
        )
        .unwrap();
    let first = serde_json::to_value(&context.selected[0].finding).unwrap();
    assert_eq!(first["status"], "invalid");
    assert!(first["experiment"].as_str().unwrap().contains("G0-v6"));
    let overview = bundled
        .context(runtime.store(), "実験成果を教えて")
        .unwrap();
    let statuses: Vec<_> = overview
        .selected
        .iter()
        .map(|selected| serde_json::to_value(&selected.finding).unwrap()["status"].clone())
        .collect();
    assert!(statuses.contains(&json!("supported")));
    assert!(statuses.contains(&json!("invalid")) || statuses.contains(&json!("failed")));
    assert!(overview.selected.len() < overview.active_findings);
    assert!(serde_json::to_vec(&overview).unwrap().len() <= MAX_CONTEXT_BYTES);
}

#[test]
fn source_drift_missing_lines_and_symlink_escape_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("evidence.md");
    std::fs::write(&source, "first\nsecond\n").unwrap();
    let valid = catalog(vec![finding(1, "g0-v6", "invalid")]);
    valid.validate_sources(root.path()).unwrap();
    std::fs::write(&source, "changed\nsecond\n").unwrap();
    assert!(valid.validate_sources(root.path()).is_err());
    std::fs::write(&source, "first\nsecond\n").unwrap();
    let mut bad_lines = finding(1, "g0-v6", "invalid");
    bad_lines["sources"][0]["end_line"] = json!(3);
    assert!(
        catalog(vec![bad_lines])
            .validate_sources(root.path())
            .is_err()
    );

    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("outside.md"), "first\nsecond\n").unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("outside.md"),
            root.path().join("escape.md"),
        )
        .unwrap();
        let mut escaped = finding(2, "escape", "pending");
        escaped["sources"][0]["path"] = json!("escape.md");
        assert!(
            catalog(vec![escaped])
                .validate_sources(root.path())
                .is_err()
        );
    }
}

#[test]
fn malformed_status_identity_revisions_paths_and_oversized_content_fail_closed() {
    for (pointer, value) in [
        ("/status", json!("strong_pass")),
        ("/id", json!("../injected")),
        ("/artifact_id", json!("not-an-id")),
        ("/artifact_id", json!("00000000000000000000000000000000")),
        ("/revision", json!(0)),
        ("/limitations", json!("")),
        ("/claim", json!("x".repeat(1_201))),
        ("/keywords", json!(["k".repeat(81)])),
        ("/keywords", json!(["same", "SAME"])),
        ("/sources/0/path", json!("../outside.md")),
        ("/sources/0/path", json!("/absolute.md")),
        ("/sources/0/path", json!("nested/../../outside.md")),
        ("/sources/0/path", json!("nested\\outside.md")),
        ("/sources/0/path", json!("nested/./file.md")),
        ("/sources/0/path", json!("C:/outside.md")),
        ("/sources/0/digest", json!("sha256:bad")),
        ("/sources/0/start_line", json!(0)),
        ("/sources/0/end_line", json!(0)),
        ("/sources", json!([])),
    ] {
        let mut changed = finding(1, "g0-v6", "invalid");
        *changed.pointer_mut(pointer).unwrap() = value;
        assert!(
            ResearchCatalog::from_json(&document(vec![changed]).to_string()).is_err(),
            "{pointer}"
        );
    }
    let mut extra = finding(1, "g0-v6", "invalid");
    extra["unreviewed_instruction"] = json!("must reject unknown field");
    assert!(ResearchCatalog::from_json(&document(vec![extra]).to_string()).is_err());
    for second in [
        finding(2, "g0-v6", "supported"),
        finding(1, "other-experiment", "pending"),
        {
            let mut same_revision = finding(2, "g0-v6", "supported");
            same_revision["revision"] = json!(2);
            same_revision
        },
    ] {
        assert!(
            ResearchCatalog::from_json(
                &document(vec![finding(1, "g0-v6", "invalid"), second]).to_string()
            )
            .is_err()
        );
    }
    let too_many = document(
        (1..=65)
            .map(|n| finding(n, &format!("exp-{n}"), "pending"))
            .collect(),
    );
    assert!(ResearchCatalog::from_json(&too_many.to_string()).is_err());
    assert!(ResearchCatalog::from_json(&" ".repeat(256 * 1024 + 1)).is_err());
}

#[test]
fn imports_are_idempotent_and_new_versions_keep_old_artifacts_and_canonical_head() {
    let (_dir, runtime) = runtime();
    let head = runtime.head().unwrap();
    let initial_count = runtime.store().artifacts().unwrap().len();
    let first = catalog(vec![finding(1, "g0-v6", "invalid")]);
    assert_eq!(first.sync(runtime.store()).unwrap(), 1);
    let original = runtime
        .store()
        .get_artifact(library_id(1))
        .unwrap()
        .unwrap();
    let original_chunks = runtime.store().chunks(library_id(1)).unwrap();
    assert_eq!(first.sync(runtime.store()).unwrap(), 0);
    assert_eq!(
        runtime.store().get_artifact(library_id(1)).unwrap(),
        Some(original.clone())
    );

    let mut next_finding = finding(2, "g0-v6", "limited");
    next_finding["revision"] = json!(2);
    next_finding["claim"] = json!("新しい条件で再評価した。旧版は無効のまま保存する。");
    let mut next = document(vec![next_finding]);
    next["revision"] = json!(2);
    let second = ResearchCatalog::from_json(&next.to_string()).unwrap();
    assert_eq!(second.sync(runtime.store()).unwrap(), 1);
    assert_eq!(second.sync(runtime.store()).unwrap(), 0);
    assert_eq!(
        runtime.store().artifacts().unwrap().len(),
        initial_count + 2
    );
    assert_eq!(
        runtime.store().get_artifact(library_id(1)).unwrap(),
        Some(original)
    );
    assert_eq!(
        runtime.store().chunks(library_id(1)).unwrap(),
        original_chunks
    );
    let selected = second
        .context(runtime.store(), "G0-v6の結果を教えて")
        .unwrap();
    assert_eq!(selected.catalog_revision, 2);
    assert_eq!(selected.selected.len(), 1);
    assert_eq!(selected.selected[0].artifact_id, library_id(2));
    assert_eq!(selected.selected[0].finding.revision, 2);
    assert_eq!(runtime.head().unwrap(), head);
}

#[test]
fn published_revision_cannot_be_rewritten_or_reassigned_to_another_artifact() {
    let (_dir, runtime) = runtime();
    catalog(vec![finding(1, "g0-v6", "invalid")])
        .sync(runtime.store())
        .unwrap();
    let original_chunks = runtime.store().chunks(library_id(1)).unwrap();
    for changed in [
        finding(1, "g0-v6", "supported"),
        finding(2, "g0-v6", "supported"),
    ] {
        let conflicting = catalog(vec![finding(3, "unrelated-new", "pending"), changed]);
        assert!(conflicting.sync(runtime.store()).is_err());
        assert!(
            runtime
                .store()
                .get_artifact(library_id(3))
                .unwrap()
                .is_none(),
            "conflicting catalog partially imported"
        );
    }
    assert_eq!(
        runtime.store().chunks(library_id(1)).unwrap(),
        original_chunks
    );
}

#[test]
fn japanese_matching_preserves_invalid_and_failed_findings_with_all_limits_and_sources() {
    let (_dir, runtime) = runtime();
    let mut invalid = finding(1, "g0-v6", "invalid");
    invalid["claim"] = json!("見かけのSTRONG_PASSは評価方法の問題で無効。");
    invalid["limitations"] = json!("能力獲得の証拠に使えない。修正した条件での再実験が必要。");
    let reviewed = catalog(vec![
        invalid.clone(),
        finding(2, "g0-v5", "failed"),
        finding(3, "mio-recovery", "limited"),
        finding(4, "mio-action", "pending"),
        finding(5, "mio-observation", "supported"),
    ]);
    reviewed.sync(runtime.store()).unwrap();
    let result = reviewed
        .context(runtime.store(), "G0-v6は成功したから能力として使っていい？")
        .unwrap();
    assert_eq!(result.active_findings, 5);
    assert_eq!(result.selected.len(), 1);
    let card = &result.selected[0];
    assert_eq!(serde_json::to_value(&card.finding).unwrap(), invalid);
    let payload = serde_json::to_string(&card.finding).unwrap();
    assert_eq!(card.content_digest, content_digest(payload.as_bytes()));
    assert!(runtime.store().chunks(card.artifact_id).unwrap().len() > 1);
    let failed = reviewed
        .context(runtime.store(), "G0-v5について聞きたい")
        .unwrap();
    assert_eq!(
        serde_json::to_value(&failed.selected[0].finding).unwrap()["status"],
        "failed"
    );
    let pending = reviewed
        .context(runtime.store(), "mio-actionについて聞きたい")
        .unwrap();
    assert_eq!(
        serde_json::to_value(&pending.selected[0].finding).unwrap()["status"],
        "pending"
    );
    let overview = reviewed
        .context(runtime.store(), "実験成果の総覧を教えて")
        .unwrap();
    assert_eq!(overview.selected.len(), MAX_SELECTED_FINDINGS);
    assert!(
        overview
            .selected
            .iter()
            .any(|selected| selected.finding.id == "g0-v6")
    );
    assert!(
        reviewed
            .context(runtime.store(), "今日のお昼は何にしよう")
            .unwrap()
            .selected
            .is_empty()
    );
}

#[test]
fn context_budget_omits_whole_cards_and_never_truncates_their_limits() {
    let (_dir, runtime) = runtime();
    let mut large = finding(1, "oversized", "invalid");
    large["claim"] = json!("\\".repeat(1_200));
    large["limitations"] = json!("\\".repeat(1_200));
    large["applications"] = json!((0..8).map(|_| "\\".repeat(400)).collect::<Vec<_>>());
    let mut source = large["sources"][0].clone();
    source["path"] = json!("a".repeat(500));
    large["sources"] = json!(
        (0..4)
            .map(|n| {
                let mut reference = source.clone();
                reference["start_line"] = json!(n + 1);
                reference["end_line"] = json!(n + 1);
                reference
            })
            .collect::<Vec<_>>()
    );
    let mut cards = vec![large];
    cards.extend((2..8).map(|n| {
        let mut card = finding(n, &format!("finding-{n}"), "limited");
        card["claim"] = json!("結論。".repeat(100));
        card["limitations"] = json!("限界。".repeat(100));
        card
    }));
    let reviewed = catalog(cards);
    reviewed.sync(runtime.store()).unwrap();
    let result = reviewed
        .context(runtime.store(), "恒常性と負債の研究について")
        .unwrap();
    assert!(!result.selected.is_empty());
    assert!(result.selected.len() <= MAX_SELECTED_FINDINGS);
    assert!(serde_json::to_vec(&result).unwrap().len() <= MAX_CONTEXT_BYTES);
    assert!(
        result
            .selected
            .iter()
            .all(|selected| selected.finding.id != "oversized")
    );
    for selected in &result.selected {
        let complete = reviewed
            .findings
            .iter()
            .find(|finding| finding.id == selected.finding.id)
            .unwrap();
        assert_eq!(&selected.finding, complete);
        assert_eq!(selected.finding.limitations, "限界。".repeat(100));
        assert!(!selected.finding.sources.is_empty());
    }
}

#[test]
fn stored_chunk_tampering_is_rejected_even_if_artifact_digest_is_unchanged() {
    let (dir, runtime) = runtime();
    let reviewed = catalog(vec![finding(1, "g0-v6", "invalid")]);
    reviewed.sync(runtime.store()).unwrap();
    let connection = rusqlite::Connection::open(dir.path().join("kamimusuhi.sqlite")).unwrap();
    connection.execute(
        "UPDATE library_chunks SET text = 'tampered: capability acquired' WHERE artifact_id = ?1 AND ordinal = 0",
        [library_id(1).to_string()],
    ).unwrap();
    assert!(reviewed.context(runtime.store(), "G0-v6について").is_err());
    assert!(reviewed.sync(runtime.store()).is_err());
}
