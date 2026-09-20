"""Model plumbing tests + a tiny end-to-end training check."""
import unittest

import numpy as np
import torch

from experiments.x0.config import EnvConfig, ModelSpec
from experiments.x0.env.latent_cause import LatentCauseEnv
from experiments.x0.models import make_model
from experiments.x0.train import collate, compute_losses


def setup(seed=0, n_scenes=64):
    ec = EnvConfig(n_events=6, T=16, lag=2)
    env = LatentCauseEnv(ec.scene_params(), factors=ec.factors,
                         holdout=ec.holdout, modalities=ec.modalities,
                         seed=seed)
    scenes = env.sample_scenes(n_scenes, seed=seed + 1)
    dims = {m: env.transforms[m].out_dim for m in env.modalities}
    return env, scenes, dims


class TestShapes(unittest.TestCase):
    def test_binder(self):
        env, scenes, dims = setup()
        model = make_model(ModelSpec(kind="binder"), dims)
        for m, d in dims.items():
            x = torch.randn(5, d)
            z = model.embed(m, x)
            self.assertEqual(tuple(z.shape), (5, 32))
            self.assertTrue(torch.allclose(
                z.norm(dim=1), torch.ones(5), atol=1e-5))
        n_pairs = len(dims) * (len(dims) - 1)
        self.assertEqual(len(model.decoders), n_pairs)

    def test_bottleneck_and_ae(self):
        env, scenes, dims = setup()
        for kind in ("bottleneck", "indep_ae"):
            model = make_model(ModelSpec(kind=kind), dims)
            m = env.modalities[0]
            z = model.embed(m, torch.randn(4, dims[m]))
            self.assertEqual(tuple(z.shape), (4, 32))
            rec = model.decode_into(m, model.encoders[m](
                torch.randn(4, dims[m])))
            self.assertEqual(tuple(rec.shape), (4, dims[m]))


class TestTinyTraining(unittest.TestCase):
    def test_binder_loss_decreases(self):
        torch.manual_seed(0)
        env, scenes, dims = setup(n_scenes=128)
        spec = ModelSpec(kind="binder")
        model = make_model(spec, dims)
        opt = torch.optim.Adam(model.parameters(), lr=3e-3)
        mods = env.modalities
        toks = collate(scenes, mods)
        l0, _ = compute_losses(model, toks, spec, mods)
        for _ in range(40):
            loss, _ = compute_losses(model, toks, spec, mods)
            opt.zero_grad()
            loss.backward()
            opt.step()
        l1, _ = compute_losses(model, toks, spec, mods)
        self.assertLess(float(l1), float(l0) * 0.95)

    def test_losses_finite(self):
        torch.manual_seed(0)
        env, scenes, dims = setup(n_scenes=32)
        mods = env.modalities
        toks = collate(scenes, mods)
        for kind in ("binder", "bottleneck", "indep_ae"):
            spec = ModelSpec(kind=kind)
            model = make_model(spec, dims)
            loss, parts = compute_losses(model, toks, spec, mods)
            self.assertTrue(np.isfinite(float(loss)))
            self.assertTrue(all(np.isfinite(v) for v in parts.values()))


if __name__ == "__main__":
    unittest.main()
