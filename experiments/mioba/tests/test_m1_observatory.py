"""M1 §11-§19: the Observatory shows the population, not the machines.

The test that matters most here is §18. In M0 the GUI displayed
"平均発火率 0.50 Hz" while fitness had seen 9.5 Hz for the same
evaluation, because the two numbers came from different computations.
This file pins them to one source and one row.
"""
from __future__ import annotations

import copy
import json

import pytest

from experiments.mioba.evolution import fitness as F
from experiments.mioba.gui import observatory as OBS
from experiments.mioba.storage import models as M

from .conftest import run_worker_once


@pytest.fixture
def seeded(client, service):
    """A run with a few evaluated individuals."""
    for i in range(4):
        if run_worker_once(client, f"w{i}") is None:
            break
    return client, service


# ------------------------------------------------------ canonical rate §18
def test_gui_and_fitness_read_the_same_rate_for_one_evaluation(seeded):
    client, service = seeded
    ev = service.db.list_evaluations(service.experiment_id, limit=1)[0]
    summary = json.loads(ev["summary_json"])

    gui = client.get(f"/api/gui/rate/{ev['evaluation_id']}").json()
    fitness_input = summary["mean_rate_hz"]

    assert gui["mean_rate_hz"] == fitness_input
    assert gui["source"] == "evaluations.summary_json:mean_rate_hz"
    assert gui["evaluation_id"] == ev["evaluation_id"]
    # the M0 placeholder was computed from exactly this number
    target = float(service.config["evaluation"]["target_rate_hz"])
    assert ev["fitness"] == pytest.approx(
        F.fitness_placeholder(summary, target))


def test_canonical_rate_exposes_its_own_inputs(seeded):
    """The displayed rate must be checkable, not merely asserted: the
    pipeline's inputs travel with it, and recomputing from them gives the
    same number."""
    _client, service = seeded
    ev = service.db.list_evaluations(service.experiment_id, limit=1)[0]
    rate = OBS.canonical_rate(ev)
    for key in ("spikes_total", "population_size", "simulated_ms",
                "replicates", "recomputed_mean_rate_hz"):
        assert key in rate and rate[key] is not None, key
    assert rate["recomputed_mean_rate_hz"] == pytest.approx(
        rate["mean_rate_hz"], rel=1e-5)


def test_recomputed_rate_matches_the_reported_one():
    """spike_count / (population x duration) is the pipeline; the stored
    mean_rate_hz must be that number, not a different aggregation."""
    ev = {"evaluation_id": "e1", "completed_replicates": 2,
          "summary": {"mean_rate_hz": 5.0, "spikes_total": 2000,
                      "t_ms": 1000.0, "circuit": {"n_neurons": 200}}}
    rate = OBS.canonical_rate(ev)
    # 2000 spikes / (200 neurons x 1 s x 2 replicates) = 5.0 Hz
    assert rate["recomputed_mean_rate_hz"] == pytest.approx(5.0)
    assert rate["mean_rate_hz"] == pytest.approx(
        rate["recomputed_mean_rate_hz"], rel=1e-6)


# ----------------------------------------------------------- live payload
def test_live_endpoint_shows_the_population_not_the_gpus(seeded):
    client, _service = seeded
    d = client.get("/api/gui/live").json()
    assert d["kind"] == "DERIVED"
    for key in ("population", "lineage_summary", "extinctions",
                "environment", "notable", "innovations", "feed"):
        assert key in d, key
    pop = d["population"]
    assert pop["population"] >= 1
    assert pop["evaluated"] + pop["pending"] == pop["population"]
    ind = pop["individuals"][0]
    for key in ("genome_id", "generation", "selection_score",
                "artificial_neurons", "artificial_organs", "mean_rate_hz"):
        assert key in ind, key
    assert "bins" in pop["neuron_histogram"]
    # the machine view is a different page, and still works
    assert client.get("/api/gui/dashboard").status_code == 200


def test_population_view_counts_organ_classes(seeded):
    client, _service = seeded
    pop = client.get("/api/gui/population").json()
    counts = pop["counts"]
    for key in ("functional_organs", "neutral_organs", "invalid_organs",
                "disabled_organs"):
        assert key in counts and counts[key] >= 0


def test_lineage_marks_living_and_extinct_branches(seeded):
    client, service = seeded
    d = client.get("/api/gui/lineage").json()
    assert d["nodes"] and d["edges"] is not None
    assert d["alive"] >= 1
    assert d["alive"] + d["extinct"] == len(d["nodes"])
    newest = max(n["generation"] for n in d["nodes"])
    for node in d["nodes"]:
        if node["generation"] == newest:
            assert node["alive"], "a newest-generation individual is extinct"
    assert sum(1 for n in d["nodes"] if n["champion"]) <= 1


def test_event_feed_highlights_life_events(seeded):
    client, _service = seeded
    feed = client.get("/api/gui/feed").json()
    types = {e["type"] for e in feed["events"]}
    assert M.EV_GENOME_BORN in types
    # infrastructure noise stays on the infrastructure page
    assert "worker_joined" not in types
    assert feed["last_event_id"] >= 0
    structural = [e for e in feed["events"]
                  if e["type"] == M.EV_STRUCTURAL_MUTATION]
    if structural:
        assert structural[0]["highlight"]


def test_notable_organisms_gate_smallness_on_being_viable(seeded):
    client, _service = seeded
    n = client.get("/api/gui/notable").json()
    assert n["entries"], "no notable organisms surfaced"
    keys = {e["key"] for e in n["entries"]}
    # §14 asks for both ends, not a single leaderboard
    assert "largest_circuit" in keys
    for entry in n["entries"]:
        if entry["key"] in ("smallest_successful", "lowest_memory"):
            assert entry["viable_only"] is True
            if n["viability_threshold"] is not None:
                assert (entry["individual"]["selection_score"]
                        >= n["viability_threshold"])


def test_notable_includes_the_largest_phenotype_jump(seeded):
    _client, service = seeded
    n = OBS.notable_organisms(service.db, service.experiment_id)
    jump = next((e for e in n["entries"]
                 if e["key"] == "largest_phenotype_jump"), None)
    if jump:      # needs at least one evaluated parent-child pair
        assert jump["field"] == "mean_rate_hz"
        assert "parent_rate_hz" in jump["detail"]


def test_individual_view_carries_the_m1_panels(seeded):
    client, service = seeded
    gid = service.db.list_genomes(service.experiment_id, limit=1)[0][
        "genome_id"]
    d = client.get(f"/api/gui/individual/{gid}").json()
    for key in ("individual", "genome", "mutations", "homeostasis",
                "circuit", "region_activity", "resource", "metrics", "rate"):
        assert key in d, key
    assert client.get("/api/gui/individual/nope").status_code == 404


def test_extinction_is_defined_as_absent_from_the_newest_generation(seeded):
    _client, service = seeded
    rows = OBS.extinction_events(service.db, service.experiment_id)
    genomes = service.db.list_genomes(service.experiment_id, limit=10000)
    newest = max(g["generation"] for g in genomes)
    for row in rows:
        assert row["last_generation"] < newest
        assert row["generations_gone"] == newest - row["last_generation"]


def test_environment_panel_reports_the_virtual_environment(tmp_path,
                                                           smoke_config):
    """§2.4-10: the Observatory's environment panel is the *scientific*
    environment. GPU temperature is a different page."""
    pytest.importorskip("torch")
    from fastapi.testclient import TestClient

    from experiments.mioba.coordinator.app import create_app
    from experiments.mioba.coordinator.service import MiobaService

    cfg = copy.deepcopy(smoke_config)
    cfg["evaluation"].update({"backend": "torch", "duration_ms": 30,
                              "replicates": 1})
    cfg["fba"] = {"synthetic": True, "synthetic_neurons": 1200,
                  "synthetic_edges": 6000, "base_seed": 2}
    cfg["environment"] = {
        "enabled": True, "virtual": {"slices": 5,
                                     "sensor_neurons_per_channel": 16},
        "disturbance": {"enabled": True, "count_weights": {"1": 1.0},
                        "kind_weights": {"thermal": 1.0},
                        "severity_weights": {"medium": 1.0}}}
    svc = MiobaService(cfg, tmp_path / "runs")
    try:
        cl = TestClient(create_app(svc))
        run_worker_once(cl, "w1")
        env = cl.get("/api/gui/live").json()["environment"]
        assert env["episodes_sampled"] >= 1
        assert env["disturbed_episodes"] >= 1
        assert "thermal" in env["active_kinds"]
        assert env["max_peak_debt"] is not None
    finally:
        svc.db.close()


# ------------------------------------------------------------------ §19
def test_observatory_never_touches_the_simulation():
    """§19: the GUI reads the DB. It must not import a backend, and no
    coordinator/worker module may import it."""
    import inspect

    src = inspect.getsource(OBS)
    for forbidden in ("fba.registry", "get_backend", "torch",
                      "evaluate_replicates", "run_episode"):
        assert forbidden not in src, forbidden

    from experiments.mioba.workers import worker as W

    assert "observatory" not in inspect.getsource(W)


def test_live_payload_is_bounded(seeded):
    """The page polls every 1-2 s; the payload must not grow without
    limit as the run goes on."""
    client, _service = seeded
    d = client.get("/api/gui/live").json()
    assert len(d["feed"]["events"]) <= 120
    assert len(d["extinctions"]) <= 10
    assert len(d["innovations"]) <= 20
