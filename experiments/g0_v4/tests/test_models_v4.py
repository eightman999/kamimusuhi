"""v4 model shape/sanity tests."""

from pathlib import Path

import numpy as np
import torch

from ..config import load_config
from ..models import MODEL_REGISTRY_V4, build_model, compute_loss


def _cfg():
    return load_config(Path(__file__).parent.parent
                       / "configs/smoke.yaml")


def _batch(e=3, t=16, d=16):
    rng = np.random.default_rng(0)
    obs = rng.normal(size=(e, t, d)).astype(np.float32)
    nxt = rng.normal(size=(e, t, d)).astype(np.float32)
    act = rng.integers(4, size=(e, t))
    return (torch.as_tensor(obs), torch.as_tensor(nxt),
            torch.as_tensor(act, dtype=torch.long))


def test_all_methods_forward_shapes():
    cfg = _cfg()
    obs, nxt, act = _batch()
    for name in MODEL_REGISTRY_V4:
        m = build_model(name, cfg, 16, 4)
        pred, lat, aux = m(obs, act)
        assert pred.shape == obs.shape, name
        assert lat.shape == obs.shape[:2] + (m.latent_dim,), name
        # intervention metric plumbing: head must map latent -> obs
        assert m.head.in_features == m.latent_dim, name
        assert m.head.out_features == 16, name


def test_all_methods_loss_finite():
    cfg = _cfg()
    obs, nxt, act = _batch()
    gen = torch.Generator().manual_seed(0)
    for name in MODEL_REGISTRY_V4:
        m = build_model(name, cfg, 16, 4)
        loss, logs = compute_loss(m, name, obs, nxt, act, cfg, gen)
        assert loss.ndim == 0 and torch.isfinite(loss), name
        loss.backward()  # gradients must flow
        gnorm = sum(float(p.grad.abs().sum()) for p in m.parameters()
                    if p.grad is not None)
        assert gnorm > 0, name


def test_jepa_ema_update_moves_target():
    cfg = _cfg()
    m = build_model("jepa", cfg, 16, 4)
    obs, nxt, act = _batch()
    p_t0 = next(m.target_gru.parameters()).detach().clone()
    loss, _ = m.compute_loss(obs, nxt, act, cfg, None)
    loss.backward()
    with torch.no_grad():
        for p in m.gru.parameters():
            p.add_(0.01)          # pretend an optimizer step
    m.ema_update()
    p_t1 = next(m.target_gru.parameters()).detach()
    assert not torch.allclose(p_t0, p_t1)
    # target encoder must stay no-grad
    assert all(not p.requires_grad
               for p in m.target_gru.parameters())


def test_untrained_twin_identical_init():
    """Untrained twin must be the same arch with the same init seed."""
    cfg = _cfg()
    torch.manual_seed(7)
    m1 = build_model("cpc", cfg, 16, 4)
    torch.manual_seed(7)
    m2 = build_model("cpc", cfg, 16, 4)
    for a, b in zip(m1.state_dict().values(), m2.state_dict().values()):
        assert torch.equal(a, b)
