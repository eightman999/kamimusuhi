"""AFC A0/G0: canonical anatomy store loading and graft compilation.

Covers the acceptance-relevant mechanics on a tiny fixture store so the
tests stay fast — the real BANC store is exercised by
``scripts/a0g0_vertical.py``, not unit tests.
"""
import pytest

torch = pytest.importorskip("torch")
pd = pytest.importorskip("pandas")

from experiments.mioba.anatomy import schema as A            # noqa: E402
from experiments.mioba.graft.schema import (                # noqa: E402
    GraftSpec, Graft, GraftLink, HostSelector)
from experiments.mioba.graft.build import (                 # noqa: E402
    compile_grafts, resolve_selector)
from experiments.mioba.fba.torch_backend import TorchBackend  # noqa: E402
from experiments.mioba.fba.topology import (                # noqa: E402
    organ_topology_hash)

N = 60


def _store(tmp_path):
    neurons = pd.DataFrame({
        "neuron_idx": list(range(N)),
        "dataset_id": [f"banc:{1000 + i}" for i in range(N)],
        "soma_x": [float(i) for i in range(N)],
        "soma_y": [float(i * 2) for i in range(N)],
        "soma_z": [0.0] * N,
        "cell_type": ["ct_a"] * N,
        "flow_class": ["afferent"] * 10 + ["intrinsic"] * 40
                      + ["efferent"] * 10,
        "super_class": ["sensory"] * 10 + ["intrinsic"] * 40
                       + ["descending"] * 10,
        "neuropil": ["np1"] * N,
        "side": ["left"] * N,
        "nt_top": [None] * N,
        "nt_confidence": [None] * N,
        "syn_pre_total": [1] * N,
        "syn_post_total": [1] * N,
        "morphology_ref": [None] * N,
        "hemilineage": [None] * N,
        "birth_time": [None] * N,
    })
    conn = pd.DataFrame({
        "pre_idx": list(range(N - 1)),
        "post_idx": list(range(1, N)),
        "anatomical_count": [1] * (N - 1),
        "weight": [0.5] * (N - 1),
        "weight_provenance": ["MODEL_INFERENCE"] * (N - 1),
    })
    man = A.manifest_template()
    man["dataset_kind"] = "toy_v1"
    A.write_store(tmp_path / "store", neurons, conn, man)
    return tmp_path / "store"


def _backend(store, phenotype, seed=7):
    b = TorchBackend(anatomy_dir=str(store), synthetic=False)
    b.initialize(phenotype, batch_size=1, seed=seed, device="cpu",
                 replicate_seeds=[seed])
    return b


def _spec(direction="in", flow="afferent", sup=None, ids=None):
    sel = HostSelector(flow_class=flow, super_class=sup,
                       dataset_ids=ids, max_targets=8, seed=3)
    link = GraftLink(link_id="g001:l0", direction=direction,
                     host=sel, p=1.0, weight=5.0)
    return GraftSpec(grafts=[Graft(graft_id="g001", n_neurons=4,
                                  links=[link])])


def test_anatomy_store_loads_into_backend(tmp_path):
    store = _store(tmp_path)
    phen = {"artificial_organs": [], "attachments": [],
            "n_extra_neurons": 0, "params": {}}
    b = _backend(store, phen)
    assert b.n_base == N and b.n == N
    ident = b.dataset_identity()
    assert ident["dataset_id"] == "toy_v1"
    assert ident["manifest_hash"]
    # base graph carries the imported edges, not synthetic randoms
    assert b.nnz_base == N - 1


def test_selector_resolution(tmp_path):
    store = _store(tmp_path)
    _m, neurons, _c = A.load_store(store)
    idx = resolve_selector(HostSelector(flow_class="afferent",
                                        max_targets=6, seed=1),
                           neurons, "banc")
    assert len(idx) == 6 and all(i < 10 for i in idx)
    idx = resolve_selector(HostSelector(dataset_ids=["banc:1005",
                                                     "banc:1042"]),
                           neurons, "banc")
    assert idx == [5, 42]
    with pytest.raises(ValueError):
        resolve_selector(HostSelector(flow_class="nonexistent"),
                         neurons, "banc")
    with pytest.raises(ValueError):
        resolve_selector(HostSelector(dataset_ids=["banc:999999"]),
                         neurons, "banc")


def test_bidirectional_link_wires_both_legs(tmp_path):
    store = _store(tmp_path)
    phen = compile_grafts(_spec(direction="bidirectional"), store)
    (att,) = phen["attachments"]
    assert att["direction"] == "bidirectional"
    b = _backend(store, phen)
    ij = b.W_extra.to_coo().coalesce().indices()
    post, pre = ij[0].tolist(), ij[1].tolist()
    h2g = sum(1 for p, r in zip(post, pre) if p >= N and r < N)
    g2h = sum(1 for p, r in zip(post, pre) if p < N and r >= N)
    assert h2g > 0 and g2h > 0


def test_disabled_graft_wires_nothing(tmp_path):
    store = _store(tmp_path)
    spec = _spec()
    spec.grafts[0].enabled = False
    phen = compile_grafts(spec, store)
    b = _backend(store, phen)
    assert phen["artificial_organs"] == [] and b.W_extra is None


def test_topology_hash_covers_endpoint_indices(tmp_path):
    store = _store(tmp_path)
    a = compile_grafts(_spec(ids=["banc:1001", "banc:1002"]), store)
    b = compile_grafts(_spec(ids=["banc:1003", "banc:1004"]), store)
    assert organ_topology_hash(a, 0) != organ_topology_hash(b, 0)


def test_graft_receives_driven_input(tmp_path):
    """End-to-end mini check: driven host cells that feed the graft make
    graft neurons fire — the G0 acceptance property in miniature."""
    store = _store(tmp_path)
    phen = compile_grafts(_spec(direction="in"), store)
    att = phen["attachments"][0]
    b = _backend(store, phen)
    b.set_inputs({"rates_hz": {str(i): 400.0
                               for i in att["source_idx"]}})
    b.run(100.0)
    assert b.spike_counts[0, N:].sum().item() > 0
