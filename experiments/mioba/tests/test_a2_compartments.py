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


# --------------------------------------------------------- A2.1 ------
def _soma_not_root_skeleton():
    """§29 fixture: soma is NOT the root — a branch above the soma and
    dendrites below it must ALL receive path distances."""
    return _swc([
        [1, 3, 0, -30, 0, 1, -1],   # root: dendrite ABOVE soma (dist 30)
        [2, 3, 0, -60, 0, 1, 1],    # far parent-side dendrite (dist 60)
        [3, 4, 0, -10, 0, 1, 1],    # primary neurite above soma
        [4, 7, 0, 0, 0, 2, 1],      # soma (child of node 1!)
        [5, 3, 0, 5, 0, 1, 4],      # dendrite below soma
        [6, 3, 0, 45, 0, 1, 5],     # distal dendrite below
        [7, 2, 5, 0, 0, 1, 4],      # axon
    ])


def test_soma_not_root_gets_distances_everywhere():
    """§27–28: undirected distance_from_soma — nodes on the parent side
    of a non-root soma must not be dropped into dist=0."""
    red = C.reduce_entity(3, _soma_not_root_skeleton())
    assert set(red.node_map) == {1, 2, 3, 4, 5, 6, 7}
    # node 2 is 60 nm above the soma through node 1 — it is a DISTAL
    # dendrite, not proximal. The buggy children-only walk left it at
    # dist 0 (PROX); the undirected walk must classify it DIST.
    assert red.node_map[2] == "DENDRITE_DIST"
    assert red.node_map[6] == "DENDRITE_DIST"
    assert red.node_map[5] == "DENDRITE_PROX"
    assert red.node_map[4] == "SOMA" and red.node_map[7] == "AXON"


def test_unknown_nodes_fallback_is_counted():
    """§30–31: UNKNOWN-labelled SWC nodes fall back to DENDRITE_PROX
    and that fallback is counted, never claimed as real."""
    nodes = _labeled_skeleton().tolist()
    nodes.append([7, 0, 0, 9, 9, 1, 5])      # UNKNOWN-labelled node
    red = C.reduce_entity(5, _swc(nodes))
    assert red.unknown_swc_nodes == 1
    assert red.unknown_nodes_fallback_mapped == 1
    assert red.node_map[7] == "DENDRITE_PROX"


def _split_df():
    """One entity pair with THREE compartment placements (§2 fixture):
    they must stay separate runtime edges."""
    return pd.DataFrame({
        "pre_idx": [0, 0, 0],
        "post_idx": [1, 1, 1],
        "pre_compartment": ["AXON", "AXON", "AXON"],
        "post_compartment": ["DENDRITE", "SOMA", "AXON"],
        "anatomical_count": [7, 2, 1]})


def test_v2_keeps_compartment_edges_separate():
    reds = {0: C.reduce_entity(0, _labeled_skeleton()),
            1: C.reduce_entity(1, _labeled_skeleton())}
    (n, post, pre, w, coup, rows, man, audit) = \
        C.compile_reduced_graph_v2(_split_df(), reds)
    assert len(w) == 3                     # no dominant collapse (§2)
    # weight = count/32 per split edge (§6)
    assert sorted(w.tolist()) == sorted([7 / 32, 2 / 32, 1 / 32])
    # anatomical record fields present (§5)
    for col in ("pre_entity", "post_entity", "pre_compartment",
                "post_compartment", "anatomical_count",
                "runtime_weight", "weight_provenance",
                "fallback_used"):
        assert col in audit.columns
    assert man["algorithm"] == "reduce-v1-split-synapse"
    assert man["split_edges_total"] == 3
    assert man["split_edges_exact_compartment"] == 3
    # §1: PRIMARY_NEURITE folding is declared in the manifest
    assert "PRIMARY_NEURITE" in man["compartment_reduction"]


def test_v2_unknown_fallback_audited():
    df = _split_df()
    df.loc[2, "post_compartment"] = "UNKNOWN"
    reds = {0: C.reduce_entity(0, _labeled_skeleton()),
            1: C.reduce_entity(1, _labeled_skeleton())}
    _, _, _, w, _, _, man, audit = \
        C.compile_reduced_graph_v2(df, reds, mapping_mode="permissive")
    assert len(w) == 3                     # permissive keeps the edge
    assert man["split_edges_unknown_compartment"] == 1
    assert man["split_edges_fallback_to_soma"] == 1
    fb = audit[audit["fallback_used"]]
    assert fb["fallback_reason"].iloc[0] == "UNKNOWN_POST_COMPARTMENT"
    assert fb["mapping_provenance"].iloc[0] == "MODEL_INFERENCE"


def test_v2_strict_skips_unknown():
    df = _split_df()
    df.loc[2, "post_compartment"] = "UNKNOWN"
    reds = {0: C.reduce_entity(0, _labeled_skeleton()),
            1: C.reduce_entity(1, _labeled_skeleton())}
    _, _, _, w, _, _, man, _ = \
        C.compile_reduced_graph_v2(df, reds, mapping_mode="strict")
    assert len(w) == 2                     # strict drops it (§11)
    assert man["split_edges_skipped_strict"] == 1


def test_telemetry_counts_artificial_events():
    """§19: per-node event count / synaptic input / voltage telemetry."""
    b = TorchBackend(synthetic=False,
                     base_override=(5, np.array([1]), np.array([2]),
                                    np.array([1.0])))
    phen = {"artificial_organs": [
                {"organ_id": "graft:g001", "size": 2, "kind": "graft",
                 "internal_p": 0.0, "neuron_model": "lif"}],
            "attachments": [
                {"attachment_id": "g:out", "direction": "forward",
                 "weight_scale": 50.0, "p": 1.0, "source": "graft:g001",
                 "target": "host:g:out", "target_idx": [0],
                 "connection_provenance": "ARTIFICIAL_GRAFT"}],
            "n_extra_neurons": 2, "params": {}}
    b.initialize(phen, batch_size=1, seed=1, device="cpu",
                 replicate_seeds=[1])
    b.set_telemetry([0])
    b.force_spikes([5, 6])
    b.run(30.0)
    tel = b.get_telemetry()
    assert tel and tel["nodes"][0]["input_event_count"][0] > 0
    assert tel["nodes"][0]["synaptic_input_sum"][0] > 0
    assert tel["nodes"][0]["membrane_voltage_peak"][0] > -52.0


def test_matched_targets_require_both_compartments():
    """§13: matched set = entities provably having SOMA *and*
    DENDRITE_DIST — the only legal B-vs-C comparison set."""
    from experiments.mioba.scripts.g01_compartment_graft import (
        matched_targets)
    rows = [
        {"runtime_idx": 0, "entity_idx": 10, "compartment": "SOMA"},
        {"runtime_idx": 1, "entity_idx": 10,
         "compartment": "DENDRITE_DIST"},
        {"runtime_idx": 2, "entity_idx": 11, "compartment": "SOMA"},
        {"runtime_idx": 3, "entity_idx": 11,
         "compartment": "DENDRITE_PROX"},
        {"runtime_idx": 4, "entity_idx": 12, "compartment": "SOMA"},
    ]
    matched, comp_of = matched_targets(rows, [10, 11, 12])
    assert matched == [10]                 # 11 lacks DIST, 12 lacks both
    assert comp_of[10]["DENDRITE_DIST"] == 1
