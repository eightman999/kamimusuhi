"""M1.5 §8 deterministic runaway guard: configured limits terminate the
simulation deterministically, report an explicit reason, and stay
untripped (and identical) when not configured."""
import pytest

torch = pytest.importorskip("torch")

from experiments.mioba.development.phenotype import develop  # noqa: E402
from experiments.mioba.evolution import fitness as F  # noqa: E402
from experiments.mioba.fba.torch_backend import TorchBackend  # noqa: E402
from experiments.mioba.genome.schema import fba0_genome  # noqa: E402
from experiments.mioba.workers.worker import evaluate_replicates  # noqa: E402

N = 200


def _backend(seed=7):
    b = TorchBackend(synthetic=True, synthetic_neurons=N, connectivity=0.05)
    b.initialize(develop(fba0_genome(), base_neurons=N), batch_size=2,
                 seed=seed, device="cpu")
    return b


def test_edge_event_guard_trips_and_stays_tripped():
    b = _backend()
    b.force_spikes([0])          # neuron 0 fires every step -> edge events
    b.set_run_limits({"max_propagated_edge_events": 1})
    out = b.run(10.0)
    assert out["guard"]["reason"] == "max_propagated_edge_events"
    assert out["guard"]["deterministic"] is True
    assert out["simulated_ms"] < 10.0     # terminated early, not silently
    t_at_trip = out["guard"]["at_t_ms"]
    # a tripped guard persists: later run() calls simulate nothing
    out2 = b.run(10.0)
    assert out2["simulated_ms"] == 0.0
    assert out2["guard"]["at_t_ms"] == t_at_trip


def test_guard_is_deterministic_across_instances():
    trip_t = []
    for _ in range(2):
        b = _backend()
        b.force_spikes([0])
        b.set_run_limits({"max_propagated_edge_events": 500})
        out = b.run(50.0)
        assert out["guard"]["tripped"]
        trip_t.append(out["guard"]["at_t_ms"])
    assert trip_t[0] == trip_t[1]


def test_no_limits_means_no_guard_and_identical_run():
    a, b = _backend(11), _backend(11)
    a.set_run_limits({})               # configured-but-empty = M1 semantics
    ra, rb = a.run(5.0), b.run(5.0)
    assert "guard" not in ra and "guard" not in rb
    assert a.spike_counts.eq(b.spike_counts).all()
    assert (a.get_state_summary()["guard"]
            == b.get_state_summary()["guard"] is None)


def test_spike_event_guard_also_works():
    b = _backend()
    b.force_spikes([0, 1, 2])
    b.set_run_limits({"max_spike_events": 3})
    out = b.run(5.0)
    assert out["guard"]["reason"] == "max_spike_events"


def test_evaluate_replicates_surfaces_guard_from_config():
    b = _backend()
    b.force_spikes([0])
    job = {"seed": 5, "replicates": 2, "duration_ms": 30.0,
           "environment_id": "synthetic-quiet-v0",
           "config": {"evaluation": {
               "target_rate_hz": 5.0,
               "guard": {"max_propagated_edge_events": 10}}}}
    rep = evaluate_replicates(b, develop(fba0_genome(), base_neurons=N),
                              job, "cpu", execution_batch=2)
    assert rep["summary"]["guard"]["reason"] == \
        "max_propagated_edge_events"


def test_guard_trip_is_a_reproducible_selection_penalty():
    summary = {"mean_rate_hz": 0.57,
               "episode": {"per_lane": [{"task_score_clean": -4.4,
                                         "disturbances": []}]},
               "guard": {"tripped": True,
                         "reason": "max_propagated_edge_events",
                         "deterministic": True}}
    cfg = {"fitness": {"weights": {"runaway_guard": 0.5}}}
    tripped = F.compute_metrics(summary, {}, cfg, 5.0)
    assert tripped["runaway_terminated"] is True
    clean = F.compute_metrics({k: v for k, v in summary.items()
                               if k != "guard"}, {}, cfg, 5.0)
    assert clean["runaway_terminated"] is False
    assert (F.selection_score(tripped, cfg)
            < F.selection_score(clean, cfg))
    # weight 0 (M1 default) ignores the guard entirely
    m1 = F.compute_metrics(summary, {}, {"fitness": {}}, 5.0)
    assert F.selection_score(m1, {"fitness": {}}) == \
        F.selection_score(clean, {"fitness": {}})


def test_maintenance_cost_is_config_keyed_and_default_zero():
    st = {"n_enabled_organs": 2, "n_artificial_neurons": 40,
          "n_enabled_attachments": 5}
    summary = {"mean_rate_hz": 0.57}
    m1 = F.compute_metrics(summary, st, {"fitness": {}}, 5.0)
    assert m1["structural_maintenance_cost"] == 0.0
    assert m1["selection_components"]["structural_maintenance"] is None
    cfg = {"fitness": {"weights": {"structural_maintenance": 1.0},
                       "maintenance": {"per_organ": 0.01,
                                       "per_artificial_neuron": 0.001,
                                       "per_attachment": 0.005}}}
    m2 = F.compute_metrics(summary, st, cfg, 5.0)
    assert m2["structural_maintenance_cost"] == pytest.approx(
        2 * 0.01 + 40 * 0.001 + 5 * 0.005)
    assert m2["selection_components"]["structural_maintenance"] == \
        pytest.approx(-m2["structural_maintenance_cost"])
    assert (F.selection_score(m2, cfg)
            < F.selection_score(m1, {"fitness": {}}))
