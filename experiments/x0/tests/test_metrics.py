"""Metric correctness tests: oracle/chance/timing behave as expected on
synthetic ground truth — the numbers that anchor the env-validation table."""
import unittest

import numpy as np

from experiments.x0.analysis.metrics import (false_binding, matching_accuracy,
                                             retrieval)
from experiments.x0.config import EnvConfig
from experiments.x0.env.latent_cause import LatentCauseEnv
from experiments.x0.models.baselines import (ChanceMethod, TimingMethod)


def make_env(seed=0):
    ec = EnvConfig()
    return LatentCauseEnv(ec.scene_params(), factors=ec.factors,
                          holdout=ec.holdout, modalities=ec.modalities,
                          seed=seed)


class TestRetrievalMath(unittest.TestCase):
    def test_toy_retrieval(self):
        sim = np.array([[0.9, 0.1, 0.0],
                        [0.2, 0.3, 0.8]])          # q1 true=j2 at rank0
        partner = np.array([0, 2])
        r = retrieval(sim, partner)
        self.assertEqual(r["top1"], 1.0)
        self.assertAlmostEqual(r["mrr"], 1.0)

    def test_retrieval_second_rank(self):
        sim = np.array([[0.8, 0.9, 0.2]])          # partner at rank 1
        r = retrieval(sim, np.array([0]))
        self.assertEqual(r["top1"], 0.0)
        self.assertAlmostEqual(r["mrr"], 0.5)

    def test_matching_identity(self):
        sim = np.eye(4) + 0.01 * np.random.default_rng(0).random((4, 4))
        np.fill_diagonal(sim, 1.0)
        ev = np.arange(4)
        self.assertEqual(matching_accuracy(sim, ev, ev), 1.0)

    def test_matching_wrong_assignment(self):
        # identity sim -> assignment picks j=i; b-token j has event perm[j]
        # (a derangement) -> every assignment is wrong
        sim = np.eye(4)
        ev_a = np.arange(4)
        ev_b = np.array([1, 2, 3, 0])             # derangement
        self.assertEqual(matching_accuracy(sim, ev_a, ev_b), 0.0)


class TestHeuristicBehaviour(unittest.TestCase):
    def test_timing_fails_conflict(self):
        """On conflict scenes the co-timed candidate is ALWAYS a different
        cause: the timing heuristic must false-bind ~always."""
        env = make_env(5)
        scs = env.sample_scenes(64, seed=0, mode="conflict",
                                token_drop_p=0.0, modality_drop_p=0.0)
        timing = TimingMethod()
        wrong = n = 0
        for sc in scs:
            ra, rb = sc.emissions["vis"], sc.emissions["aud"]
            sim = timing.pair_sim("vis", {"times": ra.times, "feats": ra.feats},
                                  "aud", {"times": rb.times, "feats": rb.feats})
            partner = sc.partner_index("vis", "aud")
            fb = false_binding(sim, ra.times, rb.times, ra.events, rb.events,
                               partner)
            wrong += fb["cotimed_wrong"] * fb["n"]
            n += fb["n"]
        self.assertGreater(wrong / n, 0.9)

    def test_chance_floor(self):
        env = make_env(6)
        scs = env.sample_scenes(64, seed=0, token_drop_p=0.0,
                                modality_drop_p=0.0)
        ch = ChanceMethod(0)
        hits = n = 0
        for sc in scs:
            ra, rb = sc.emissions["vis"], sc.emissions["aud"]
            sim = ch.pair_sim("vis", {"times": ra.times, "feats": ra.feats},
                              "aud", {"times": rb.times, "feats": rb.feats})
            partner = sc.partner_index("vis", "aud")
            r = retrieval(sim, partner)
            hits += r["top1"] * r["n"]
            n += r["n"]
        self.assertLess(hits / n, 0.35)   # ~1/6 expected, margin for noise


if __name__ == "__main__":
    unittest.main()
