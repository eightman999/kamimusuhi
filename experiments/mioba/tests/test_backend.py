import pytest

from experiments.mioba.fba.backend import BackendUnavailable
from experiments.mioba.fba.mock_backend import MockBackend
from experiments.mioba.fba.registry import available_backends, get_backend


def _phen():
    from experiments.mioba.development.phenotype import develop
    from experiments.mioba.genome.schema import fba0_genome
    return develop(fba0_genome())


def test_mock_deterministic():
    a, b = MockBackend(n_neurons=64), MockBackend(n_neurons=64)
    a.initialize(_phen(), batch_size=2, seed=7, device="cpu")
    b.initialize(_phen(), batch_size=2, seed=7, device="cpu")
    a.set_inputs({"rates_hz": {"slice:0-5": 50.0}, "silence": []})
    b.set_inputs({"rates_hz": {"slice:0-5": 50.0}, "silence": []})
    sa = a.run(50)
    sb = b.run(50)
    assert sa["spikes_total"] == sb["spikes_total"]
    assert a.get_state_summary() == b.get_state_summary()


def test_mock_checkpoint_restore():
    m = MockBackend(n_neurons=64)
    m.initialize(_phen(), batch_size=1, seed=3, device="cpu")
    m.set_inputs({"rates_hz": {"slice:0-4": 40.0}, "silence": []})
    m.step(20)
    blob = m.checkpoint()
    m.step(20)
    after = m.spike_counts.copy(), m.v.copy()
    m.restore(blob)
    m.step(20)
    assert (m.spike_counts == after[0]).all()
    assert (m.v == after[1]).all()


def test_mock_population_activity_with_organ():
    phen = _phen()
    phen["n_extra_neurons"] = 16
    phen["artificial_organs"] = [{"organ_id": "orgA", "kind": "lif_cluster",
                                  "size": 16, "params": {}}]
    m = MockBackend(n_neurons=64)
    m.initialize(phen, batch_size=2, seed=1, device="cpu")
    m.run(20)
    act = m.get_population_activity(["fba0", "organ:orgA", "all"])
    assert all(len(v) == 2 for v in act.values())


def test_torch_synthetic_cpu():
    pytest.importorskip("torch")
    b = get_backend("torch", synthetic=True, synthetic_neurons=500)
    b.initialize(_phen(), batch_size=2, seed=0, device="cpu")
    b.set_inputs({"rates_hz": {"slice:0-5": 50.0}, "silence": []})
    stats = b.run(20)
    assert stats["simulated_ms"] >= 19.9
    s = b.get_state_summary()
    assert s["vram_bytes"] is None
    blob = b.checkpoint()
    b.restore(blob)
    act = b.get_population_activity(["all"])
    assert len(act["all"]) == 2


def test_registry_availability():
    avail = available_backends()
    assert avail["mock"]["available"] is True
    assert "genn" in avail and "torch" in avail


def test_genn_unavailable_or_usable():
    try:
        b = get_backend("genn")
        assert b.name == "genn"
    except BackendUnavailable as exc:
        assert "pygenn" in str(exc)


def test_unknown_backend():
    with pytest.raises(BackendUnavailable):
        get_backend("nope")
