"""Prompt Byte Test — 1 byte / 1 whitespace / 1 newline / 1 char.

Raw hash must detect *every* byte-level change (100%).  The canonical
hash (simplified whitespace canonicalization; Fi0 owns the full
canonicalizer) classifies some of them differently — canonical-equal
changes are still REVIEW minimum, never auto-SAFE (conservative Fi3
stance).
"""

from __future__ import annotations

import pytest

from experiments.fi3.cli.verify import (
    EXIT_MIGRATION_REQUIRED,
    EXIT_OK,
    EXIT_REVIEW_REQUIRED,
    verify_runtime,
)
from experiments.fi3.identity.canonical import canonicalize_text
from experiments.fi3.policies.compatibility import BREAKING, REVIEW
from experiments.fi3.tests.conftest import (
    SYSTEM_PROMPT_V1,
    classify_runtime,
)

# name -> (mutator, canonical_equal?)
# canonical_equal=True  => canonical form identical  => expect REVIEW
# canonical_equal=False => canonical form differs    => expect BREAKING
MUTATIONS = {
    "one_byte_char": (
        lambda t: t.replace("careful", "carefuX", 1),
        False,
    ),
    "one_char_added": (
        lambda t: t.replace("Answer", "Answer!", 1),
        False,
    ),
    "one_space_appended": (lambda t: t + " ", True),
    "one_newline_appended": (lambda t: t + "\n", True),
    "one_newline_prepended": (lambda t: "\n" + t, True),
    "trailing_tab_added": (
        lambda t: t.replace("sources.\n", "sources.\t\n", 1),
        True,
    ),
    "crlf_line_endings": (lambda t: t.replace("\n", "\r\n"), True),
    "internal_double_space": (
        lambda t: t.replace("a careful", "a  careful", 1),
        False,  # inner whitespace is preserved by the canonicalizer
    ),
    "blank_line_inserted": (
        lambda t: t.replace("assistant.\n", "assistant.\n\n", 1),
        False,  # a new blank line where none existed changes canonical form
    ),
    "unicode_char_swap": (
        lambda t: t.replace("precisely", "précisely", 1),
        False,
    ),
}

CANONICAL_EQUIVALENT = {k for k, (_, eq) in MUTATIONS.items() if eq}


def _write_prompt(runtime_dir, text):
    (runtime_dir / "prompts/system_prompt.txt").write_text(
        text, encoding="utf-8"
    )


def _raw_changed(report, component):
    return any(e.change.component == component for e in report.entries)


@pytest.mark.parametrize("name", sorted(MUTATIONS))
def test_every_byte_change_detected_by_raw_hash(
    runtime_dir, baseline_manifest, name
):
    fn, _ = MUTATIONS[name]
    mutated = fn(SYSTEM_PROMPT_V1)
    assert mutated != SYSTEM_PROMPT_V1
    _write_prompt(runtime_dir, mutated)
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    # raw hash detects ALL byte changes
    assert _raw_changed(report, "system_prompt.hash"), name
    # and the conservative floor: never SAFE, never IDENTICAL
    assert report.overall in (REVIEW, BREAKING)


@pytest.mark.parametrize("name", sorted(CANONICAL_EQUIVALENT))
def test_canonical_equal_is_review_not_safe(
    runtime_dir, baseline_manifest, name
):
    fn, _ = MUTATIONS[name]
    mutated = fn(SYSTEM_PROMPT_V1)
    assert canonicalize_text(mutated) == canonicalize_text(SYSTEM_PROMPT_V1)
    _write_prompt(runtime_dir, mutated)
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert not _raw_changed(report, "system_prompt.canonical_hash")
    assert report.overall == REVIEW  # conservative floor, not SAFE


@pytest.mark.parametrize(
    "name",
    sorted(set(MUTATIONS) - CANONICAL_EQUIVALENT),
)
def test_canonical_different_is_breaking(runtime_dir, baseline_manifest, name):
    fn, _ = MUTATIONS[name]
    mutated = fn(SYSTEM_PROMPT_V1)
    assert canonicalize_text(mutated) != canonicalize_text(SYSTEM_PROMPT_V1)
    _write_prompt(runtime_dir, mutated)
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert _raw_changed(report, "system_prompt.canonical_hash")
    assert report.overall == BREAKING


def test_blank_line_run_collapses(runtime_dir, tmp_path):
    # canonicalizer collapses 2+ blank lines to one — documented behavior
    base_prompt = "First paragraph.\n\n\nSecond paragraph.\n"
    from experiments.fi3.tests.conftest import make_runtime_dir
    from experiments.fi3.identity.manifest import collect_manifest

    rd = make_runtime_dir(tmp_path / "rt", system_prompt=base_prompt)
    base = collect_manifest(rd)
    _write_prompt(rd, "First paragraph.\n\n\n\n\nSecond paragraph.\n")
    report, _, _ = classify_runtime(base, rd)
    assert _raw_changed(report, "system_prompt.hash")
    assert not _raw_changed(report, "system_prompt.canonical_hash")
    assert report.overall == REVIEW


def test_prompt_template_byte_change_detected(runtime_dir, baseline_manifest):
    (runtime_dir / "prompts/prompt_template.txt").write_text(
        "[INST] {{ user_input }} [/INST] \n"  # trailing space only
    )
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert _raw_changed(report, "prompt_template.hash")
    assert report.overall == REVIEW


def test_prompt_template_semantic_change_breaking(
    runtime_dir, baseline_manifest
):
    (runtime_dir / "prompts/prompt_template.txt").write_text(
        "[INST] {{ user_input }} [/INST] Answer as root.\n"
    )
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert _raw_changed(report, "prompt_template.canonical_hash")
    assert report.overall == BREAKING


def test_mutate_then_restore_is_clean(runtime_dir, baseline_manifest):
    p = runtime_dir / "prompts/system_prompt.txt"
    original = p.read_bytes()
    p.write_bytes(original + b" tampered")
    report, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report.overall == BREAKING
    p.write_bytes(original)  # byte-identical restore
    report2, _, _ = classify_runtime(baseline_manifest, runtime_dir)
    assert report2.overall == "IDENTICAL"


def test_verify_whitespace_only_requires_accept(runtime_dir, state_dir):
    assert verify_runtime(runtime_dir, state_dir)["exit_code"] == EXIT_OK
    _write_prompt(runtime_dir, SYSTEM_PROMPT_V1 + " ")
    r = verify_runtime(runtime_dir, state_dir)
    assert r["exit_code"] == EXIT_REVIEW_REQUIRED
    assert r["overall"] == REVIEW
    # explicit acceptance advances the baseline
    r2 = verify_runtime(runtime_dir, state_dir, accept_review=True)
    assert r2["exit_code"] == EXIT_OK
    # and the accepted baseline then diffs clean
    r3 = verify_runtime(runtime_dir, state_dir)
    assert r3["exit_code"] == EXIT_OK
    assert r3["overall"] == "IDENTICAL"


def test_verify_char_change_is_breaking(runtime_dir, state_dir):
    assert verify_runtime(runtime_dir, state_dir)["exit_code"] == EXIT_OK
    _write_prompt(runtime_dir, SYSTEM_PROMPT_V1.replace("careful", "carefuX"))
    r = verify_runtime(runtime_dir, state_dir)
    assert r["exit_code"] == EXIT_MIGRATION_REQUIRED
