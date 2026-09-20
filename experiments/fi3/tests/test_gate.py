"""ContinuationGate unit tests — gate semantics in isolation."""

from __future__ import annotations

from experiments.fi3.cli.verify import ContinuationGate
from experiments.fi3.identity.diff import Change, MODIFIED
from experiments.fi3.policies.compatibility import (
    BREAKING,
    IDENTICAL,
    REVIEW,
    SAFE,
    classify_changes,
)


def _report(*components):
    changes = [Change(c, MODIFIED, "a", "b") for c in components]
    return classify_changes(changes)


def test_identical_continues():
    gate = ContinuationGate()
    d = gate.decide(classify_changes([]))
    assert d.allowed and d.overall == IDENTICAL


def test_safe_continues():
    gate = ContinuationGate()
    d = gate.decide(_report("operational.log_dir"))
    assert d.allowed and d.overall == SAFE


def test_review_denied_without_flag():
    gate = ContinuationGate(accept_review=False)
    d = gate.decide(_report("sampling.temperature"))
    assert not d.allowed and d.verdict == "accept_required"


def test_review_allowed_with_flag():
    gate = ContinuationGate(accept_review=True)
    d = gate.decide(_report("sampling.temperature"))
    assert d.allowed and d.overall == REVIEW


def test_breaking_blocked_even_with_flag():
    # --accept-review must NOT bypass BREAKING
    gate = ContinuationGate(accept_review=True)
    d = gate.decide(_report("model.hash"))
    assert not d.allowed and d.verdict == "blocked"


def test_mixed_change_max_severity():
    d = ContinuationGate(accept_review=True).decide(
        _report("operational.log_dir", "sampling.temperature", "model.hash")
    )
    assert not d.allowed and d.overall == BREAKING
