"""Schema/version detection — memory schema, tool schema, runtime
version, dependency lock.  Success criterion: memory schema change
detected; spec classifications hold (minor -> REVIEW, major ->
BREAKING)."""

from __future__ import annotations

import json

import pytest

from experiments.fi3.policies.compatibility import BREAKING, REVIEW
from experiments.fi3.tests.conftest import classify_runtime, edit_config


def _set_version(runtime_dir, key, value):
    edit_config(runtime_dir, lambda c: c["versions"].__setitem__(key, value))


# ---- memory schema ---------------------------------------------------

@pytest.mark.parametrize(
    "new_version,expected",
    [
        ("2.0.0", BREAKING),   # incompatible major bump
        ("1.1.0", REVIEW),     # compatible minor bump
        ("1.0.1", REVIEW),     # patch
        ("garbage", BREAKING), # unparseable -> conservative
    ],
)
def test_memory_schema_version_change(
    runtime_dir, baseline_manifest, new_version, expected
):
    _set_version(runtime_dir, "memory_schema", new_version)
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == expected
    assert any(
        e.change.component == "versions.memory_schema" for e in report.entries
    )


# ---- runtime version ---------------------------------------------------

@pytest.mark.parametrize(
    "new_version,expected",
    [
        ("1.0.0", BREAKING),  # runtime major -> BREAKING
        ("0.4.0", REVIEW),    # runtime minor -> REVIEW
        ("0.3.2", REVIEW),    # patch -> REVIEW
    ],
)
def test_runtime_version_change(
    runtime_dir, baseline_manifest, new_version, expected
):
    _set_version(runtime_dir, "runtime", new_version)
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == expected


# ---- tool schema --------------------------------------------------------

def test_tool_schema_version_bump_review(runtime_dir, baseline_manifest):
    _set_version(runtime_dir, "tool_schema", "2.2.0")
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == REVIEW


def test_tool_schema_file_content_change_review(
    runtime_dir, baseline_manifest
):
    tools = json.loads(
        (runtime_dir / "config/tools.json").read_text()
    )
    tools["tools"].append({"name": "exec", "args": {"cmd": "str"}})
    (runtime_dir / "config/tools.json").write_text(
        json.dumps(tools, sort_keys=True, indent=2)
    )
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == REVIEW
    assert any(
        e.change.component == "tool_schema.hash" for e in report.entries
    )


# ---- dependency lock -----------------------------------------------------

def test_dependency_lock_change_review(runtime_dir, baseline_manifest):
    (runtime_dir / "deps.lock").write_text("numpy==2.2.0\npytest==8.3.4\n")
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == REVIEW
    assert any(
        e.change.component == "dependency_lock.hash" for e in report.entries
    )


def test_dependency_lock_deleted(runtime_dir, baseline_manifest):
    (runtime_dir / "deps.lock").unlink()
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == REVIEW  # lockfile loss is still detected
