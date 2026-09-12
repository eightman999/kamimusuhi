"""Sampling Identity — sampling drift is never 'same runtime'.

Every sampling parameter change must be recorded and classified at
least REVIEW; continuation requires explicit --accept-review.
"""

from __future__ import annotations

import pytest

from experiments.fi3.cli.verify import (
    EXIT_OK,
    EXIT_REVIEW_REQUIRED,
    verify_runtime,
)
from experiments.fi3.identity.manifest import (
    collect_manifest,
    runtime_identity_hash,
)
from experiments.fi3.policies.compatibility import REVIEW, severity_rank
from experiments.fi3.tests.conftest import (
    classify_runtime,
    edit_config,
)

SAMPLING_MUTATIONS = [
    ("temperature", 0.7, 1.2),
    ("top_p", 0.9, 0.5),
    ("top_k", 40, 1),
    ("seed", 1234, 9999),
    ("repetition_penalty", 1.05, 1.3),
    ("context_length", 4096, 8192),
]


@pytest.mark.parametrize(
    "key,old,new", [(k, o, n) for k, o, n in SAMPLING_MUTATIONS]
)
def test_each_sampling_param_is_review(
    runtime_dir, baseline_manifest, baseline_identity, key, old, new
):
    edit_config(runtime_dir, lambda c: c["sampling"].__setitem__(key, new))
    report, _, current = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == REVIEW
    entry = next(
        e for e in report.entries if e.change.component == f"sampling.{key}"
    )
    assert entry.classification == REVIEW
    assert entry.change.old == old and entry.change.new == new
    # 'same model' is NOT 'same runtime': identity must differ
    assert runtime_identity_hash(current) != baseline_identity


def test_all_sampling_params_together(runtime_dir, baseline_manifest):
    def mutate(c):
        for k, _, n in SAMPLING_MUTATIONS:
            c["sampling"][k] = n

    edit_config(runtime_dir, mutate)
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == REVIEW
    changed = {e.change.component for e in report.entries}
    assert changed == {f"sampling.{k}" for k, _, _ in SAMPLING_MUTATIONS}


def test_sampling_drift_never_safe(runtime_dir, baseline_manifest):
    # even a 'harmless-looking' seed change floors at REVIEW
    edit_config(runtime_dir, lambda c: c["sampling"].__setitem__("seed", 0))
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert severity_rank(report.overall) >= severity_rank(REVIEW)


def test_verify_sampling_review_gate(runtime_dir, state_dir):
    assert verify_runtime(runtime_dir, state_dir)["exit_code"] == EXIT_OK
    edit_config(
        runtime_dir, lambda c: c["sampling"].__setitem__("temperature", 1.0)
    )
    r = verify_runtime(runtime_dir, state_dir)
    assert r["exit_code"] == EXIT_REVIEW_REQUIRED
    assert r["decision"]["allowed"] is False

    r2 = verify_runtime(runtime_dir, state_dir, accept_review=True)
    assert r2["exit_code"] == EXIT_OK

    # baseline advanced: same runtime now verifies IDENTICAL
    r3 = verify_runtime(runtime_dir, state_dir)
    assert r3["exit_code"] == EXIT_OK
    assert r3["overall"] == "IDENTICAL"
