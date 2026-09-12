"""False-positive control — identical runtimes must never trigger.

Success criterion 6: identical runtime -> zero false positives.
"""

from __future__ import annotations

import json
import os
import random
import shutil

from experiments.fi3.cli.verify import EXIT_OK, verify_runtime
from experiments.fi3.identity.manifest import (
    collect_manifest,
    runtime_identity_hash,
)
from experiments.fi3.policies.compatibility import IDENTICAL, SAFE
from experiments.fi3.tests.conftest import (
    classify_runtime,
    edit_config,
    make_runtime_dir,
)


def test_identical_runtime_no_changes(runtime_dir, baseline_manifest):
    report, changes, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert changes == []
    assert report.overall == IDENTICAL


def test_manifest_collection_deterministic(runtime_dir):
    m1 = collect_manifest(runtime_dir)
    m2 = collect_manifest(runtime_dir)
    assert m1 == m2
    assert runtime_identity_hash(m1) == runtime_identity_hash(m2)


def test_rebuilt_fixture_identical(tmp_path):
    rd1 = make_runtime_dir(tmp_path / "r1")
    rd2 = make_runtime_dir(tmp_path / "r2")
    assert runtime_identity_hash(collect_manifest(rd1)) == runtime_identity_hash(
        collect_manifest(rd2)
    )


def test_copytree_same_identity(runtime_dir, tmp_path):
    dst = tmp_path / "runtime-copy"
    shutil.copytree(runtime_dir, dst, symlinks=True)
    assert runtime_identity_hash(collect_manifest(dst)) == (
        runtime_identity_hash(collect_manifest(runtime_dir))
    )


def test_runtime_json_key_order_irrelevant(runtime_dir, baseline_manifest):
    p = runtime_dir / "config/runtime.json"
    cfg = json.loads(p.read_text())
    shuffled = dict(reversed(list(cfg.items())))
    p.write_text(json.dumps(shuffled, indent=2))
    report, changes, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert changes == []
    assert report.overall == IDENTICAL


def test_operational_changes_are_safe(runtime_dir, baseline_manifest):
    def mutate(c):
        c["operational"]["log_dir"] = "logs/secondary"
        c["operational"]["ui_theme"] = "light"

    edit_config(runtime_dir, mutate)
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == SAFE
    classes = {e.change.component: e.classification for e in report.entries}
    assert classes["operational.log_dir"] == SAFE
    assert classes["operational.ui_theme"] == SAFE


def test_verify_identical_continues(runtime_dir, state_dir):
    assert verify_runtime(runtime_dir, state_dir)["exit_code"] == EXIT_OK
    for _ in range(3):
        r = verify_runtime(runtime_dir, state_dir)
        assert r["exit_code"] == EXIT_OK
        assert r["overall"] == IDENTICAL


def test_verify_safe_change_continues(runtime_dir, state_dir):
    assert verify_runtime(runtime_dir, state_dir)["exit_code"] == EXIT_OK
    edit_config(
        runtime_dir,
        lambda c: c["operational"].__setitem__("ui_theme", "light"),
    )
    r = verify_runtime(runtime_dir, state_dir)
    assert r["exit_code"] == EXIT_OK
    assert r["overall"] == SAFE
    # baseline advanced: next run is clean
    assert verify_runtime(runtime_dir, state_dir)["overall"] == IDENTICAL


def test_random_rechecks_no_false_positive(tmp_path):
    # deterministic 'fuzz': rebuild + verify N times with a fixed seed
    rng = random.Random(0)
    rd = make_runtime_dir(tmp_path / "rt")
    sd = tmp_path / "state"
    assert verify_runtime(rd, sd)["exit_code"] == EXIT_OK
    for i in range(20):
        # randomly rebuild the identical runtime and re-verify
        if rng.random() < 0.5:
            rd = make_runtime_dir(tmp_path / f"rt{i}")
        r = verify_runtime(rd, sd)
        assert r["exit_code"] == EXIT_OK, i
        assert r["overall"] == IDENTICAL, i
