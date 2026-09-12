"""Unit tests for the metric implementations."""

from __future__ import annotations

import math

from experiments.fi0.analysis import agreement, cache, drift


def test_pairwise_agreement_all_same():
    assert agreement.pairwise_agreement({"a": "YES", "b": "YES"}) == 1.0


def test_pairwise_agreement_half():
    # 4 variants: 3 YES, 1 NO -> agree pairs = C(3,2)=3 of 6
    by = {"a": "YES", "b": "YES", "c": "YES", "d": "NO"}
    assert agreement.pairwise_agreement(by) == 0.5


def test_exact_agreement():
    assert agreement.exact_agreement(["A", "A"]) == 1.0
    assert agreement.exact_agreement(["A", "B"]) == 0.0
    assert agreement.exact_agreement([]) == 1.0


def test_majority_choice_tie_is_lexicographic():
    assert agreement.majority_choice(["B", "A"]) == "A"
    assert agreement.majority_choice(["B", "B", "A"]) == "B"


def test_js_divergence_bounds():
    p = [1.0, 0.0]
    q = [0.0, 1.0]
    assert drift.js_divergence(p, p) == 0.0
    assert abs(drift.js_divergence(p, q) - 1.0) < 1e-9  # max is 1 bit
    assert 0.0 < drift.js_divergence([0.5, 0.5], p) < 1.0


def test_kl_divergence():
    p = [0.5, 0.5]
    q = [0.9, 0.1]
    expected = 0.5 * math.log(0.5 / 0.9) + 0.5 * math.log(0.5 / 0.1)
    assert abs(drift.kl_divergence(p, q) - expected) < 1e-9
    assert drift.kl_divergence(p, p) == 0.0
    # disjoint support -> large (epsilon-smoothed), not infinite
    assert drift.kl_divergence(p, [1.0, 0.0]) > 10.0


def test_distribution():
    d = drift.distribution(["A", "A", "B"], ["A", "B", "C"])
    assert d == [2 / 3, 1 / 3, 0.0]


def test_per_task_drift_zero_for_stable():
    rows = [
        {"variant": "v1", "choice": "YES"},
        {"variant": "v2", "choice": "YES"},
    ]
    out = drift.per_task_drift(rows, ["YES", "NO"])
    assert out["js_mean"] == 0.0
    assert out["n_distinct_choices"] == 1
    assert out["pooled_entropy_bits"] == 0.0


def test_per_task_drift_detects_split():
    rows = [
        {"variant": "v1", "choice": "YES"},
        {"variant": "v2", "choice": "NO"},
    ]
    out = drift.per_task_drift(rows, ["YES", "NO"])
    assert out["js_max"] > 0
    assert out["pooled_entropy_bits"] == 1.0


def test_lcp_ratio():
    assert cache.lcp_ratio("abcdef", "abcXYZ") == 3 / 6
    assert cache.lcp_ratio("same", "same") == 1.0
    assert cache.lcp_ratio("", "") == 1.0
    assert cache.lcp_ratio("abc", "xyz") == 0.0


def test_cacheability_groups():
    prompts = {
        "plain|cache_max|tight|meta:suffix": "[SYSTEM]\ncommon prefix AAAA",
        "plain|sys_mem_user|tight|meta:suffix": "[SYSTEM]\ncommon prefix BBBB",
        "json|cache_max|tight|meta:suffix": '{"SYSTEM": "x"}',
    }
    out = cache.cacheability_by_group(prompts)
    assert 0.0 <= out["overall_mean_lcp_ratio"] <= 1.0
    assert "plain" in out["within_format_mean_lcp_ratio"]
    # plain variants share a long prefix; json shares none with plain
    assert out["within_format_mean_lcp_ratio"]["plain"] > 0.3


def test_consistency():
    rows = [
        {"choice": "YES", "marker_present": True},
        {"choice": "NO", "marker_present": False},
    ]
    out = drift.consistency(rows, expected="YES", marker="K7")
    assert out["semantic"] == 0.5
    assert out["persona"] == 0.5
