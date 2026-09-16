"""A2/G0.1: SWC parsing, compartment reduction, coupling runtime and
compartment-aware graft selection — on tiny fixtures so the tests stay
fast."""
import numpy as np
import pytest

torch = pytest.importorskip("torch")
pd = pytest.importorskip("pandas")

from experiments.mioba.anatomy import morphology as M          # noqa: E402
from experiments.mioba.anatomy import compartments as C        # noqa: E402
from experiments.mioba.graft.schema import HostSelector        # noqa: E402
from experiments.mioba.graft.build import resolve_selector     # noqa: E402
from experiments.mioba.fba.torch_backend import TorchBackend   # noqa: E402


def _swc(rows):
    return np.asarray(rows, dtype=float)


def _labeled_skeleton():
    """soma(1) -> axon(2) chain, soma -> dendrite(3) chain, +prim(4)."""
    return _swc([
        [1, 7, 0, 0, 0, 2, -1],     # soma root
        [2, 2, 10, 0, 0, 1, 1],     # axon
        [3, 2, 20, 0, 0, 1, 2],     # axon
        [4, 3, 0, 5, 0, 1, 1],      # dendrite prox
        [5, 3, 0, 40, 0, 1, 4],     # dendrite dist
        [6, 4, 0, 10, 5, 1, 4],     # primary neurite
    ])


# ------------------------------------------------------------- SWC ----
def test_parse_and_validate_good_swc(tmp_path):
    p = tmp_path / "x.swc"
    p.write_text("1 7 0 0 0 2 -1\n2 3 0 1 0 1 1\n3 3 0 2 0 1 2\n")
    nodes = M.parse_swc(p)
    assert nodes.shape == (3, 7)
    v = M.validate_swc(nodes)
    assert v["status"] == "VALID"


def test_validate_swc_rejects_bad_topology():
    dup = _swc([[1, 7, 0, 0, 0, 1, -1], [1, 3, 0, 1, 0, 1, 1]])
    assert M.validate_swc(dup)["status"] != "VALID"
    orphan = _swc([[1, 7, 0, 0, 0, 1, -1], [2, 3, 0, 1, 0, 1, 99]])
    assert M.validate_swc(orphan)["status"] != "VALID"
    disc = _swc([[1, 7, 0, 0, 0, 1, -1], [2, 3, 0, 1, 0, 1, 1],
                 [3, 3, 5, 5, 5, 1, 3]])   # self-parented island
    assert M.validate_swc(disc)["status"] != "VALID"


# ------------------------------------------------------- reduction ----
def test_reduce_entity_labeled_skeleton():
    red = C.reduce_entity(7, _labeled_skeleton())
    assert set(red.compartments) >= {"SOMA", "AXON", "DENDRITE_PROX",
                                     "DENDRITE_DIST"}
    assert red.original_nodes == 6 and red.valid
    row = red.manifest_row()
    assert row["runtime_compartments"] == len(red.compartments)
    assert row["mapping_hash"] and row["algorithm"] == "reduce-v0"


def test_reduce_entity_unlabeled_falls_back_to_soma():
    nodes = _swc([[1, 0, 0, 0, 0, 1, -1], [2, 0, 0, 1, 0, 1, 1]])
    red = C.reduce_entity(9, nodes)
    assert red.compartments == ["SOMA"]   # UNKNOWN never invented


def test_compile_reduced_graph_routes_and_couples():
    conn = pd.DataFrame({"pre_idx": [0, 1], "post_idx": [1, 0],
                         "weight": [1.0, 1.0],
                         "post_compartment": ["DENDRITE", "SOMA"]})
    reds = {0: C.reduce_entity(0, _labeled_skeleton()),
            1: C.reduce_entity(1, _labeled_skeleton())}
    n, post, pre, w, coup, rows, man = C.compile_reduced_graph(conn, reds)
    comps = {(r["entity_idx"], r["compartment"]): r["runtime_idx"]
             for r in rows}
    assert n == len(rows)
    # edge 0->1 labeled DENDRITE must land on a dendrite compartment
    i = list(post).index(comps[(1, "DENDRITE_PROX")]) \
        if comps[(1, "DENDRITE_PROX")] in post else None
    # either PROX or DIST — but not SOMA and not AXON
    dst = [p for p, r in zip(post, pre)
           if r == comps[(0, "SOMA")]]
    assert all(p in (comps[(1, "DENDRITE_PROX")],
                     comps[(1, "DENDRITE_DIST")]) for p in dst)
    # all emissions leave from SOMA
    assert set(pre.tolist()) <= {comps[(0, "SOMA")], comps[(1, "SOMA")]}
    # coupling matrix: symmetric off-diagonal, negative diagonal
    crow, ccol, cval = coup
    for a, b in ((comps[(0, "SOMA")], comps[(0, "DENDRITE_PROX")]),):
        assert any(i == a and j == b and v > 0
                   for i, j, v in zip(crow, ccol, cval))
        assert any(i == b and j == a and v > 0
                   for i, j, v in zip(crow, ccol, cval))
        assert any(i == a and j == a and v < 0
                   for i, j, v in zip(crow, ccol, cval))
    assert man["coupling_model"].endswith("MODEL_INFERENCE")


def test_coupling_relays_dendritic_input_to_soma():
    """A 2-compartment entity: driving DENDRITE_DIST must depolarize
    SOMA through the coupling — the A2 runtime property."""
    conn = pd.DataFrame({"pre_idx": [0], "post_idx": [0],
                         "weight": [0.0], "post_compartment": ["SOMA"]})
    red = C.reduce_entity(0, _labeled_skeleton())
    n, post, pre, w, coup, rows, _m = C.compile_reduced_graph(
        conn, {0: red})
    dst = next(r["runtime_idx"] for r in rows
               if r["compartment"] == "DENDRITE_DIST")
    soma = next(r["runtime_idx"] for r in rows
                if r["compartment"] == "SOMA")
    b = TorchBackend(synthetic=False,
                     base_override=(n, post, pre, w),
                     voltage_coupling=coup)
    b.initialize({"artificial_organs": [], "attachments": [],
                  "n_extra_neurons": 0, "params": {}},
                 batch_size=1, seed=1, device="cpu",
                 replicate_seeds=[1])
    b.set_inputs({"rates_hz": {str(dst): 400.0}})
    b.run(50.0)
    # dendrite may or may not spike; soma must have depolarized past v0
    assert b.v[0, soma].item() > -52.0 or b.spike_counts[0, soma] > 0


# --------------------------------------------- compartment selectors --
def _neurons():
    return pd.DataFrame({
        "entity_idx": [0, 1, 2],
        "dataset_id": ["banc:0", "banc:1", "banc:2"],
        "cell_type": ["ct"] * 3,
        "flow_class": ["afferent"] * 3,
        "super_class": ["sensory"] * 3,
        "neuropil": ["np"] * 3,
        "entity_class": ["BIOLOGICAL_NEURON"] * 3})


def _split():
    # entity 0 emits via AXON, entity 1 via DENDRITE, entity 2 no split
    return pd.DataFrame({
        "pre_idx": [0, 1], "post_idx": [2, 2],
        "pre_compartment": ["AXON", "DENDRITE"],
        "post_compartment": ["DENDRITE", "DENDRITE"],
        "weight": [1.0, 1.0]})


def test_compartment_selector_filters_by_split_labels():
    idx = resolve_selector(
        HostSelector(flow_class="afferent", compartment_type="AXON",
                     max_targets=8, seed=0),
        _neurons(), "banc", direction="in", split_df=_split())
    assert idx == [0]            # only entity 0 provably emits via AXON


def test_strict_mode_rejects_dendritic_source():
    with pytest.raises(ValueError):
        resolve_selector(
            HostSelector(flow_class="afferent",
                         compartment_type="DENDRITE"),
            _neurons(), "banc", direction="in", split_df=_split(),
            strict=True)


def test_permissive_mode_warns_but_resolves():
    with pytest.warns(UserWarning):
        idx = resolve_selector(
            HostSelector(flow_class="afferent",
                         compartment_type="DENDRITE"),
            _neurons(), "banc", direction="in", split_df=_split(),
            strict=False)
    assert idx == [1]            # entity 1 has dendritic source edges
