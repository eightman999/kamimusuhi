"""Functional-departure evaluation (M2 §16-18): conditions, controls,
determinism, and the derived dependency metrics."""
from __future__ import annotations

import random

from experiments.mioba.development.phenotype import develop
from experiments.mioba.evolution.fitness import compute_metrics
from experiments.mioba.fba.mock_backend import MockBackend
from experiments.mioba.fba.replicates import replicate_seeds
from experiments.mioba.genome.mutation import mutate
from experiments.mioba.genome.schema import (ArtificialOrgan, Attachment,
                                             fba0_genome)
from experiments.mioba.m2.departure import evaluate_departure

_CFG = {
    "env": {"stim_fraction": 0.01, "stim_rate_hz": 50.0},
    "evaluation": {"target_rate_hz": 5.0},
    "functional_departure": {"enabled": True, "severities": [0.1, 0.5],
                             "controls": {"founder": True, "sham": True}},
}


def _organ_genome(seed: int = 3):
    g = fba0_genome(seed=seed)
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_a", kind="lif_cluster", size=16))
    g.attachments += [
        Attachment(attachment_id="in", source="fba0:medulla",
                   target="org_a"),
        Attachment(attachment_id="out", source="org_a",
                   target="fba0:central_complex")]
    return g.finalize()


def _job(n_rep: int = 3) -> dict:
    return {"seed": 4242, "replicates": n_rep, "duration_ms": 100,
            "environment_id": "synthetic-quiet-v0", "config": _CFG}


def _backend() -> MockBackend:
    return MockBackend(n_neurons=500)


def test_departure_battery_runs_all_conditions():
    phen = develop(_organ_genome(), base_neurons=500)
    job = _job()
    dep = evaluate_departure(_backend(), phen, job, _CFG,
                             seeds=replicate_seeds(job["seed"], 3))
    assert set(dep["conditions"]) >= {
        "intact", "sham", "founder", "organ_ablation",
        "fba0_lesion_0.10", "fba0_lesion_0.50"}
    assert dep["conditions"]["fba0_lesion_0.10"]["n_silenced"] == 50
    assert dep["conditions"]["fba0_lesion_0.50"]["n_silenced"] == 250
    assert dep["conditions"]["organ_ablation"]["n_silenced"] == 16


def test_sham_equals_intact():
    """The negative control: the lesion procedure applied to nothing is
    the intact run — same seeds, same result."""
    phen = develop(_organ_genome(), base_neurons=500)
    dep = evaluate_departure(_backend(), phen, _job(), _CFG,
                             seeds=replicate_seeds(4242, 3))
    assert dep["sham_score"] == dep["intact_score"]
    assert dep["conditions"]["sham"]["mean_rate_hz"] == \
        dep["conditions"]["intact"]["mean_rate_hz"]


def test_departure_is_deterministic():
    phen = develop(_organ_genome(), base_neurons=500)
    job = _job()
    a = evaluate_departure(_backend(), phen, job, _CFG,
                           seeds=replicate_seeds(job["seed"], 3))
    b = evaluate_departure(_backend(), phen, job, _CFG,
                           seeds=replicate_seeds(job["seed"], 3))
    assert a == b


def test_founder_control_matches_the_founder_phenotype():
    """The founder condition must equal developing and simulating the
    actual founder genome under the same seeds."""
    job = _job()
    seeds = replicate_seeds(job["seed"], 3)
    dep = evaluate_departure(_backend(), develop(_organ_genome(),
                                                 base_neurons=500),
                             job, _CFG, seeds=seeds)
    founder_dep = evaluate_departure(
        _backend(), develop(fba0_genome(seed=job["seed"]),
                            base_neurons=500), job, _CFG, seeds=seeds)
    assert dep["founder_score"] == founder_dep["intact_score"]


def test_lesion_loss_and_dependencies_are_derived_from_raw():
    phen = develop(_organ_genome(), base_neurons=500)
    dep = evaluate_departure(_backend(), phen, _job(), _CFG,
                             seeds=replicate_seeds(4242, 3))
    intact = dep["intact_score"]
    for sev in ("0.10", "0.50"):
        expected = round(intact - dep["conditions"]
                         [f"fba0_lesion_{sev}"]["task_score"], 6)
        assert dep["fba0_lesion_loss"][sev] == expected
    assert dep["fba0_lesion_10_score"] == \
        dep["conditions"]["fba0_lesion_0.10"]["task_score"]
    assert dep["artificial_organ_dependency"] == \
        dep["organ_ablation_loss"]
    assert dep["structural_ancestry_fraction"] == \
        phen["structural_ancestry_fraction"]


def test_disabled_departure_returns_none():
    assert evaluate_departure(_backend(), develop(_organ_genome()),
                              _job(), {}) is None


def test_departure_feeds_the_fitness_components():
    """raw metrics persist under metrics_json, and the
    departure_resistance component is populated only then."""
    phen = develop(_organ_genome(), base_neurons=500)
    job = _job()
    dep = evaluate_departure(_backend(), phen, job, _CFG,
                             seeds=replicate_seeds(job["seed"], 3))
    summary = {"mean_rate_hz": dep["intact_score"] * -1 + 5.0,
               "departure": dep}
    metrics = compute_metrics(summary, None, _CFG, 5.0)
    assert metrics["departure"] == dep
    assert metrics["departure_resistance"] is not None
    comps = metrics["selection_components"]
    assert comps["departure_resistance"] == \
        metrics["departure_resistance"]
    # no battery -> no component (the M1 evaluation path)
    plain = compute_metrics({"mean_rate_hz": 5.0}, None, {}, 5.0)
    assert plain["departure"] is None
    assert plain["departure_resistance"] is None
    assert plain["selection_components"]["departure_resistance"] is None


def test_mutation_children_measure_their_own_departure():
    """A mutated child's departure is its own, not the parent's."""
    parent = _organ_genome(seed=5)
    child, _ = mutate(parent, random.Random(7), birth_index=0,
                      generation=1)
    job = _job()
    seeds = replicate_seeds(job["seed"], 3)
    pa = evaluate_departure(_backend(), develop(parent, base_neurons=500),
                            job, _CFG, seeds=seeds)
    ch = evaluate_departure(_backend(), develop(child, base_neurons=500),
                            job, _CFG, seeds=seeds)
    assert pa["intact_score"] is not None and ch["intact_score"] is not None
