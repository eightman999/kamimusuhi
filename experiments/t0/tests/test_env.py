import unittest

import torch

from experiments.t0.env.interval_env import ACT, WAIT, IntervalEnv
from experiments.t0.env.temporal_tasks import (DelayedResponseEnv,
                                               IntervalComparisonEnv,
                                               TemporalOrderEnv)


class TestIntervalEnv(unittest.TestCase):
    def make(self, n=16, **config):
        return IntervalEnv(n, "cpu", 7, config)

    def test_deterministic_replay(self):
        env_a, env_b = self.make(), self.make()
        obs_a, obs_b = env_a.reset(), env_b.reset()
        self.assertTrue(torch.equal(obs_a, obs_b))
        for _ in range(10):
            out_a = env_a.step(torch.zeros(16, dtype=torch.long))
            out_b = env_b.step(torch.zeros(16, dtype=torch.long))
            self.assertTrue(torch.equal(out_a[0], out_b[0]))
            self.assertTrue(torch.equal(out_a[1], out_b[1]))

    def test_snapshot_restore(self):
        env = self.make()
        env.reset()
        for _ in range(5):
            env.step(torch.zeros(16, dtype=torch.long))
        snap = env.snapshot()
        obs_before = env.observation.clone()
        for _ in range(5):
            env.step(torch.zeros(16, dtype=torch.long))
        env.restore(snap)
        self.assertTrue(torch.equal(env.observation, obs_before))
        fork_a, fork_b = [], []
        for _ in range(5):
            fork_a.append(env.step(torch.zeros(16, dtype=torch.long))[0])
        env.restore(snap)
        for _ in range(5):
            fork_b.append(env.step(torch.zeros(16, dtype=torch.long))[0])
        for a, b in zip(fork_a, fork_b):
            self.assertTrue(torch.equal(a, b))

    def test_act_at_target_succeeds(self):
        env = self.make(delay_distribution="grid", delays=[16])
        env.reset()
        self.assertTrue(bool((env.target_step == 16).all()))
        for t in range(20):
            action = torch.full((16,), ACT if t == 16 else WAIT, dtype=torch.long)
            _, reward, done, info = env.step(action)
            if t == 16:
                self.assertTrue(bool(info["success"].all()))
                self.assertTrue(bool((reward > .9).all()))  # 1 minus step_cost
                self.assertTrue(bool(done.all()))
                return

    def test_early_and_late_flags(self):
        env = self.make(delays=[16])
        env.reset()
        for t in range(8):
            _, _, _, info = env.step(torch.full((16,), ACT if t == 7 else WAIT))
        self.assertTrue(bool(info["early"].all()))
        self.assertFalse(bool(info["success"].any()))
        env = self.make(delays=[16])
        env.reset()
        for t in range(24):
            _, _, _, info = env.step(torch.full((16,), ACT if t == 23 else WAIT))
        self.assertTrue(bool(info["late"].all()))
        self.assertFalse(bool(info["success"].any()))

    def test_never_act_penalty(self):
        env = self.make(delays=[16], horizon=40)
        env.reset()
        reward = torch.zeros(16)
        for _ in range(40):
            _, reward, done, _ = env.step(torch.zeros(16, dtype=torch.long))
        self.assertTrue(bool((reward == -1.).all()))
        self.assertTrue(bool(done.all()))
        # never-acting is strictly worse than any committed ACT
        self.assertLess(-1. - 40 * env.step_cost, -1.)

    def test_window_tolerance(self):
        env = self.make(delays=[16], window=2)
        env.reset()
        for t in range(18):
            _, _, _, info = env.step(torch.full((16,), ACT if t == 14 else WAIT))
        self.assertTrue(bool(info["success"].all()))

    def test_time_scale_shifts_target(self):
        env = self.make(delays=[32], time_scale=2.0)
        env.reset()
        self.assertTrue(bool((env.target_step == 16).all()))
        env = self.make(delays=[32], time_scale=0.5)
        env.reset()
        self.assertTrue(bool((env.target_step == 64).all()))

    def test_cue_encodes_delay_only_during_pulse(self):
        env = self.make(delays=[64], pulse_length=2)
        obs = env.reset()
        self.assertTrue(torch.allclose(obs[:, 1], torch.full((16,), .5)))
        obs, _, _, _ = env.step(torch.zeros(16, dtype=torch.long))
        self.assertTrue(torch.allclose(obs[:, 1], torch.full((16,), .5)))
        obs, _, _, _ = env.step(torch.zeros(16, dtype=torch.long))
        self.assertTrue(bool((obs[:, 1] == 0.).all()))

    def test_delay_distributions(self):
        for dist in ("grid", "uniform", "geometric", "mixed"):
            env = self.make(64, delay_distribution=dist, delay_min=8, delay_max=64,
                            delays=[8, 16, 24, 32, 48, 64])
            env.reset()
            self.assertTrue(bool(((env.delay >= 8) & (env.delay <= 64)).all()))
        env = self.make(64, delay_distribution="grid", delays=[8, 16, 24, 32, 48, 64])
        env.reset()
        self.assertTrue(bool(set(env.delay.tolist()) <= {8, 16, 24, 32, 48, 64}))

    def test_obs_bounds(self):
        env = self.make(distractor_rate=.5)
        env.reset()
        for _ in range(20):
            obs, _, _, _ = env.step(torch.zeros(16, dtype=torch.long))
            self.assertTrue(bool(((obs >= 0) & (obs <= 1)).all()))

    def test_delay_override(self):
        env = self.make()
        env.reset(delay_override=torch.full((16,), 40))
        self.assertTrue(bool((env.delay == 40).all()))


class TestAuxTasks(unittest.TestCase):
    def test_comparison_smoke(self):
        env = IntervalComparisonEnv(16, "cpu", 3, {"horizon": 64})
        env.reset()
        total = torch.zeros(16)
        for _ in range(64):
            actions = env.oracle_actions()
            _, reward, done, info = env.step(actions)
            total += reward
        self.assertTrue(bool((info["success"]).all()))
        self.assertTrue(bool((total == 1.).all()))

    def test_order_smoke(self):
        env = TemporalOrderEnv(16, "cpu", 3, {"horizon": 32})
        env.reset()
        for _ in range(32):
            _, _, _, info = env.step(env.oracle_actions())
        self.assertTrue(bool(info["success"].all()))

    def test_delayed_response_smoke(self):
        env = DelayedResponseEnv(16, "cpu", 3, {"delays": [16], "horizon": 40})
        env.reset()
        for _ in range(40):
            _, _, _, info = env.step(env.oracle_actions())
        self.assertTrue(bool(info["success"].all()))


if __name__ == "__main__":
    unittest.main()
