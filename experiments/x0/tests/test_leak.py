"""X0 leak tests: neither timestamps nor token indices may uniquely
identify cross-modal pairs, and the observation must carry no cause/event
label fields.

These tests pin down the anti-shortcut properties the spec demands:
  * times:  co-timed cross-modal tokens are OFTEN different causes, and
            true partners are often NOT co-timed -> a pure clock fails.
  * index:  each modality's observed order is independently shuffled, so
            position in one modality's list carries no pairing signal.
  * labels: observation dicts contain only "times" and "feats".
"""
import unittest
from collections import defaultdict

import numpy as np

from experiments.x0.env.latent_cause import LatentCauseEnv
from experiments.x0.config import EnvConfig


def make_env(seed=0):
    ec = EnvConfig()
    return LatentCauseEnv(ec.scene_params(), factors=ec.factors,
                          holdout=ec.holdout, modalities=ec.modalities,
                          seed=seed)


class TestNoLabelLeak(unittest.TestCase):
    def test_observation_fields(self):
        env = make_env(0)
        rng = np.random.default_rng(0)
        for sc in env.sample_scenes(8, seed=3):
            obs = sc.observation(rng)
            for m, d in obs.items():
                self.assertEqual(set(d.keys()), {"times", "feats"})
                self.assertEqual(d["times"].dtype, np.int64)
                self.assertEqual(d["feats"].dtype, np.float64)

    def test_no_scalar_id_channel(self):
        """Feature vectors must be d_m-dimensional continuous transforms —
        never a 1-D scalar that could encode the cause index."""
        env = make_env(0)
        for sc in env.sample_scenes(8, seed=4):
            for m, em in sc.emissions.items():
                self.assertEqual(em.feats.shape[1],
                                 env.transforms[m].out_dim)
                self.assertGreater(env.transforms[m].out_dim, 3)


class TestNoTimingShortcut(unittest.TestCase):
    def test_cotimed_tokens_often_different_cause(self):
        """In standard scenes, a co-timed cross-modal token is a different
        cause with non-trivial frequency (timing can't identify pairs)."""
        env = make_env(1)
        n_cotimed = n_cotimed_wrong = n_queries = 0
        n_shifted_partner = 0
        for sc in env.sample_scenes(400, seed=11, token_drop_p=0.0,
                                    modality_drop_p=0.0):
            ra, rb = sc.emissions["vis"], sc.emissions["aud"]
            partner = sc.partner_index("vis", "aud")
            for i in range(len(ra.times)):
                if partner[i] < 0:
                    continue
                n_queries += 1
                dt_true = abs(int(ra.times[i]) - int(rb.times[partner[i]]))
                if dt_true > 0:
                    n_shifted_partner += 1
                cotimed = [j for j in range(len(rb.times))
                           if rb.times[j] == ra.times[i]]
                for j in cotimed:
                    n_cotimed += 1
                    if rb.events[j] != ra.events[i]:
                        n_cotimed_wrong += 1
        self.assertGreater(n_cotimed, 0)
        frac_wrong = n_cotimed_wrong / n_cotimed
        self.assertGreater(frac_wrong, 0.3,
                           "co-timed tokens are almost always true pairs")
        self.assertGreater(n_shifted_partner / n_queries, 0.2,
                           "true partners are almost always synchronous")

    def test_index_order_carries_no_pairing(self):
        """After observation shuffling, a token's position in modality A's
        list is statistically independent of its partner's position in B."""
        env = make_env(2)
        rng = np.random.default_rng(7)
        pos_a, pos_b = [], []
        for sc in env.sample_scenes(300, seed=12, token_drop_p=0.0,
                                    modality_drop_p=0.0):
            obs = sc.observation(rng)
            # recover positions of each event's token inside the shuffled obs
            # (uses ground truth: events still stored on the scene object)
            ra = sc.emissions["vis"]
            rb = sc.emissions["aud"]
            # recover each emission row's position in the shuffled obs order
            order_a = [np.where((obs["vis"]["feats"] == f).all(1))[0][0]
                       for f in ra.feats]
            order_b = [np.where((obs["aud"]["feats"] == f).all(1))[0][0]
                       for f in rb.feats]
            ev_to_bpos = {int(rb.events[j]): order_b[j]
                          for j in range(len(rb.times))}
            for i in range(len(ra.times)):
                pos_a.append(order_a[i])
                pos_b.append(ev_to_bpos[int(ra.events[i])])
        r = np.corrcoef(pos_a, pos_b)[0, 1]
        self.assertLess(abs(r), 0.1)

    def test_times_do_not_determine_partner(self):
        """Across scenes, conditioning on (t_a, t_b) must leave the cause
        pairing ambiguous: same (dt) -> different pairings."""
        env = make_env(3)
        pairings = defaultdict(set)
        for sc in env.sample_scenes(200, seed=13, token_drop_p=0.0,
                                    modality_drop_p=0.0):
            ra, rb = sc.emissions["vis"], sc.emissions["aud"]
            partner = sc.partner_index("vis", "aud")
            for i in range(len(ra.times)):
                if partner[i] < 0:
                    continue
                dt = int(rb.times[partner[i]]) - int(ra.times[i])
                # cause pair (a-cause, b-cause) is trivially same event;
                # what must vary is WHICH aud token sits at each dt
                pairings[dt].add(int(partner[i]))
        # at dt values observed, the partner index takes multiple values
        multi = sum(1 for v in pairings.values() if len(v) > 1)
        self.assertGreater(multi, 2)


if __name__ == "__main__":
    unittest.main()
