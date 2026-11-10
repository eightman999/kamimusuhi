//! Keep scientific dispositions, evidence revisions and runtime admission in
//! agreement. A report changing underneath a decision requires another audit.
use std::collections::BTreeSet;
use std::path::{Component, PathBuf};

use kamimusuhi_core::digest::content_digest;
use kamimusuhi_core::organs::PromotionMode;
use kamimusuhi_runtime::organs::validated_experiment_manifest;
use serde_json::Value;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn catalog() -> Value {
    serde_json::from_str(include_str!(
        "../../../knowledge/experiment-dispositions.json"
    ))
    .unwrap()
}

#[test]
fn every_current_research_directory_has_one_explicit_decision() {
    let catalog = catalog();
    assert_eq!(catalog["schema_version"], 1);
    let rows = catalog["experiments"].as_array().unwrap();
    let names: BTreeSet<String> = rows
        .iter()
        .map(|row| row["folder"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names.len(), rows.len(), "duplicate research directory");
    let actual: BTreeSet<String> = std::fs::read_dir(root().join("experiments"))
        .unwrap()
        .map(Result::unwrap)
        .filter(|entry| entry.file_type().unwrap().is_dir())
        .map(|entry| entry.file_name().into_string().unwrap())
        .filter(|name| name != "__pycache__" && !name.starts_with('.'))
        .collect();
    assert_eq!(
        names, actual,
        "new or missing experiment needs disposition review"
    );
    for row in rows {
        assert!(matches!(
            row["disposition"].as_str(),
            Some("production_candidate" | "continue" | "freeze" | "reject")
        ));
        assert!(!row["reason"].as_str().unwrap().is_empty());
        assert!(!row["next_gate"].as_str().unwrap().is_empty());
        assert_eq!(row["production_approved"], false);
    }
}

#[test]
fn evidence_content_is_pinned_and_implementation_is_not_a_result() {
    let catalog = catalog();
    for row in catalog["experiments"].as_array().unwrap() {
        let evidence = row["evidence"].as_array().unwrap();
        assert!(!evidence.is_empty());
        for source in evidence {
            let path = std::path::Path::new(source["path"].as_str().unwrap());
            assert!(path.components().all(|c| matches!(c, Component::Normal(_))));
            let file = root().join(path);
            if !file.exists() {
                assert_eq!(source["required_in_checkout"], false, "missing {path:?}");
                continue;
            }
            assert!(
                file.canonicalize()
                    .unwrap()
                    .starts_with(root().canonicalize().unwrap())
            );
            let digest = content_digest(&std::fs::read(file).unwrap());
            assert_eq!(source["sha256"], digest, "evidence drift: {path:?}");
        }
        if row["disposition"] == "production_candidate" {
            assert!(evidence.iter().any(|source| source["kind"] == "report"));
        }
    }
    // A bug-invalidated aggregate is not promoted to a successful replication.
    let g0v6 = catalog["experiments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["folder"] == "g0_v6")
        .unwrap();
    assert_eq!(g0v6["disposition"], "continue");
    assert!(
        g0v6["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| !e["path"].as_str().unwrap().contains("r1_invalid"))
    );
}

#[test]
fn runtime_eligibility_agrees_with_the_reviewed_dispositions() {
    let catalog = catalog();
    let rows = catalog["experiments"].as_array().unwrap();
    let manifest = validated_experiment_manifest();
    for descriptor in &manifest {
        let row = rows
            .iter()
            .find(|row| row["runtime_key"].as_str() == Some(descriptor.key.as_str()))
            .unwrap();
        assert_eq!(row["id"], descriptor.evidence.experiment);
        assert_eq!(row["runtime_mode"], "shadow");
        assert_eq!(descriptor.promotion, PromotionMode::Shadow);
        assert!(!descriptor.influences_cognition());
    }
    for row in rows {
        if row["runtime_mode"] == "excluded" {
            assert!(row["runtime_key"].is_null());
        } else {
            assert!(
                manifest
                    .iter()
                    .any(|d| row["runtime_key"].as_str() == Some(d.key.as_str()))
            );
        }
    }
}
