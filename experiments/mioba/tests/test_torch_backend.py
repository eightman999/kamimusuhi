"""Torch backend: sparse-only construction, connectivity direction,
deterministic RNG/checkpoint replay, phenotype params, attachment
semantics."""
import pytest

torch = pytest.importorskip("torch")

from experiments.mioba.development.phenotype import develop  # noqa: E402
from experiments.mioba.fba.params import UnsupportedAttachmentRegion  # noqa: E402
from experiments.mioba.fba.torch_backend import (  # noqa: E402
    SYNTHETIC_REGION_MODE, TorchBackend)
from experiments.mioba.genome.schema import (  # noqa: E402
    ArtificialOrgan, Attachment, ParameterMutation, fba0_genome)


def _organ_genome(size=32, region="medulla", direction="forward",
                  weight_scale=1.0):
    g = fba0_genome()
    g.artificial_organs.append(ArtificialOrgan(organ_id="orgA",
                                               kind="lif_cluster", size=size))
    g.attachments.append(Attachment(attachment_id="attA",
                                    source=f"fba0:{region}", target="orgA",
                                    direction=direction,
                                    weight_scale=weight_scale))
    return g.finalize()


def _forbid_dense(monkeypatch, n):
    """Fail the test if any (n, n) dense tensor is created or a sparse
    tensor is densified."""
    def boom(*a, **k):
        raise AssertionError("dense N x N allocation")
    monkeypatch.setattr(torch.Tensor, "to_dense", boom)
    for fn in ("zeros", "rand", "randn", "ones", "empty", "full"):
        orig = getattr(torch, fn)

        def guard(*a, _orig=orig, **k):
            shape = a[0] if a and isinstance(a[0], (tuple, list, torch.Size)) \
                else a[: len([x for x in a if isinstance(x, int)])]
            if len(shape) == 2 and shape[0] >= n and shape[1] >= n:
                raise AssertionError(f"dense {tuple(shape)} allocation")
            return _orig(*a, **k)
        monkeypatch.setattr(torch, fn, guard)


# ------------------------------------------------------------ blocker 1
def test_organs_stay_sparse_no_dense_allocation(monkeypatch):
    n_base = 1500
    phen = develop(_organ_genome(size=64), base_neurons=n_base)
    _forbid_dense(monkeypatch, n_base)
    b = TorchBackend(synthetic=True, synthetic_neurons=n_base,
                     connectivity=0.01)
    b.initialize(phen, batch_size=2, seed=1, device="cpu")
    assert b.W.layout == torch.sparse_csr
    assert b.n == n_base + 64
    b.set_inputs({"rates_hz": {"all": 20.0}, "silence": []})
    b.run(5)
    # memory is O(E_base + organ_edges): nnz far below N^2 and bounded by
    # the sampled edge count
    assert b.nnz <= b.n_edges_total
    assert b.nnz < 0.02 * b.n * b.n
    expected_base = 0.01 * n_base * n_base
    assert b.n_edges_base == pytest.approx(expected_base, rel=0.05)
    organ_edges = b.n_edges_total - b.n_edges_base
    assert 0 < organ_edges < 0.5 * expected_base


# ------------------------------------------------------------ blocker 2
def _fixed_network(edges, n=3, w=1.0):
    """Backend over an explicit pre->post edge list, no random edges."""
    b = TorchBackend(synthetic=True, synthetic_neurons=n, connectivity=0.0)
    b.initialize(develop(fba0_genome(), base_neurons=n), batch_size=1,
                 seed=0, device="cpu")
    post = torch.tensor([p for _, p in edges], dtype=torch.int64)
    pre = torch.tensor([q for q, _ in edges], dtype=torch.int64)
    W = torch.sparse_coo_tensor(torch.stack([post, pre]),
                                torch.full((len(edges),), w), (n, n))
    b.W = W.coalesce().to_sparse_csr()
    return b


def test_propagation_direction_pre_to_post():
    b = _fixed_network([(0, 1)])  # only neuron 0 -> neuron 1
    spikes = torch.zeros(1, 3)
    spikes[0, 0] = 1.0
    out = b.propagate(spikes)
    assert out.tolist() == [[0.0, 1.0, 0.0]]
    # a spike of neuron 1 must not reach 0 (no reverse edge) nor 2
    spikes = torch.zeros(1, 3)
    spikes[0, 1] = 1.0
    assert b.propagate(spikes).tolist() == [[0.0, 0.0, 0.0]]


def test_forced_spike_drives_only_postsynaptic_target():
    b = _fixed_network([(0, 1)], w=50.0)
    b.set_inputs({"rates_hz": {}, "silence": []})
    b.force_spikes([0])
    b.step(int(round(5.0 / b.params["dt"])))
    counts = b.spike_counts[0].tolist()
    assert counts[0] > 0            # forced
    assert counts[1] > 0            # received input from 0
    assert counts[2] == 0           # not connected
    # membrane of neuron 2 never moved off rest
    assert b.v[0, 2].item() == pytest.approx(b.params["vRest"])


# ------------------------------------------------------------ blocker 3
def _make(seed=3, n=300):
    phen = develop(_organ_genome(size=16), base_neurons=n)
    b = TorchBackend(synthetic=True, synthetic_neurons=n, connectivity=0.02)
    b.initialize(phen, batch_size=2, seed=seed, device="cpu")
    b.set_inputs({"rates_hz": {"all": 200.0}, "silence": []})
    return b


def test_same_seed_same_result():
    a, b = _make(), _make()
    a.step(200)
    b.step(200)
    assert torch.equal(a.spike_counts, b.spike_counts)
    assert torch.equal(a.v, b.v)
    assert a.spike_counts.sum() > 0
    c = _make(seed=4)
    c.step(200)
    assert not torch.equal(a.spike_counts, c.spike_counts)


def test_checkpoint_restore_replays_identically():
    b = _make()
    b.step(50)
    blob = b.checkpoint()
    b.step(100)
    first = (b.spike_counts.clone(), b.v.clone(), b.g.clone(), b.t_ms)
    b.step(37)  # move away so restore must really rewind the RNG
    b.restore(blob)
    b.step(100)
    assert torch.equal(b.spike_counts, first[0])
    assert torch.equal(b.v, first[1])
    assert torch.equal(b.g, first[2])
    assert b.t_ms == pytest.approx(first[3])
    assert first[0].sum() > 0


def test_step_rng_does_not_touch_global_torch_rng():
    torch.manual_seed(123)
    before = torch.get_rng_state()
    b = _make()
    b.step(20)
    assert torch.equal(torch.get_rng_state(), before)


# ------------------------------------------------------------ blocker 7
def test_wscale_mutation_changes_weights():
    n = 200
    base = develop(fba0_genome(), base_neurons=n)
    g = fba0_genome()
    g.parameter_mutations.append(ParameterMutation(
        mutation_id="m1", path="wScale", op="scale", value=2.0))
    mutated = develop(g.finalize(), base_neurons=n)
    assert mutated["params"]["wScale"] == pytest.approx(
        2.0 * base["params"]["wScale"])
    a = TorchBackend(synthetic=True, synthetic_neurons=n, connectivity=0.05)
    b = TorchBackend(synthetic=True, synthetic_neurons=n, connectivity=0.05)
    a.initialize(base, batch_size=1, seed=5, device="cpu")
    b.initialize(mutated, batch_size=1, seed=5, device="cpu")
    assert b.params["wScale"] == pytest.approx(2.0 * a.params["wScale"])
    assert torch.allclose(b.W.values(), 2.0 * a.W.values())


def test_attachment_edges_respect_source_target_direction():
    n = 700
    phen = develop(_organ_genome(size=16, region="medulla"), base_neurons=n)
    b = TorchBackend(synthetic=True, synthetic_neurons=n, connectivity=0.0)
    b.initialize(phen, batch_size=1, seed=2, device="cpu")
    assert b.region_mode == SYNTHETIC_REGION_MODE
    lo, hi = b.region_range("medulla")
    coo = b.W.to_sparse_coo().coalesce()
    post, pre = coo.indices()
    organ = (post >= n) & (pre >= n)
    att = ~organ
    assert att.any()
    # forward: pre in medulla range, post in organ
    assert bool(((pre[att] >= lo) & (pre[att] < hi)).all())
    assert bool((post[att] >= n).all())
    # no organ -> fba0 edges for a forward attachment
    assert not bool(((pre >= n) & (post < n)).any())


def test_bidirectional_attachment_adds_reverse_leg():
    n = 700
    phen = develop(_organ_genome(size=16, direction="bidirectional"),
                   base_neurons=n)
    b = TorchBackend(synthetic=True, synthetic_neurons=n, connectivity=0.0)
    b.initialize(phen, batch_size=1, seed=2, device="cpu")
    post, pre = b.W.to_sparse_coo().coalesce().indices()
    assert bool(((pre >= n) & (post < n)).any())


def test_attachment_weight_scale_applied():
    n = 700
    p1 = develop(_organ_genome(size=16, weight_scale=1.0), base_neurons=n)
    p2 = develop(_organ_genome(size=16, weight_scale=3.0), base_neurons=n)
    a = TorchBackend(synthetic=True, synthetic_neurons=n, connectivity=0.0)
    b = TorchBackend(synthetic=True, synthetic_neurons=n, connectivity=0.0)
    a.initialize(p1, batch_size=1, seed=2, device="cpu")
    b.initialize(p2, batch_size=1, seed=2, device="cpu")
    ca, cb = a.W.to_sparse_coo().coalesce(), b.W.to_sparse_coo().coalesce()
    assert torch.equal(ca.indices(), cb.indices())
    att = ca.indices()[1] < n
    assert torch.allclose(cb.values()[att], 3.0 * ca.values()[att])
    assert torch.allclose(cb.values()[~att], ca.values()[~att])


def test_region_attachment_without_mapping_is_rejected():
    phen = develop(_organ_genome(size=8), base_neurons=100)
    b = TorchBackend(synthetic=True, synthetic_neurons=100,
                     region_mode="none")
    with pytest.raises(UnsupportedAttachmentRegion):
        b.initialize(phen, batch_size=1, seed=0, device="cpu")
    phen = develop(_organ_genome(size=8, region="not_a_region"),
                   base_neurons=100)
    b = TorchBackend(synthetic=True, synthetic_neurons=100)
    with pytest.raises(UnsupportedAttachmentRegion):
        b.initialize(phen, batch_size=1, seed=0, device="cpu")


def test_dataset_identity_is_logical_not_a_path():
    b = TorchBackend(synthetic=True, synthetic_neurons=50)
    b.initialize(develop(fba0_genome(), base_neurons=50), batch_size=1,
                 seed=0, device="cpu")
    ident = b.dataset_identity()
    assert ident["dataset_id"] == "synthetic-fba"
    assert ident["region_mode"] == SYNTHETIC_REGION_MODE
    assert "/" not in ident["version"]
