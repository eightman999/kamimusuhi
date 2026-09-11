"""Model + representation shape/sanity tests."""

import numpy as np
import torch

from ..config import ModelConfig
from ..models import MODEL_REGISTRY, build_model
from ..representations.base import (KMeansRep, PCARep, RawRep, TorchRep,
                                    WindowRep)


def _batch(e=3, t=12, d=16):
    rng = np.random.default_rng(0)
    obs = rng.normal(size=(e, t, d)).astype(np.float32)
    act = rng.integers(4, size=(e, t))
    return obs, act


def test_all_models_forward_shapes():
    obs, act = _batch()
    mcfg = ModelConfig()
    for name in MODEL_REGISTRY:
        m = build_model(name, 16, 4, mcfg)
        pred, lat, aux = m(torch.as_tensor(obs),
                           torch.as_tensor(act, dtype=torch.long))
        assert pred.shape == obs.shape, name
        assert lat.shape[:2] == obs.shape[:2], name
        assert lat.shape[-1] == m.latent_dim, name


def test_vq_codes_valid_and_deterministic():
    obs, act = _batch()
    mcfg = ModelConfig()
    for name in ("ae_vq", "gru_vq"):
        m = build_model(name, 16, 4, mcfg).eval()
        with torch.no_grad():
            _, _, a1 = m(torch.as_tensor(obs),
                         torch.as_tensor(act, dtype=torch.long))
            _, _, a2 = m(torch.as_tensor(obs),
                         torch.as_tensor(act, dtype=torch.long))
        codes = a1["codes"]
        assert codes.min() >= 0 and codes.max() < mcfg.codebook_size
        assert (a2["codes"] == codes).all(), "VQ not deterministic in eval"


def test_reps_shapes_and_kmeans():
    obs, act = _batch(e=4)
    raw = RawRep(16)
    assert raw.encode(obs, act).shape == obs.shape
    win = WindowRep(16, window=4)
    z = win.encode(obs, act)
    assert z.shape == (4, 12, 4 * (16 + 4))
    pca = PCARep(16, n_comp=5).fit(obs, act)
    assert pca.encode(obs, act).shape == (4, 12, 5)
    km = KMeansRep(raw, k=6, seed=0).fit(obs, act)
    z = km.encode(obs, act)
    assert z.shape == (4, 12, 6)
    np.testing.assert_allclose(z.sum(-1), 1.0)
    assert km.codes(obs, act).shape == (4, 12)


def test_window_left_pad_zero():
    obs, act = _batch(e=1, t=5)
    w = WindowRep(16, window=4, use_act=False)
    z = w.encode(obs, act)[0]
    # at t=0 the window contains 3 zero-pad slots + obs[0]
    assert np.allclose(z[0, : 3 * 16], 0.0)
    assert np.allclose(z[0, -16:], obs[0, 0])
