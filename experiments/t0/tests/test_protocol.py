"""T0-v2 protocol tests: holdout exclusion, split integrity, and
intervention target-independence."""
import unittest

import torch

from experiments.t0.analysis.protocol import (ProtocolViolation,
                                              intervention_target_independence,
                                              require_clean, training_support,
                                              validate_split_integrity)
from experiments.t0.env.interval_env import INTERP_HOLDOUT, IntervalEnv
from experiments.t0.evaluate import freeze_dynamics, post_cue_blank
from experiments.t0.train import collect

V2_ENV = {"delay_distribution": "mixed",
          "delays": [8, 16, 24, 32, 48, 64], "delay_min": 8, "delay_max": 64,
          "excluded_training_delays": list(INTERP_HOLDOUT), "horizon": 96}
V2_CONFIG = {"task": "interval", "seed": 0, "env": V2_ENV}
HOLDOUT = set(INTERP_HOLDOUT)


class Waiter(torch.nn.Module):
    """Never acts; keeps every episode alive to the horizon."""
    num_actions = 3

    def initial_state(self, b, d):
        return torch.ones(b, 8, device=d)

    def forward(self, o, s):
        logits = torch.zeros(o.shape[0], 3)
        logits[:, 0] = 10.
        return logits, torch.zeros(o.shape[0]), s + 1


class TestHoldoutSampling(unittest.TestCase):
    def test_training_never_samples_interpolation_holdout(self):
        for dist in ("uniform", "mixed", "geometric"):
            env = IntervalEnv(256, "cpu", 1, dict(V2_ENV, delay_distribution=dist))
            sampled = set()
            for _ in range(30):
                env.reset()
                sampled.update(env.delay.tolist())
            self.assertFalse(sampled & HOLDOUT, f"{dist} sampled holdout")
            if dist in ("uniform", "mixed"):
                # band-adjacent allowed delays are still reachable
                self.assertTrue(sampled & {43, 44, 53, 59})

    def test_eval_can_force_heldout_delay(self):
        env = IntervalEnv(16, "cpu", 1, dict(V2_ENV, delay_distribution="uniform"))
        env.reset(delay_override=torch.full((16,), 40))
        self.assertTrue(bool((env.delay == 40).all()))
        self.assertTrue(bool((env.target_step == 40).all()))
        env.reset(delay_override=torch.full((16,), 56))
        self.assertTrue(bool((env.delay == 56).all()))

    def test_grid_overlap_is_config_error(self):
        for dist in ("grid", "mixed"):
            env = IntervalEnv(8, "cpu", 1, dict(
                V2_ENV, delay_distribution=dist, delays=[8, 40, 64]))
            with self.assertRaises(ValueError):
                env.reset()

    def test_training_support_respects_exclusion(self):
        support = training_support(V2_ENV)
        self.assertFalse(support & HOLDOUT)
        self.assertIn(43, support)
        contaminated = training_support(dict(V2_ENV, excluded_training_delays=[]))
        self.assertTrue(contaminated & HOLDOUT)


class TestProtocolValidator(unittest.TestCase):
    def test_protocol_validator_accepts_clean_split(self):
        out = validate_split_integrity(V2_CONFIG)
        self.assertEqual(out["status"], "PASS")
        for key in ("train_interpolation_overlap", "train_holdout_overlap",
                    "train_extrapolation_overlap", "validation_test_overlap"):
            self.assertEqual(out[key], [])
        self.assertTrue(out["rng_separated"])
        self.assertEqual(out["excluded_training_delays"], sorted(INTERP_HOLDOUT))

    def test_protocol_validator_rejects_overlap(self):
        v1_env = dict(V2_ENV, excluded_training_delays=[])
        out = validate_split_integrity({"task": "interval", "seed": 0,
                                        "env": v1_env})
        self.assertEqual(out["status"], "INVALID_PROTOCOL")
        self.assertEqual(out["train_interpolation_overlap"], [40, 56])
        with self.assertRaises(ProtocolViolation):
            require_clean(out)
        grid_bad = {"task": "interval", "seed": 0,
                    "env": dict(V2_ENV, delay_distribution="grid",
                                delays=[8, 40, 64])}
        self.assertEqual(validate_split_integrity(grid_bad)["status"],
                         "INVALID_PROTOCOL")


class TestPostCueBlank(unittest.TestCase):
    def _observed(self, intervention_fn, delay=32, horizon=48):
        env = IntervalEnv(8, "cpu", 5, dict(
            V2_ENV, delay_distribution="grid", delays=[delay],
            horizon=horizon))
        env.reset()
        seen = []
        fn = intervention_fn()

        def wrap(t, state, observation, _env=env):
            state, obs = fn(t, state, observation, _env)
            seen.append((env.t, obs.clone()))
            return state, obs

        collect(Waiter(), env, greedy=True, intervention=wrap)
        return env, seen

    def test_post_cue_blank_preserves_initial_cue(self):
        env, seen = self._observed(post_cue_blank)
        obs0, obs1 = seen[0][1], seen[1][1]
        self.assertTrue(bool((obs0[:, 0] == 1.).all()))
        self.assertTrue(bool((obs0[:, 1] > 0.).all()))   # delay cue intact
        self.assertTrue(bool((obs1[:, 0] == 1.).all()))
        for t, obs in seen[env.pulse_length:]:
            self.assertTrue(bool((obs == .5).all()), f"t={t} not blanked")

    def test_post_cue_blank_does_not_release_at_target(self):
        env, seen = self._observed(post_cue_blank)
        t_star = int(env.target_step[0])
        around = {t: obs for t, obs in seen}
        for t in (t_star - 1, t_star, t_star + 1):
            self.assertTrue(bool((around[t] == .5).all()),
                            f"blank released/changed at t={t}")

    def test_freeze_dynamics_does_not_release_at_target(self):
        env, seen = self._observed(freeze_dynamics)
        t_star = int(env.target_step[0])
        frozen = seen[env.pulse_length][1]
        around = {t: obs for t, obs in seen}
        for t in range(env.pulse_length + 1, min(t_star + 3, env.horizon)):
            self.assertTrue(torch.equal(around[t], frozen),
                            f"frozen obs changed at t={t}")

    def test_freeze_is_per_episode(self):
        env, seen = self._observed(freeze_dynamics)
        frozen = seen[env.pulse_length][1]
        self.assertGreater(float(frozen.std(dim=0).max()), 1e-3,
                           "all episodes froze to the same observation")
        late = seen[env.pulse_length + 5][1]
        self.assertTrue(torch.equal(late, frozen))


class TestTargetIndependence(unittest.TestCase):
    def test_v2_observation_interventions_are_target_independent(self):
        self.assertTrue(intervention_target_independence(post_cue_blank()))
        self.assertTrue(intervention_target_independence(freeze_dynamics()))

    def test_target_reading_intervention_fails(self):
        def bad(t, state, obs, env):
            if env.t >= int(env.target_step[0]):
                obs = torch.zeros_like(obs)
            return state, obs
        self.assertFalse(intervention_target_independence(bad))


if __name__ == "__main__":
    unittest.main()
