"""M1 §5-§9: disturbances, homeostasis, and separated selection metrics.

The properties pinned here are the ones that decide whether M1 is an
experiment or a simulation of one: crises must be reproducible but not
predictable, they must actually cost something, the host machine must
stay out of the science, and the efficiency term must not be winnable by
doing nothing.
"""
from __future__ import annotations

import copy

import pytest

from experiments.mioba.evolution import fitness as F
from experiments.mioba.fba.seeds import disturbance_seed
from experiments.mioba.mie.disturbance import (KINDS, merged_config,
                                               sample_schedule)
from experiments.mioba.mie.episode import analyse_trace, run_episode
from experiments.mioba.mie.virtual_env import (CHANNELS, NOMINAL,
                                               PhysicalSensorsNotTraced,
                                               VirtualEnvironment)


def _cfg(**over):
    cfg = {"environment": {"disturbance": {"enabled": True},
                           "virtual": {"slices": 10}},
           "env": {"stim_fraction": 0.05, "stim_rate_hz": 50},
           "evaluation": {"target_rate_hz": 5.0}}
    for key, value in over.items():
        cfg.setdefault(key, {})
        if isinstance(value, dict):
            cfg[key].update(value)
        else:
            cfg[key] = value
    return cfg


# ------------------------------------------------------------- schedule
def test_disturbances_are_reproducible_from_the_seed():
    a = sample_schedule(1234, _cfg())
    b = sample_schedule(1234, _cfg())
    assert a.to_dict() == b.to_dict()
    assert sample_schedule(1235, _cfg()).to_dict() != a.to_dict()


def test_onsets_are_spread_not_fixed():
    """§7.1: an organism must not be able to adapt to the clock."""
    starts = set()
    for seed in range(300):
        for ev in sample_schedule(seed, _cfg()).events:
            starts.add(round(ev.start_frac, 3))
            assert 0.2 <= ev.start_frac <= 0.8
            assert ev.end_frac > ev.start_frac
    assert len(starts) > 50, "onsets are clustered on too few values"


def test_episodes_mix_none_single_and_multiple():
    counts = {}
    for seed in range(500):
        n = len(sample_schedule(seed, _cfg()).events)
        counts[n] = counts.get(n, 0) + 1
    assert counts.get(0, 0) > 0, "no undisturbed control episodes"
    assert counts.get(1, 0) > 0
    assert sum(v for k, v in counts.items() if k >= 2) > 0


def test_every_required_disturbance_kind_can_occur():
    seen = set()
    for seed in range(800):
        seen |= {e.kind for e in sample_schedule(seed, _cfg()).events}
    assert seen == set(KINDS)


def test_compound_disturbances_overlap():
    found = None
    for seed in range(2000):
        for ev in sample_schedule(seed, _cfg()).events:
            if ev.compound_with:
                found = (seed, ev)
                break
        if found:
            break
    assert found, "no compound disturbance was ever sampled"
    seed, ev = found
    sched = sample_schedule(seed, _cfg())
    overlapping = [o for o in sched.events
                   if o is not ev and o.start_frac <= ev.start_frac < o.end_frac]
    assert overlapping


def test_disturbances_can_be_switched_off():
    cfg = _cfg()
    cfg["environment"]["disturbance"]["enabled"] = False
    assert sample_schedule(7, cfg).events == []


# ---------------------------------------------------------- environment
def test_channels_rest_at_nominal_without_a_disturbance():
    cfg = _cfg()
    cfg["environment"]["disturbance"]["enabled"] = False
    env = VirtualEnvironment(seed=1, config=cfg, n_base=2000,
                             task_neurons=(0, 100))
    st = env.advance(0.5)
    assert set(st.channels) == set(CHANNELS)
    assert all(abs(v - NOMINAL) < 1e-9 for v in st.channels.values())
    assert st.debt == 0.0 and st.violation == 0.0


def test_a_disturbance_moves_its_channel_and_costs_something():
    cfg = _cfg()
    cfg["environment"]["disturbance"] = {
        "enabled": True, "count_weights": {"1": 1.0},
        "kind_weights": {"thermal": 1.0},
        "severity_weights": {"severe": 1.0}}
    env = VirtualEnvironment(seed=3, config=cfg, n_base=2000,
                             task_neurons=(0, 100))
    ev = env.schedule.events[0]
    mid = (ev.start_frac + ev.end_frac) / 2
    calm = env.advance(max(0.0, ev.start_frac - 0.05))
    hot = env.advance(mid)
    assert hot.channels["virtual_temperature"] > calm.channels[
        "virtual_temperature"]
    assert hot.violation > 0 and hot.debt > 0
    cons = env.consequences()
    assert cons["sensor_noise"] > 0 or cons["compute_fraction"] < 1.0


def test_debt_decays_when_the_crisis_ends():
    """§6: deviation that is survived costs less than deviation left to
    run — debt must come down again."""
    cfg = _cfg()
    cfg["environment"]["disturbance"] = {
        "enabled": True, "count_weights": {"1": 1.0},
        "kind_weights": {"thermal": 1.0},
        "severity_weights": {"severe": 1.0},
        "onset_window": [0.2, 0.25], "duration_frac": [0.1, 0.12]}
    env = VirtualEnvironment(seed=5, config=cfg, n_base=2000,
                             task_neurons=(0, 100))
    debts = [env.advance(i / 40).debt for i in range(40)]
    peak = max(debts)
    assert peak > 0
    assert debts[-1] < peak, "debt never decayed after the crisis"


def test_runaway_debt_terminates_the_episode():
    cfg = _cfg()
    cfg["environment"]["virtual"] = {"slices": 10, "debt_terminate": 0.2}
    cfg["environment"]["disturbance"] = {
        "enabled": True, "count_weights": {"1": 1.0},
        "kind_weights": {"thermal": 1.0},
        "severity_weights": {"severe": 1.0},
        "onset_window": [0.2, 0.21], "duration_frac": [0.6, 0.7]}
    env = VirtualEnvironment(seed=9, config=cfg, n_base=2000,
                             task_neurons=(0, 100))
    states = [env.advance(i / 20) for i in range(20)]
    assert any(s.terminated for s in states)
    assert env.consequences()["terminate"] is True


def test_sensor_channels_reach_the_network_as_input():
    """§5.1: the organism can see its own state — nothing tells it what
    the state means."""
    env = VirtualEnvironment(seed=2, config=_cfg(), n_base=2000,
                             task_neurons=(0, 100))
    env.advance(0.0)
    drive = env.drive(task_rate_hz=50.0)
    keys = set(drive["rates_hz"])
    for name, (lo, hi) in env.sensor_ranges.items():
        assert f"slice:{lo}-{hi}" in keys, name
    assert all(lo < hi for lo, hi in env.sensor_ranges.values())
    # sensor blocks do not overlap the task stimulus
    assert min(lo for lo, _ in env.sensor_ranges.values()) >= 100


def test_dropout_silences_channels_rather_than_zeroing_them():
    cfg = _cfg()
    cfg["environment"]["disturbance"] = {
        "enabled": True, "count_weights": {"1": 1.0},
        "kind_weights": {"sensor_dropout": 1.0},
        "severity_weights": {"severe": 1.0},
        "onset_window": [0.2, 0.21], "duration_frac": [0.5, 0.6]}
    env = VirtualEnvironment(seed=11, config=cfg, n_base=2000,
                             task_neurons=(0, 100))
    env.advance(0.4)
    assert env.consequences()["sensor_dropout"] > 0
    silenced = set()
    for _ in range(20):
        silenced |= set(env.drive(50.0)["silence"])
    assert silenced, "dropout never silenced a channel"


def test_latency_makes_the_network_act_on_a_stale_picture():
    cfg = _cfg()
    cfg["environment"]["disturbance"] = {"enabled": False}
    env = VirtualEnvironment(seed=13, config=cfg, n_base=2000,
                             task_neurons=(0, 100))
    env.advance(0.0)
    first = env.drive(10.0)
    # force a latency by hand: the consequence, not the sampler, is what
    # is under test here
    env.state.channels["virtual_latency"] = 1.0
    delayed = [env.drive(999.0) for _ in range(2)]
    assert delayed[0]["rates_hz"] == first["rates_hz"]


def test_physical_sensors_require_a_trace():
    """§2.4-10: real machine state is not reproducible by itself. Using
    it as a scientific input without recording it is refused."""
    cfg = _cfg()
    cfg["environment"]["virtual"] = {"slices": 5, "use_physical_sensors": True}
    with pytest.raises(PhysicalSensorsNotTraced):
        VirtualEnvironment(seed=1, config=cfg, n_base=100,
                           task_neurons=(0, 10))
    cfg["environment"]["virtual"]["physical_sensor_trace"] = "traces/phys.json"
    VirtualEnvironment(seed=1, config=cfg, n_base=100, task_neurons=(0, 10))


def test_lanes_meet_different_crises_but_replay_the_same_ones():
    a = sample_schedule(disturbance_seed(999, 0), _cfg())
    b = sample_schedule(disturbance_seed(999, 1), _cfg())
    again = sample_schedule(disturbance_seed(999, 0), _cfg())
    assert a.to_dict() == again.to_dict()
    assert a.to_dict() != b.to_dict()


# -------------------------------------------------------------- episode
def test_episode_produces_a_scored_trace_under_disturbance():
    pytest.importorskip("torch")
    from experiments.mioba.development.phenotype import develop
    from experiments.mioba.fba.torch_backend import TorchBackend
    from experiments.mioba.genome.schema import fba0_genome

    cfg = _cfg()
    cfg["environment"]["disturbance"] = {
        "enabled": True, "count_weights": {"1": 1.0},
        "kind_weights": {"sensor_noise": 1.0},
        "severity_weights": {"medium": 1.0},
        "onset_window": [0.3, 0.35], "duration_frac": [0.2, 0.25]}
    n = 1200
    b = TorchBackend(synthetic=True, synthetic_neurons=n, synthetic_edges=6000)
    b.initialize(develop(fba0_genome(), base_neurons=n), batch_size=2, seed=1,
                 device="cpu")
    envs = [VirtualEnvironment(seed=disturbance_seed(1, i), config=cfg,
                               n_base=n, task_neurons=(0, 60))
            for i in range(2)]
    out = run_episode(b, envs, duration_ms=40.0, target_rate_hz=5.0,
                      task_rate_hz=200.0)
    assert out["slices"] == 10
    for lane in out["per_lane"]:
        assert len(lane["trace"]) == 10
        assert lane["task_score_all"] is not None
        assert any(s["disturbed"] for s in lane["trace"])
        assert lane["disturbances"], "no disturbance window analysed"
        w = lane["disturbances"][0]
        assert set(w) >= {"performance_before", "performance_during",
                          "performance_after", "recovery_slices",
                          "recovered"}


def test_missing_slices_after_termination_score_as_failures():
    """Ending an episode early must never be a way to win."""
    class _Env:
        n_slices = 4

        class state:
            terminated = True
            debt = 9.9

        class schedule:
            events = []

    trace = [{"slice": 0, "t_frac": 0.0, "rate_hz": 5.0, "score": 0.0,
              "disturbed": False, "kinds": [], "debt": 0.0,
              "terminated": False}]
    trace.append({"slice": 1, "t_frac": 0.25, "rate_hz": None, "score": -5.0,
                  "disturbed": True, "kinds": ["terminated"], "debt": 9.9,
                  "terminated": True, "missing": True})
    rep = analyse_trace(trace, _Env(), 5.0)
    assert rep["missing_slices"] == 1
    assert rep["task_score_all"] < 0


# -------------------------------------------------------------- fitness
def _summary(rate=5.0, neurons=64, edges=2000, compute=100.0):
    return {
        "mean_rate_hz": rate, "rate_std_hz": 0.1, "active_fraction": 0.5,
        "spikes_total": 1000,
        "resource": {"marginal_memory_cost_bytes": neurons * 100,
                     "marginal_compute_cost": compute,
                     "artificial_neuron_count": neurons,
                     "artificial_edge_count": edges,
                     "genome_bytes": 900},
        "circuit": {"n_artificial_neurons": neurons},
    }


def test_components_are_kept_apart_and_selection_never_overwrites_raw():
    m = F.compute_metrics(_summary(), {"n_artificial_neurons": 64,
                                       "n_functional_neurons": 64},
                          None, 5.0)
    for key in ("task_score", "homeostasis_score",
                "disturbance_recovery_score", "resource_efficiency_score",
                "structural_functionality", "novelty", "raw_firing_metrics"):
        assert key in m
    assert m["raw_firing_metrics"]["mean_rate_hz"] == 5.0
    score = F.selection_score(m, None)
    assert score is not None
    # raw values untouched by scoring
    assert m["raw_firing_metrics"]["mean_rate_hz"] == 5.0


def test_doing_nothing_earns_no_efficiency_bonus():
    """§9: the smallest circuit must not win by being inert."""
    inert = F.compute_metrics(_summary(rate=0.0, neurons=0, edges=0,
                                       compute=0.0), None, None, 5.0)
    assert inert["task_quality"] == 0.0
    assert inert["resource_efficiency_score"] == 0.0
    assert inert["resource_efficiency_gated"] is True

    working = F.compute_metrics(_summary(rate=5.0), None, None, 5.0)
    assert working["resource_efficiency_gated"] is False
    assert working["resource_efficiency_score"] > 0
    assert (F.selection_score(working, None)
            > F.selection_score(inert, None))


def test_cheaper_wins_only_among_equally_capable_individuals():
    big = F.compute_metrics(_summary(rate=5.0, neurons=512, compute=4000.0),
                            None, None, 5.0)
    small = F.compute_metrics(_summary(rate=5.0, neurons=16, compute=50.0),
                              None, None, 5.0)
    assert small["resource_efficiency_score"] > big["resource_efficiency_score"]
    # ... but a cheap incompetent still loses to a capable expensive one
    cheap_bad = F.compute_metrics(_summary(rate=0.2, neurons=16, compute=50.0),
                                  None, None, 5.0)
    assert (F.selection_score(big, None)
            > F.selection_score(cheap_bad, None))


def test_structural_functionality_is_a_share_not_a_count():
    """Growing neurons earns nothing; growing *live* ones raises the
    share of tissue that can matter."""
    all_live = F.structural_functionality({"n_artificial_neurons": 100,
                                           "n_functional_neurons": 100})
    half_dead = F.structural_functionality({"n_artificial_neurons": 200,
                                            "n_functional_neurons": 100})
    assert all_live == 1.0 and half_dead == 0.5
    assert F.structural_functionality({"n_artificial_neurons": 0,
                                       "n_functional_neurons": 0}) is None


def test_an_undisturbed_episode_is_not_scored_as_a_failed_recovery():
    """A control episode must be judged on what it was tested on."""
    m = F.compute_metrics(_summary(), None, None, 5.0)
    assert m["disturbance_recovery_score"] is None
    assert m["selection_components"]["disturbance_recovery_score"] is None
    assert F.selection_score(m, None) is not None


def test_weights_can_be_rederived_without_resimulating():
    m = F.compute_metrics(_summary(rate=5.0, neurons=256, compute=3000.0),
                          {"n_artificial_neurons": 256,
                           "n_functional_neurons": 64}, None, 5.0)
    lean = {"fitness": {"weights": {"task_score": 1.0,
                                    "resource_efficiency_score": 4.0}}}
    task_only = {"fitness": {"weights": {"task_score": 1.0}}}
    assert (F.selection_score(m, lean) != F.selection_score(m, task_only))


def test_novelty_rewards_being_unlike_the_archive():
    base = _summary(rate=5.0)
    archive = [F.descriptor(_summary(rate=5.0), None) for _ in range(5)]
    same = F.compute_metrics(base, None, None, 5.0, archive=archive)
    odd = F.compute_metrics(_summary(rate=40.0), None, None, 5.0,
                            archive=archive)
    assert odd["novelty"] > same["novelty"]


def test_selection_score_is_stored_alongside_the_m0_placeholder(client,
                                                                service):
    from .conftest import run_worker_once

    run_worker_once(client, "w1")
    ev = service.db.list_evaluations(service.experiment_id, limit=1)[0]
    assert ev["fitness"] is not None            # M0-comparable, unchanged
    assert ev["selection_score"] is not None    # what M1 selects on
    import json
    metrics = json.loads(ev["metrics_json"])
    assert "raw_firing_metrics" in metrics and "descriptor" in metrics


def test_end_to_end_evaluation_under_disturbance(tmp_path, smoke_config):
    """The whole path: config -> worker -> episode -> metrics -> DB."""
    pytest.importorskip("torch")
    from fastapi.testclient import TestClient

    from experiments.mioba.coordinator.app import create_app
    from experiments.mioba.coordinator.service import MiobaService
    from .conftest import run_worker_once

    cfg = copy.deepcopy(smoke_config)
    cfg["evaluation"].update({"backend": "torch", "duration_ms": 40,
                              "replicates": 2})
    cfg["fba"] = {"synthetic": True, "synthetic_neurons": 1500,
                  "synthetic_edges": 8000, "base_seed": 3}
    cfg["env"] = {"stim_fraction": 0.04, "stim_rate_hz": 300}
    cfg["environment"] = {
        "enabled": True,
        "virtual": {"slices": 6, "sensor_neurons_per_channel": 16},
        "disturbance": {"enabled": True, "count_weights": {"1": 1.0},
                        "kind_weights": {"thermal": 1.0},
                        "severity_weights": {"medium": 1.0}}}
    svc = MiobaService(cfg, tmp_path / "runs")
    try:
        cl = TestClient(create_app(svc))
        body = run_worker_once(cl, "w1")
        assert body["status"] == "SUCCEEDED", body.get("error")
        ev = svc.db.list_evaluations(svc.experiment_id, limit=1)[0]
        import json
        summary = json.loads(ev["summary_json"])
        episode = summary["episode"]
        assert episode["slices"] == 6
        assert len(episode["per_lane"]) == 2
        assert episode["environments"][0]["schedule"]["n_events"] == 1
        metrics = json.loads(ev["metrics_json"])
        assert metrics["has_disturbance"] is True
        assert metrics["disturbance_recovery_score"] is not None
        assert ev["selection_score"] is not None
    finally:
        svc.db.close()
