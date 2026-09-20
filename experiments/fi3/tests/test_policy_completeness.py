"""Policy completeness — every manifest field must have a classification
rule; unknown components default to conservative BREAKING."""

from __future__ import annotations

import copy

import pytest

from experiments.fi3.identity.canonical import flatten_components
from experiments.fi3.identity.diff import diff_manifests
from experiments.fi3.identity.manifest import COMPONENT_SCHEMA
from experiments.fi3.policies.compatibility import (
    BREAKING,
    CLASSIFICATIONS,
    POLICY_TABLE,
    PolicyGap,
    assert_policy_completeness,
    classify_changes,
    expected_fields,
)


def test_every_manifest_field_has_policy():
    fields = set(expected_fields())
    missing = fields - set(POLICY_TABLE)
    assert not missing, f"manifest fields with no policy: {missing}"


def test_no_stale_policy_keys():
    stale = set(POLICY_TABLE) - set(expected_fields())
    assert not stale, f"policy keys with no manifest field: {stale}"


def test_completeness_check_passes_on_real_table():
    assert_policy_completeness()  # must not raise


def test_completeness_check_fails_on_gap():
    table = dict(POLICY_TABLE)
    del table["model.hash"]
    with pytest.raises(PolicyGap, match="model.hash"):
        assert_policy_completeness(table=table)


def test_completeness_check_fails_on_extra_key():
    table = dict(POLICY_TABLE)
    table["bogus.field"] = "SAFE"
    with pytest.raises(PolicyGap, match="bogus.field"):
        assert_policy_completeness(table=table)


def test_completeness_check_fails_on_new_schema_field():
    # simulate a schema field added without a policy
    fake = set(expected_fields()) | {"future.new_field"}
    with pytest.raises(PolicyGap):
        assert_policy_completeness(fields=fake)


def test_collected_manifest_fields_covered(baseline_manifest):
    flat = flatten_components(baseline_manifest["components"])
    for field in flat:
        assert field in POLICY_TABLE, f"unclassified manifest field: {field}"


def test_unknown_component_is_breaking(baseline_manifest, runtime_dir):
    from experiments.fi3.identity.manifest import collect_manifest

    current = collect_manifest(runtime_dir)
    current["components"]["mystery"] = {"injected": "x"}
    changes = diff_manifests(baseline_manifest, current)
    report = classify_changes(changes)
    assert report.overall == BREAKING
    entry = next(
        e for e in report.entries if e.change.component == "mystery.injected"
    )
    assert entry.classification == BREAKING
    assert "no policy" in entry.reason


def test_removed_component_is_breaking(baseline_manifest):
    mutant = copy.deepcopy(baseline_manifest)
    del mutant["components"]["model"]["hash"]
    changes = diff_manifests(baseline_manifest, mutant)
    report = classify_changes(changes)
    assert report.overall == BREAKING


def test_manifest_version_bump_is_breaking(baseline_manifest):
    mutant = copy.deepcopy(baseline_manifest)
    mutant["manifest_version"] = 99
    changes = diff_manifests(baseline_manifest, mutant)
    report = classify_changes(changes)
    assert report.overall == BREAKING


def test_every_classified_change_has_reason(baseline_manifest, runtime_dir):
    from experiments.fi3.tests.conftest import edit_config, classify_runtime

    edit_config(
        runtime_dir, lambda c: c["sampling"].__setitem__("temperature", 2.0)
    )
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    for e in report.entries:
        assert e.classification in CLASSIFICATIONS
        assert e.reason
