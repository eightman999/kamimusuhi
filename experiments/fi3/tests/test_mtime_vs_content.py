"""Proof that hashing is content-based, not metadata-based:

* mtime-only change (touch) -> NOT a change (no false positive)
* content change with mtime preserved -> still detected
"""

from __future__ import annotations

import os

from experiments.fi3.cli.verify import (
    EXIT_MIGRATION_REQUIRED,
    EXIT_OK,
    verify_runtime,
)
from experiments.fi3.policies.compatibility import BREAKING, IDENTICAL
from experiments.fi3.tests.conftest import MODEL_V2, classify_runtime

_TRACKED = [
    "model/model.bin",
    "model/revision.txt",
    "tokenizer/tokenizer.json",
    "prompts/system_prompt.txt",
    "prompts/prompt_template.txt",
    "adapters/lora.bin",
    "config/runtime.json",
    "config/tools.json",
    "deps.lock",
]


def test_touch_all_files_no_change(runtime_dir, baseline_manifest):
    for rel in _TRACKED:
        os.utime(runtime_dir / rel, (1_700_000_000, 1_700_000_000))
    report, changes, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert changes == []
    assert report.overall == IDENTICAL


def test_content_change_with_preserved_mtime_detected(
    runtime_dir, baseline_manifest
):
    p = runtime_dir / "model/model.bin"
    st = p.stat()
    p.write_bytes(MODEL_V2)
    os.utime(p, (st.st_atime, st.st_mtime))  # restore original mtime
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == BREAKING
    assert any(e.change.component == "model.hash" for e in report.entries)


def test_verify_ignores_mtime_only(runtime_dir, state_dir):
    assert verify_runtime(runtime_dir, state_dir)["exit_code"] == EXIT_OK
    for rel in _TRACKED:
        os.utime(runtime_dir / rel, (1_800_000_000, 1_800_000_000))
    r = verify_runtime(runtime_dir, state_dir)
    assert r["exit_code"] == EXIT_OK
    assert r["overall"] == IDENTICAL


def test_verify_content_change_preserved_mtime(runtime_dir, state_dir):
    assert verify_runtime(runtime_dir, state_dir)["exit_code"] == EXIT_OK
    p = runtime_dir / "model/model.bin"
    st = p.stat()
    p.write_bytes(MODEL_V2)
    os.utime(p, (st.st_atime, st.st_mtime))
    r = verify_runtime(runtime_dir, state_dir)
    assert r["exit_code"] == EXIT_MIGRATION_REQUIRED
