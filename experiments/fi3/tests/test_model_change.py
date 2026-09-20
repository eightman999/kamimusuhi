"""Model-swap detection — success criterion: 100% detection, BREAKING,
no silent continuation (state -> MIGRATION_REQUIRED)."""

from __future__ import annotations

import json
import os

import pytest

from experiments.fi3.cli.verify import (
    EXIT_MIGRATION_REQUIRED,
    EXIT_OK,
    verify_runtime,
)
from experiments.fi3.identity.manifest import collect_manifest
from experiments.fi3.policies.compatibility import BREAKING
from experiments.fi3.state import STATUS_MIGRATION_REQUIRED, load_state
from experiments.fi3.tests.conftest import (
    ADAPTER_V2,
    MODEL_V1,
    classify_runtime,
    edit_config,
    make_runtime_dir,
)

# 10 deterministic replacement payloads -> detection rate measured over all.
MODEL_VARIANTS = [
    b"FI3-FAKE-MODEL-WEIGHTS evil-%02d\n" % i + bytes([i]) * 128
    for i in range(10)
]


@pytest.mark.parametrize("payload", MODEL_VARIANTS)
def test_model_file_swap_detected_breaking(
    runtime_dir, baseline_manifest, payload
):
    (runtime_dir / "model/model.bin").write_bytes(payload)
    report, changes, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == BREAKING
    assert any(
        e.change.component == "model.hash" and e.classification == BREAKING
        for e in report.entries
    )


def test_model_name_change_breaking(runtime_dir, baseline_manifest):
    edit_config(runtime_dir, lambda c: c["model"].__setitem__("name", "other-13b"))
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == BREAKING
    assert any(
        e.change.component == "model.name" and e.classification == BREAKING
        for e in report.entries
    )


def test_model_revision_bump_breaking(runtime_dir, baseline_manifest):
    # 'New HuggingFace revision' with identical weights is still BREAKING:
    # the revision is part of model identity.
    (runtime_dir / "model/revision.txt").write_text("hf-rev-0002\n")
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == BREAKING
    assert any(
        e.change.component == "model.revision" and e.classification == BREAKING
        for e in report.entries
    )


def test_adapter_weights_change_breaking(runtime_dir, baseline_manifest):
    (runtime_dir / "adapters/lora.bin").write_bytes(ADAPTER_V2)
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == BREAKING
    assert any(
        e.change.component == "adapter.hash" and e.classification == BREAKING
        for e in report.entries
    )


def test_adapter_removed_breaking(runtime_dir, baseline_manifest):
    os.remove(runtime_dir / "adapters/lora.bin")
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == BREAKING
    comps = {e.change.component for e in report.entries}
    assert {"adapter.present", "adapter.hash"} <= comps


def test_adapter_added_breaking(tmp_path):
    rd = make_runtime_dir(tmp_path / "rt", with_adapter=False)
    base = collect_manifest(rd)
    (rd / "adapters/lora.bin").write_bytes(ADAPTER_V2)
    report, _, _ = classify_runtime(base, rd)
    assert report.overall == BREAKING


def test_model_deleted_breaking(runtime_dir, baseline_manifest):
    os.remove(runtime_dir / "model/model.bin")
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == BREAKING


def test_verify_breaking_blocks_and_records_migration(runtime_dir, state_dir):
    r0 = verify_runtime(runtime_dir, state_dir)
    assert r0["exit_code"] == EXIT_OK
    assert r0["verdict"] == "baseline_initialized"

    (runtime_dir / "model/model.bin").write_bytes(MODEL_VARIANTS[0])
    r1 = verify_runtime(runtime_dir, state_dir)
    assert r1["exit_code"] == EXIT_MIGRATION_REQUIRED
    assert r1["overall"] == BREAKING
    assert r1["decision"]["allowed"] is False

    st = load_state(state_dir)
    assert st["status"] == STATUS_MIGRATION_REQUIRED
    # baseline pinned to the *old* manifest
    assert st["baseline"]["identity_hash"] == r1["baseline_identity_hash"]

    # migration hook output: old manifest / new manifest / diff / class.
    mig = json.loads(
        (state_dir / "migration_required.json").read_text()
    )
    assert mig["old_manifest"]["components"]["model"]["hash"] != \
        mig["new_manifest"]["components"]["model"]["hash"]
    assert mig["classification"]["overall"] == BREAKING
    assert any(d["component"] == "model.hash" for d in mig["diff"])

    # Retrying while still broken keeps blocking.
    r2 = verify_runtime(runtime_dir, state_dir)
    assert r2["exit_code"] == EXIT_MIGRATION_REQUIRED

    # Reverting to the true baseline identity clears the guard.
    (runtime_dir / "model/model.bin").write_bytes(MODEL_V1)
    r3 = verify_runtime(runtime_dir, state_dir)
    assert r3["exit_code"] == EXIT_OK
    assert load_state(state_dir)["status"] == "OK"
