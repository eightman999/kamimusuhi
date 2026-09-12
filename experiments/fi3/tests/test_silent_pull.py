"""Silent Pull Test — model file replacement, symlink replacement,
HuggingFace revision bump, tokenizer replacement.  All must be detected.
"""

from __future__ import annotations

import os
import shutil

import pytest

from experiments.fi3.cli.verify import (
    EXIT_MIGRATION_REQUIRED,
    EXIT_OK,
    EXIT_REVIEW_REQUIRED,
    verify_runtime,
)
from experiments.fi3.identity.manifest import collect_manifest
from experiments.fi3.policies.compatibility import BREAKING, REVIEW
from experiments.fi3.tests.conftest import (
    MODEL_V1,
    MODEL_V2,
    classify_runtime,
    make_runtime_dir,
)

EVIL_WEIGHTS = b"FI3-EVIL-WEIGHTS: silent pull\n" + bytes(range(64))


def test_model_file_replacement(runtime_dir, baseline_manifest):
    (runtime_dir / "model/model.bin").write_bytes(EVIL_WEIGHTS)
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == BREAKING


def test_model_symlink_swap_to_different_content(
    runtime_dir, baseline_manifest
):
    # model.bin replaced by a symlink pointing at different weights
    evil = runtime_dir / "model/weights-evil.bin"
    evil.write_bytes(EVIL_WEIGHTS)
    os.remove(runtime_dir / "model/model.bin")
    os.symlink("weights-evil.bin", runtime_dir / "model/model.bin")
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == BREAKING  # resolved content differs


def test_model_symlink_swap_to_identical_content_still_detected(
    runtime_dir, baseline_manifest
):
    # Same bytes, different file — a pure link-level replacement.
    clone = runtime_dir / "model/weights-copy.bin"
    clone.write_bytes(MODEL_V1)
    os.remove(runtime_dir / "model/model.bin")
    os.symlink("weights-copy.bin", runtime_dir / "model/model.bin")
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    # content hash is equal, but model.link changed -> detected as REVIEW
    assert any(
        e.change.component == "model.link" for e in report.entries
    )
    assert report.overall == REVIEW
    assert not any(
        e.change.component == "model.hash" for e in report.entries
    )


def test_existing_symlink_retarget(tmp_path):
    # Baseline where model.bin is already a symlink; attacker retargets it.
    rd = tmp_path / "rt"
    make_runtime_dir(rd)
    os.remove(rd / "model/model.bin")
    (rd / "model/weights-v1.bin").write_bytes(MODEL_V1)
    (rd / "model/weights-v2.bin").write_bytes(MODEL_V2)
    os.symlink("weights-v1.bin", rd / "model/model.bin")
    base = collect_manifest(rd)

    os.remove(rd / "model/model.bin")
    os.symlink("weights-v2.bin", rd / "model/model.bin")
    report, _, _ = classify_runtime(base, rd)
    assert report.overall == BREAKING  # both link and content changed
    comps = {e.change.component for e in report.entries}
    assert {"model.link", "model.hash"} <= comps


def test_existing_symlink_retarget_same_content(tmp_path):
    rd = tmp_path / "rt"
    make_runtime_dir(rd)
    os.remove(rd / "model/model.bin")
    (rd / "model/w-a.bin").write_bytes(MODEL_V1)
    (rd / "model/w-b.bin").write_bytes(MODEL_V1)  # same bytes, other name
    os.symlink("w-a.bin", rd / "model/model.bin")
    base = collect_manifest(rd)

    os.remove(rd / "model/model.bin")
    os.symlink("w-b.bin", rd / "model/model.bin")
    report, _, _ = classify_runtime(base, rd)
    assert report.overall == REVIEW  # link-level retarget detected


def test_hf_revision_bump_only(runtime_dir, baseline_manifest):
    # weights untouched, only revision metadata moved forward
    (runtime_dir / "model/revision.txt").write_text("hf-rev-0002\n")
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == BREAKING
    assert any(e.change.component == "model.revision" for e in report.entries)


def test_whole_model_dir_swap(tmp_path):
    rd = make_runtime_dir(tmp_path / "rt")
    base = collect_manifest(rd)
    # simulate an atomic directory swap: model/ renamed, lookalike moved in
    shutil.move(str(rd / "model"), str(rd / "model.orig"))
    (rd / "model").mkdir()
    (rd / "model/model.bin").write_bytes(EVIL_WEIGHTS)
    (rd / "model/revision.txt").write_text("hf-rev-0001\n")
    report, _, _ = classify_runtime(base, rd)
    assert report.overall == BREAKING


def test_tokenizer_symlink_swap(runtime_dir, baseline_manifest):
    evil = runtime_dir / "tokenizer/tok-evil.json"
    evil.write_bytes(b'{"tokenizer":"evil"}')
    os.remove(runtime_dir / "tokenizer/tokenizer.json")
    os.symlink("tok-evil.json", runtime_dir / "tokenizer/tokenizer.json")
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == BREAKING


def test_broken_symlink_detected(runtime_dir, baseline_manifest):
    os.remove(runtime_dir / "model/model.bin")
    os.symlink("nonexistent.bin", runtime_dir / "model/model.bin")
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == BREAKING
    assert any(e.change.component == "model.link" for e in report.entries)


def test_verify_end_to_end_silent_pull(runtime_dir, state_dir):
    assert verify_runtime(runtime_dir, state_dir)["exit_code"] == EXIT_OK
    evil = runtime_dir / "model/weights-evil.bin"
    evil.write_bytes(EVIL_WEIGHTS)
    os.remove(runtime_dir / "model/model.bin")
    os.symlink("weights-evil.bin", runtime_dir / "model/model.bin")
    r = verify_runtime(runtime_dir, state_dir)
    assert r["exit_code"] == EXIT_MIGRATION_REQUIRED


def test_verify_identical_content_retarget_is_review(runtime_dir, state_dir):
    assert verify_runtime(runtime_dir, state_dir)["exit_code"] == EXIT_OK
    clone = runtime_dir / "model/weights-copy.bin"
    clone.write_bytes(MODEL_V1)
    os.remove(runtime_dir / "model/model.bin")
    os.symlink("weights-copy.bin", runtime_dir / "model/model.bin")
    r = verify_runtime(runtime_dir, state_dir)
    assert r["exit_code"] == EXIT_REVIEW_REQUIRED
    r2 = verify_runtime(runtime_dir, state_dir, accept_review=True)
    assert r2["exit_code"] == EXIT_OK
