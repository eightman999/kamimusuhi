"""Dynamics sanity: occlusion behaviour, labels, boundary absorption."""

import numpy as np

from experiments.o0.env import dynamics as dyn


def make(n=256, seed=0, **over):
    p = dyn.EnvParams().with_overrides(**over)
    return p, dyn.generate_batch(p, n, np.random.default_rng(seed))


def test_occlusion_happens_and_hides_target():
    p, d = make()
    assert (d["t_occl"] >= 0).mean() > 0.9, "almost all episodes occlude"
    occ = d["occluded"]
    # invisible iff inside & alive
    assert not (d["visible"] & occ).any()
    # while occluded the target channels are exactly zero
    assert np.abs(d["obs"][occ][:, :4]).max() == 0.0


def test_labels_consistent():
    p, d = make()
    T, B = d["exist"].shape
    t = np.arange(T)[:, None]
    # exist == 0 strictly before appearance
    pre = t < d["t_appear"][None, :]
    assert (d["exist"][pre] == 0).all()
    # visible implies exist
    assert (d["exist"][d["visible"]] == 1).all()
    # occluded implies exist
    assert (d["exist"][d["occluded"]] == 1).all()


def test_reappearance_after_occlusion():
    p, d = make(p_gone=0.0)
    occluded_eps = d["t_occl"] >= 0
    assert (d["t_reapp"][occluded_eps] > 0).mean() > 0.9
    # reappears strictly after first occlusion
    ok = d["t_reapp"] > 0
    assert (d["t_reapp"][ok] > d["t_occl"][ok]).all()


def test_gone_episodes_get_absorbed_while_hidden():
    p, d = make(seed=1)
    g = d["gone"]
    assert g.mean() > 0.1
    # most gone-geometry episodes are absorbed without reappearing
    assert d["absorbed"][g].mean() > 0.5
    # absorbed episodes: exist flips to 0 and never comes back
    idx = np.where(d["absorbed"])[0]
    for b in idx[:20]:
        ta = d["t_absorb"][b]
        assert d["exist"][ta, b] == 0
        assert d["exist"][ta:, b].sum() == 0
        assert not d["visible"][ta:, b].any()


def test_position_continuous_during_occlusion():
    # hidden position should move smoothly (no teleportation)
    p, d = make()
    dx = np.abs(np.diff(d["pos"], axis=0))
    occ = d["occluded"]
    step_pairs = occ[:-1] & occ[1:]
    assert dx[step_pairs].max() < 0.2  # |v|<=v_max => |dx|<=v_max/L=0.0625+eps


def test_occluder_bounds_constant_and_ordered():
    p, d = make()
    lo = d["obs"][..., dyn.OCC_LO_IDX]
    hi = d["obs"][..., dyn.OCC_HI_IDX]
    # static within an episode
    assert (np.diff(lo, axis=0) == 0).all()
    assert (np.diff(hi, axis=0) == 0).all()
    assert (lo < hi).all()


def test_swap_label_consistency():
    p, d = make(seed=2)
    sw = d["swapped"].astype(bool)
    assert 0.1 < sw.mean() < 0.5
    for b in np.where(sw)[0][:20]:
        tr = d["t_reapp"][b]
        assert tr > 0
        # appearance actually changed at reappearance
        app_before = None
        vis_before = d["visible"][:tr, b]
        app_before = d["obs"][:tr, b][vis_before][-1, dyn.T_APP]
        app_after = d["obs"][tr, b, dyn.T_APP]
        assert abs(app_after - app_before) > 0.2
        # same label flips to 0 at reappearance
        assert d["same"][tr, b] == 0
        assert d["same"][tr - 1, b] == 1
