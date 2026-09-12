"""Tokenizer-change detection — success criterion: 100% detection."""

from __future__ import annotations

import json
import os

import pytest

from experiments.fi3.cli.verify import EXIT_MIGRATION_REQUIRED, EXIT_OK, verify_runtime
from experiments.fi3.policies.compatibility import BREAKING
from experiments.fi3.state import STATUS_MIGRATION_REQUIRED, load_state
from experiments.fi3.tests.conftest import classify_runtime, edit_config

# 10 deterministic tokenizer variants: vocab/merge/name tweaks.
TOKENIZER_VARIANTS = [
    json.dumps(
        {
            "tokenizer": "kami-tok-v1",
            "vocab": {"<s>": 0, "</s>": 1, "hello": 2, "world": 3, f"extra{i}": 4},
            "merges": ["h e", "he l", "l o"],
        },
        sort_keys=True,
        indent=2,
    ).encode()
    for i in range(10)
]


@pytest.mark.parametrize("payload", TOKENIZER_VARIANTS)
def test_tokenizer_file_swap_detected(runtime_dir, baseline_manifest, payload):
    (runtime_dir / "tokenizer/tokenizer.json").write_bytes(payload)
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == BREAKING
    assert any(
        e.change.component == "tokenizer.hash"
        and e.classification == BREAKING
        for e in report.entries
    )


def test_tokenizer_name_change_breaking(runtime_dir, baseline_manifest):
    edit_config(
        runtime_dir, lambda c: c["tokenizer"].__setitem__("name", "kami-tok-v2")
    )
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == BREAKING
    assert any(
        e.change.component == "tokenizer.name"
        and e.classification == BREAKING
        for e in report.entries
    )


def test_tokenizer_deleted_breaking(runtime_dir, baseline_manifest):
    os.remove(runtime_dir / "tokenizer/tokenizer.json")
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == BREAKING


def test_single_byte_vocab_flip_detected(runtime_dir, baseline_manifest):
    # adversarial minimal mutation: one byte inside tokenizer.json
    p = runtime_dir / "tokenizer/tokenizer.json"
    data = bytearray(p.read_bytes())
    idx = data.index(b"hello")
    data[idx] = ord("j")  # hello -> jello
    p.write_bytes(bytes(data))
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == BREAKING


def test_verify_tokenizer_swap_blocks(runtime_dir, state_dir):
    assert verify_runtime(runtime_dir, state_dir)["exit_code"] == EXIT_OK
    (runtime_dir / "tokenizer/tokenizer.json").write_bytes(
        TOKENIZER_VARIANTS[0]
    )
    r = verify_runtime(runtime_dir, state_dir)
    assert r["exit_code"] == EXIT_MIGRATION_REQUIRED
    assert load_state(state_dir)["status"] == STATUS_MIGRATION_REQUIRED
