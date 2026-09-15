"""Neuron observation must not change the evaluation it displays."""
import copy
import pickle
import threading
from types import SimpleNamespace

import pytest

from experiments.mioba.development.phenotype import develop
from experiments.mioba.evolution.fitness import compute_metrics
from experiments.mioba.fba.mock_backend import MockBackend
from experiments.mioba.fba.replicates import replicate_seeds
from experiments.mioba.genome.schema import fba0_genome
from experiments.mioba.workers import worker as W
from experiments.mioba.workers.neural_activity import (
    NeuralActivityObserver, NeuralActivityPublisher, evenly_spaced_indices)


def _job(replicates=3, environment=False, backend="mock"):
    genome = fba0_genome(seed=7)
    return {"experiment_id": "exp_activity", "genome_id": genome.genome_id,
            "genome_json": genome.to_json(), "job_id": "job_activity",
            "backend": backend, "attempt": 2, "seed": 77,
            "replicates": replicates,
            "replicate_seeds": replicate_seeds(77, replicates),
            "duration_ms": 60.0, "environment_id": "synthetic-quiet-v0",
            "config": {"env": {"stim_fraction": 0.5, "stim_rate_hz": 200.0},
                       "environment": {"enabled": environment,
                                       "virtual": {"slices": 6},
                                       "disturbance": {"enabled": False}}}}


def _backend(name):
    if name == "mock":
        return MockBackend(n_neurons=32, connectivity=0.1)
    pytest.importorskip("torch")
    from experiments.mioba.fba.torch_backend import TorchBackend
    return TorchBackend(synthetic=True, synthetic_neurons=32,
                        connectivity=0.1, topology_cache=False)


def _state(backend):
    def plain(value):
        if isinstance(value, dict):
            return {k: plain(v) for k, v in value.items()}
        if isinstance(value, (list, tuple)):
            return [plain(v) for v in value]
        if hasattr(value, "tolist"):
            return value.tolist()
        return value
    return plain(pickle.loads(backend.checkpoint()))


def _science(summary):
    out = {k: copy.deepcopy(v) for k, v in summary.items()
           if k not in {"wall_s", "sim_seconds_per_wall_second", "timing",
                        "vram", "topology"}}
    # This nested operational timer is not a neural/selection metric.
    if out.get("propagation_activity"):
        out["propagation_activity"].pop("event_propagation_seconds", None)
    return out


def test_selection_is_fixed_bounded_and_spans_population():
    assert evenly_spaced_indices(3) == [0, 1, 2]
    assert evenly_spaced_indices(1000, 4) == [0, 333, 666, 999]
    assert evenly_spaced_indices(1000, 1) == [0]
    assert len(set(evenly_spaced_indices(139000, 512))) == 512
    for bad in (0, 513, True):
        with pytest.raises(ValueError):
            evenly_spaced_indices(1000, bad)


@pytest.mark.parametrize("name", ["mock", "torch"])
def test_getter_returns_a_copy_without_changing_state_or_rng(name):
    b = _backend(name)
    b.initialize(develop(fba0_genome(), base_neurons=32), 2, 7, "cpu")
    b.set_inputs({"rates_hz": {"all": 200.0}})
    b.run(5)
    before = _state(b)
    indices = [0, 7, 31]
    observed = b.get_neuron_activity(indices, lane=1)
    assert observed["spike_counts"] == b.spike_counts[1, indices].tolist()
    observed["spike_counts"][0] = -999
    observed["neuron_indices"][0] = -999
    assert indices == [0, 7, 31]
    for _ in range(3):
        b.get_neuron_activity(indices)
    assert _state(b) == before
    for invalid in ([32], [-1], [True], [0, 0], [], list(range(513))):
        with pytest.raises(ValueError):
            b.get_neuron_activity(invalid)
    with pytest.raises(ValueError):
        b.get_neuron_activity([0], lane=2)


@pytest.mark.parametrize("name", ["mock", "torch"])
@pytest.mark.parametrize("environment", [False, True])
def test_observation_keeps_neural_state_seeds_and_science_identical(name, environment):
    job = _job(environment=environment, backend=name)
    phenotype = develop(fba0_genome(), base_neurons=32)
    a, b = _backend(name), _backend(name)
    frames = []
    observer = NeuralActivityObserver(job, "worker_test", frames.append,
                                      sample_size=32, interval_s=0)
    off = W.evaluate_replicates(a, phenotype, job, "cpu", 2)
    on = W.evaluate_replicates(b, phenotype, job, "cpu", 2, observer=observer)
    assert _state(a) == _state(b)
    assert _science(off["summary"]) == _science(on["summary"])
    assert compute_metrics(off["summary"], {}, job["config"], 5.0) == \
        compute_metrics(on["summary"], {}, job["config"], 5.0)
    assert on["replicate_seeds"] == off["replicate_seeds"]
    assert frames and set(f["replicate_index"] for f in frames) == {0, 2}
    assert [f["sequence"] for f in frames] == list(range(1, len(frames) + 1))
    for replicate in (0, 2):
        windows = [f for f in frames if f["replicate_index"] == replicate]
        assert windows[0]["window_start_ms"] == 0
        for previous, current in zip(windows, windows[1:]):
            assert previous["window_end_ms"] == current["window_start_ms"]
        assert sum(sum(f["spike_counts"]) for f in windows) == \
            on["summary"]["per_replicate_spike_counts"][replicate]
        assert all(f["replicate_seed"] == job["replicate_seeds"][replicate]
                   for f in windows)
    assert on["summary"]["spikes_total"] > 0


def test_batch_representative_uses_global_indices_and_always_emits_final_window():
    job = _job(replicates=5)
    phenotype = develop(fba0_genome(), base_neurons=32)
    all_frames = {}
    for batch in (1, 2, 5):
        frames = []
        observer = NeuralActivityObserver(job, "w", frames.append,
                                          sample_size=32, interval_s=10000)
        out = W.evaluate_replicates(_backend("mock"), phenotype, job, "cpu",
                                    batch, observer=observer)
        assert [f["replicate_index"] for f in frames] == list(range(0, 5, batch))
        for f in frames:
            assert sum(f["spike_counts"]) == \
                out["summary"]["per_replicate_spike_counts"][f["replicate_index"]]
        all_frames[batch] = frames
    for batch in (2, 5):
        assert all_frames[batch][0]["spike_counts"] == all_frames[1][0]["spike_counts"]


def test_cadence_final_window_and_unexpected_rewind_are_explicit(caplog):
    job = _job(1)
    b = _backend("mock")
    b.initialize(develop(fba0_genome(), base_neurons=32), 1, 77, "cpu")
    frames = []
    wall = [0.0]
    observer = NeuralActivityObserver(job, "w", frames.append,
                                      clock=lambda: wall[0])
    observer.begin_batch(b, [0], job["replicate_seeds"])
    b.run(1)
    wall[0] = 0.1
    observer.sample(b)
    assert not frames
    wall[0] = 0.5
    observer.sample(b)
    b.run(1)
    observer.sample(b, final=True)
    observer.sample(b, final=True)
    assert len(frames) == 2
    assert frames[0]["window_end_ms"] == frames[1]["window_start_ms"]
    b.reset()
    observer.sample(b, final=True)
    assert len(frames) == 2 and "rewound" in observer.last_error
    assert "rewound" in caplog.text
    observer.start_pass(1)
    observer.begin_batch(b, [0], job["replicate_seeds"])
    b.run(1)
    observer.sample(b, final=True)
    assert frames[-1]["evaluation_pass"] == 1
    assert frames[-1]["sequence"] == 3 and frames[-1]["window_start_ms"] == 0


def test_decreasing_counts_are_not_clamped(caplog):
    job = _job(1)
    b = _backend("mock")
    b.initialize(develop(fba0_genome(), base_neurons=32), 1, 77, "cpu")
    b.spike_counts[0, 0] = 3
    frames = []
    observer = NeuralActivityObserver(job, "w", frames.append)
    observer.begin_batch(b, [0], job["replicate_seeds"])
    b.t_ms = 1.0
    b.spike_counts[0, 0] = 2
    observer.sample(b, final=True)
    assert not frames and "rewound" in observer.last_error
    assert "rewound" in caplog.text


def test_publisher_is_bounded_and_shuts_down_after_network_failures():
    class FailingClient:
        def post(self, path, **kwargs):
            assert path == "/api/worker/neural-activity"
            assert kwargs["timeout"] == 0.3
            raise OSError("offline")
    publisher = NeuralActivityPublisher(FailingClient())
    for sequence in range(100):
        publisher.publish({"sequence": sequence})
    publisher.close()
    assert not publisher._thread.is_alive()
    assert publisher.failed > 0
    assert publisher.dropped > 0
    assert not publisher._pending


def test_publisher_drops_old_frames_and_flushes_latest():
    entered, release = threading.Event(), threading.Event()
    sent = []
    class Client:
        def post(self, path, json, timeout):
            sent.append(json["sequence"])
            entered.set()
            assert release.wait(timeout=0.5)
            return SimpleNamespace(status_code=200)
    publisher = NeuralActivityPublisher(Client())
    publisher.publish({"sequence": 1})
    assert entered.wait(timeout=0.5)
    for sequence in range(2, 20):
        publisher.publish({"sequence": sequence})
    assert len(publisher._pending) == 2
    release.set()
    publisher.close()
    assert sent[0] == 1 and sent[-1] == 19
    assert not publisher._thread.is_alive()


def test_local_sink_failure_does_not_fail_evaluation():
    def fail(_frame):
        raise OSError("local sink failed")
    job = _job(1)
    observer = NeuralActivityObserver(job, "w", fail, interval_s=0)
    out = W.evaluate_replicates(_backend("mock"),
                                develop(fba0_genome(), base_neurons=32),
                                job, "cpu", 1, observer=observer)
    assert out["completed_replicates"] == 1
    assert observer.last_error == "snapshot or local publisher unavailable"


def test_run_job_oom_pass_and_genome_fallback_keep_sequence(monkeypatch):
    job = _job(3)
    del job["genome_id"]  # compatibility with older coordinator claims
    frames = []
    publishers = []
    class Publisher:
        def __init__(self, _client):
            self.closed = False
            publishers.append(self)
        def publish(self, frame):
            frames.append(copy.deepcopy(frame))
        def close(self):
            self.closed = True
    monkeypatch.setattr(W, "NeuralActivityPublisher", Publisher)
    monkeypatch.setattr(W, "get_backend", lambda *a, **k: _backend("mock"))
    actual = W.evaluate_replicates
    calls = []
    def evaluate(*args, **kwargs):
        result = actual(*args, **kwargs)
        calls.append(args[4])
        if len(calls) == 1:
            raise RuntimeError("CUDA out of memory")
        return result
    monkeypatch.setattr(W, "evaluate_replicates", evaluate)
    body = W.run_job(None, "worker_test", job, "cpu", 2,
                    deliver=False, profile=False, neural_activity=True)
    assert body["status"] == "SUCCEEDED"
    assert calls == [2, 1]
    assert publishers[0].closed
    assert {f["evaluation_pass"] for f in frames} == {0, 1}
    assert [f["sequence"] for f in frames] == list(range(1, len(frames) + 1))
    assert all(f["genome_id"] == fba0_genome(seed=7).genome_id for f in frames)
    assert all(f["phase"] == "evaluation" for f in frames)


def test_run_job_network_failure_is_nonfatal_and_sender_exits(monkeypatch):
    monkeypatch.setattr(W, "get_backend", lambda *a, **k: _backend("mock"))
    class Client:
        def post(self, *args, **kwargs):
            raise OSError("offline")
    before = {t.ident for t in threading.enumerate()}
    job = _job(1)
    off = W.run_job(None, "w", job, "cpu", 1, deliver=False, profile=False)
    on = W.run_job(Client(), "w", job, "cpu", 1, deliver=False,
                   profile=False, neural_activity=True)
    assert on["status"] == off["status"] == "SUCCEEDED"
    assert _science(on["evaluation"]["summary"]) == _science(off["evaluation"]["summary"])
    assert not [t for t in threading.enumerate()
                if t.name == "mioba-neural-activity" and t.ident not in before]
