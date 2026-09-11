"""X0 env tests: determinism, cause-dependence, scene structure."""
import unittest

import numpy as np

from experiments.x0.config import EnvConfig
from experiments.x0.env.latent_cause import CauseSpace, LatentCauseEnv
from experiments.x0.modalities.transforms import make_transforms


def make_env(seed=0, **kw):
    ec = EnvConfig(**{k: v for k, v in kw.items()
                      if k in EnvConfig.__dataclass_fields__})
    return LatentCauseEnv(ec.scene_params(), factors=ec.factors,
                          holdout=ec.holdout, modalities=ec.modalities,
                          seed=seed)


class TestDeterminism(unittest.TestCase):
    def test_same_seed_same_scenes(self):
        e1, e2 = make_env(7), make_env(7)
        s1 = e1.sample_scenes(4, seed=99)
        s2 = e2.sample_scenes(4, seed=99)
        for a, b in zip(s1, s2):
            for m in a.emissions:
                np.testing.assert_array_equal(a.emissions[m].times,
                                              b.emissions[m].times)
                np.testing.assert_allclose(a.emissions[m].feats,
                                           b.emissions[m].feats)

    def test_transform_determinism(self):
        tr = make_transforms(("vis", "aud"), 12, 5)
        z = np.eye(12)[:3]
        np.testing.assert_allclose(tr["vis"](z), tr["vis"](z))

    def test_alt_transform_differs(self):
        t1 = make_transforms(("vis",), 12, 5)
        t2 = make_transforms(("vis",), 12, 5, alt=True)
        z = np.eye(12)[:4]
        self.assertFalse(np.allclose(t1["vis"](z), t2["vis"](z)))


class TestCauseDependence(unittest.TestCase):
    def test_features_depend_on_cause(self):
        """Same cause -> identical noiseless features; different causes ->
        different features in every modality."""
        env = make_env(3)
        z = env.causes.codes
        for m in env.modalities:
            F = env.transforms[m](z)
            np.testing.assert_allclose(F[0], env.transforms[m](z[0:1])[0])
            self.assertGreater(np.abs(F[0] - F[1]).max(), 1e-3,
                               f"{m}: causes 0/1 indistinguishable")

    def test_within_cause_clustering(self):
        """Under read noise, same-cause emissions must be closer (in feature
        space) than random different-cause pairs, on average."""
        env = make_env(4)
        rng = np.random.default_rng(0)
        for m in ("vis", "aud"):
            f = env.transforms[m]
            same, diff = [], []
            for c in range(10):
                for _ in range(6):
                    x = f(env.causes.codes[c]) + rng.normal(
                        0, 0.1, f.out_dim)
                    x2 = f(env.causes.codes[c]) + rng.normal(
                        0, 0.1, f.out_dim)
                    y = f(env.causes.codes[rng.integers(10, 64)]) \
                        + rng.normal(0, 0.1, f.out_dim)
                    same.append(np.linalg.norm(x - x2))
                    diff.append(np.linalg.norm(x - y))
            self.assertLess(np.mean(same), np.mean(diff) * 0.5)

    def test_scene_structure(self):
        env = make_env(0)
        sc = env.sample_scene(np.random.default_rng(0))
        self.assertEqual(len(np.unique(sc.causes)),
                         len(sc.causes))          # distinct causes per scene
        for m, em in sc.emissions.items():
            self.assertLessEqual(len(em.times), env.params.n_events)
            self.assertTrue((em.times >= 0).all())

    def test_holdout_disjoint(self):
        cs = CauseSpace((4, 4, 4), (2, 2))
        self.assertEqual(len(cs.train_ids), 48)
        self.assertEqual(len(cs.holdout_ids), 16)
        # every factor value still appears in training combos
        for fi, f in enumerate(cs.factors):
            vals = set(cs.combos[cs.train_ids, fi].tolist())
            self.assertEqual(vals, set(range(f)))

    def test_holdout_pool_sampling(self):
        env = make_env(1)
        scs = env.sample_scenes(8, seed=5, cause_pool="holdout")
        for sc in scs:
            self.assertTrue(env.causes.held_out[sc.causes].all())


class TestConflictConstruction(unittest.TestCase):
    def test_conflict_means_cotimed_are_wrong(self):
        env = make_env(2)
        scs = env.sample_scenes(32, seed=1, mode="conflict",
                                token_drop_p=0.0, modality_drop_p=0.0)
        a, b = env.params.conflict_pair
        for sc in scs:
            ra, rb = sc.emissions[a], sc.emissions[b]
            # every vis token has a co-timed aud token of a DIFFERENT event
            for i in range(len(ra.times)):
                cotimed = np.where(rb.times == ra.times[i])[0]
                self.assertGreater(len(cotimed), 0)
                for j in cotimed:
                    self.assertNotEqual(int(rb.events[j]), int(ra.events[i]))


if __name__ == "__main__":
    unittest.main()
