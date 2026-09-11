"""M1 §10, §20, §21: the measurement tools that set the run's parameters.

M1 §22 forbids fixing the population size, generation count, device
configuration or cheap evaluator in advance. These are the tools whose
output those decisions come from, so what they must not do is quietly
produce a number that looks measured.
"""
from __future__ import annotations

import pytest

from experiments.mioba.genome.structure import analyse
from experiments.mioba.perf.rank_agreement import (candidate_cost,
                                                   parse_candidate,
                                                   search_cheap_evaluator)
from experiments.mioba.perf.sensitivity import (DEFAULT_TARGETS, recommend,
                                                variant)


# ---------------------------------------------------------- §10 sweep
def test_variant_changes_exactly_one_quantity():
    base = variant("vThr", 1.0)
    hotter = variant("vThr", 1.4)
    assert len(hotter.parameter_mutations) == 1
    m = hotter.parameter_mutations[0]
    assert m.path == "vThr" and m.value == 1.4
    # structure is untouched, so the measured move is the parameter's
    assert analyse(base).n_artificial_neurons == \
        analyse(hotter).n_artificial_neurons

    bigger = variant("organ_size", 2.0, base_size=16)
    assert analyse(bigger).n_artificial_neurons == 32
    assert not bigger.parameter_mutations

    heavier = variant("attachment_weight", 3.0)
    assert [a.weight_scale for a in heavier.attachments] == [3.0, 3.0]
    assert not heavier.parameter_mutations


def test_every_swept_target_is_constructible():
    for target in DEFAULT_TARGETS:
        g = variant(target, 1.2)
        assert analyse(g).counts.get("functional", 0) == 1, target


def test_recommendation_lowers_but_never_removes_an_inert_parameter():
    """§10 asks for a lower draw probability, not deletion: removing a
    parameter would be a claim about the model, not a measurement."""
    rows = [
        {"target": "vThr", "sensitivity": 1.0},
        {"target": "tauMem", "sensitivity": 0.5},
        {"target": "wScale", "sensitivity": 0.0},
        {"target": "organ_size", "sensitivity": 0.3},
    ]
    rec = recommend(rows)
    w = rec["parameter_weights"]
    assert w["vThr"] == 1.0
    assert w["tauMem"] == pytest.approx(0.5)
    assert w["wScale"] == 0.2 and w["wScale"] > 0      # lowered, not removed
    assert "wScale" in rec["insensitive_parameters"]
    # an inert parameter gets a wider range instead, since the sweep has
    # just shown that a 5% change is beneath the noise
    assert rec["parameter_scale_by_path"]["wScale"] == [0.6, 1.4]
    assert "vThr" not in rec["parameter_scale_by_path"]
    # structural quantities are reported separately, not weighted as params
    assert rec["structural_sensitivity"] == {"organ_size": 0.3}
    assert "organ_size" not in w


def test_sweep_measures_a_real_phenotype_move():
    pytest.importorskip("torch")
    from experiments.mioba.perf.sensitivity import sweep

    cfg = {"evaluation": {"backend": "torch", "duration_ms": 10,
                          "replicates": 1, "target_rate_hz": 5.0,
                          "environment_id": "synthetic-quiet-v0"},
           "fba": {"synthetic": True, "synthetic_neurons": 600,
                   "synthetic_edges": 3000, "base_seed": 1},
           "env": {"stim_fraction": 0.05, "stim_rate_hz": 400}}
    rep = sweep(cfg, "cpu", targets=("vThr", "organ_size"),
                factors=(0.8, 1.2), base_size=16)
    assert rep["baseline"]["ok"]
    assert {r["target"] for r in rep["rows"]} == {"vThr", "organ_size"}
    for row in rep["rows"]:
        assert len(row["points"]) == 2
        assert all(p["ok"] for p in row["points"])
        assert row["sensitivity"] is not None
    assert "parameter_weights" in rep["recommendation"]


# ------------------------------------------------------ §21 cheap evaluator
def test_candidate_parsing_and_ordering():
    assert parse_candidate("250x2") == {"duration_ms": 250.0,
                                        "replicates": 2}
    assert parse_candidate(" 500 X 4 ") == {"duration_ms": 500.0,
                                            "replicates": 4}
    with pytest.raises(ValueError):
        parse_candidate("250")
    assert candidate_cost({"duration_ms": 250, "replicates": 2}) == 500
    assert (candidate_cost({"duration_ms": 500, "replicates": 2})
            < candidate_cost({"duration_ms": 500, "replicates": 4}))


def test_search_takes_the_cheapest_accepted_candidate(monkeypatch):
    """§21: the *lightest* setting that ranks like gold, not the best one
    — otherwise agreement is bought with compute."""
    from experiments.mioba.perf import rank_agreement as RA

    seen = []

    def fake_compare(genomes, config, device, gold, cheap, score, **kw):
        seen.append(cheap)
        # 250x2 disagrees; 500x2 agrees; 500x4 would also agree
        ok = candidate_cost(cheap) >= 1000
        return {"cheap": dict(cheap), "gold": dict(gold), "accepted": ok,
                "spearman_rho": 0.9 if ok else 0.4,
                "top_k": {"k": 8, "overlap": 8 if ok else 3},
                "rejected_because": [] if ok else ["spearman 0.400 < 0.85"],
                "speedup": 2.0}

    monkeypatch.setattr(RA, "compare_evaluators", fake_compare)
    out = RA.search_cheap_evaluator(
        [], {}, "cpu", {"duration_ms": 500, "replicates": 8},
        [parse_candidate(c) for c in ("500x4", "250x2", "500x2")],
        score=lambda s: 0.0)
    # cheapest first, stop at the first acceptance
    assert seen == [{"duration_ms": 250.0, "replicates": 2},
                    {"duration_ms": 500.0, "replicates": 2}]
    assert out["chosen"] == {"duration_ms": 500.0, "replicates": 2}
    assert len(out["attempts"]) == 2


def test_search_reports_failure_rather_than_settling(monkeypatch):
    from experiments.mioba.perf import rank_agreement as RA

    monkeypatch.setattr(RA, "compare_evaluators",
                        lambda *a, **k: {"cheap": dict(a[4]), "accepted": False,
                                         "spearman_rho": 0.1,
                                         "top_k": {"k": 8, "overlap": 1},
                                         "rejected_because": ["rho too low"],
                                         "speedup": 3.0})
    out = RA.search_cheap_evaluator(
        [], {}, "cpu", {"duration_ms": 500, "replicates": 8},
        [parse_candidate("250x2")], score=lambda s: 0.0)
    assert out["chosen"] is None
    assert "gold evaluator" in out["note"]


def test_a_candidate_at_or_above_gold_cost_is_skipped():
    from experiments.mioba.perf import rank_agreement as RA

    out = RA.search_cheap_evaluator(
        [], {}, "cpu", {"duration_ms": 500, "replicates": 8},
        [parse_candidate("500x8"), parse_candidate("500x16")],
        score=lambda s: 0.0)
    assert out["chosen"] is None
    assert all(a.get("skipped") for a in out["attempts"])


# ------------------------------------------------------- §20 device bench
def test_device_benchmark_compares_on_identical_conditions():
    pytest.importorskip("torch")
    from experiments.mioba.perf.evalbench import device_benchmark

    cfg = {"evaluation": {"backend": "torch", "duration_ms": 10,
                          "replicates": 1,
                          "environment_id": "synthetic-quiet-v0"},
           "fba": {"synthetic": True, "synthetic_neurons": 400,
                   "synthetic_edges": 2000, "base_seed": 4},
           "env": {"stim_fraction": 0.05, "stim_rate_hz": 200}}
    rep = device_benchmark(cfg, ("cpu",), evaluations=2,
                           slot_candidates=(1, 2))
    row = rep["rows"][0]
    assert row["ok"] and row["device"] == "cpu"
    assert row["evaluations_per_minute"] > 0
    assert row["p50_latency_s"] is not None
    assert row["p95_latency_s"] >= row["p50_latency_s"]
    assert row["active_edge_ratio"] is not None
    assert rep["recommended_device"] == "cpu"
    assert rep["devices_agree_on_results"] is True


def test_device_benchmark_flags_devices_that_disagree(monkeypatch):
    """If two devices produce different firing rates for the same genome
    and seed, one of them is not running the experiment that was asked
    for — and that must stop the recommendation, not colour it."""
    from experiments.mioba.perf import evalbench as EB

    rates = iter([[1.0, 2.0], [9.0, 9.0]])

    def fake_sweep(genomes, config, device, **kw):
        vals = next(rates)
        return {"rows": [{"slots": 1, "succeeded": 2, "failed": 0, "oom": 0,
                          "wall_s": 1.0,
                          "successful_evaluations_per_minute": 60.0,
                          "p50_latency_s": 0.5, "p95_latency_s": 0.6,
                          "peak_vram_bytes": None, "active_edge_ratio": 0.1,
                          "active_edges_per_step": 3.0,
                          "per_evaluation": [{"mean_rate_hz": v}
                                             for v in vals]}],
                "selected_slots": 1, "selected_evaluations_per_minute": 60.0}

    monkeypatch.setattr(EB, "slot_sweep", fake_sweep)
    rep = EB.device_benchmark({}, ("cuda:0", "cuda:1"), evaluations=2)
    assert rep["devices_agree_on_results"] is False
    assert "investigate" in rep["note"]


def test_device_benchmark_records_a_failed_device_instead_of_raising(
        monkeypatch):
    from experiments.mioba.perf import evalbench as EB

    def boom(*a, **k):
        raise RuntimeError("no CUDA device")

    monkeypatch.setattr(EB, "slot_sweep", boom)
    rep = EB.device_benchmark({}, ("cuda:9",), evaluations=1)
    assert rep["rows"][0]["ok"] is False
    assert "no CUDA device" in rep["rows"][0]["error"]
    assert rep["recommended_device"] is None
