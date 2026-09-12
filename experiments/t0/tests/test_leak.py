"""Spec section 1 leak tests: the observation must not carry the current
timestep, absolute time, remaining time, normalized progress, or a
deterministic position encoding derived from environment time.

A channel is a position-encoding leak iff it is (a) deterministic across
episodes at a fixed step AND (b) varying across steps.  Channels constant in
both axes (e.g. the stimulus flag being 0 during the delay) carry no time
information; channels stochastic across episodes at every step (the
oscillator/OU dynamics) carry state-change information, not a clock.
"""
import unittest

import torch

from experiments.t0.env.interval_env import OBS_DIM, IntervalEnv


def delay_period_obs(seed, n=64, delays=(32,), steps=24, **config):
    env = IntervalEnv(n, "cpu", seed, dict(delays=list(delays), **config))
    obs = [env.reset()]
    for _ in range(steps):
        obs.append(env.step(torch.zeros(n, dtype=torch.long))[0])
    return torch.stack(obs)  # [T, N, C]


class TestNoTimeLeak(unittest.TestCase):
    def test_no_deterministic_time_varying_channel(self):
        eps = 1e-4
        for seed in (11, 22):
            obs = delay_period_obs(seed)
            delay_obs = obs[2:]                      # after the stimulus pulse
            per_t_std = delay_obs.std(dim=1)         # across episodes: [T, C]
            per_t_mean = delay_obs.mean(dim=1)
            t_variation = per_t_mean.std(dim=0)      # across steps: [C]
            deterministic = per_t_std.max(0).values < eps
            time_varying = t_variation > eps
            self.assertFalse(bool((deterministic & time_varying).any()),
                             "channel is a deterministic function of t")

    def test_no_linear_time_ramp(self):
        obs = delay_period_obs(5, delays=(16, 32, 48))
        delay_obs = obs[2:]
        t_len, n, c = delay_obs.shape
        t = torch.arange(t_len).float().unsqueeze(1).expand(t_len, n).reshape(-1)
        t = (t - t.mean()) / t.std()
        for channel in range(c):
            y = delay_obs[:, :, channel].reshape(-1)
            if y.std() < 1e-6:
                continue
            r2 = (torch.corrcoef(torch.stack([t, y]))[0, 1]) ** 2
            self.assertLess(float(r2), .5,
                            f"channel {channel} linearly tracks elapsed steps")

    def test_stimulus_pulse_only_at_start(self):
        obs = delay_period_obs(9, delays=(64,), steps=30)
        self.assertTrue(bool((obs[0, :, 0] == 1.).all()))
        self.assertTrue(bool((obs[2:, :, 0] == 0.).all()))

    def test_cue_gone_after_pulse(self):
        obs = delay_period_obs(9, delays=(64,), steps=10)
        self.assertTrue(bool((obs[2:, :, 1] == 0.).all()))

    def test_observation_carries_state_not_clock(self):
        """Dynamics channels differ across episodes at every step: a single
        observation cannot be decoded into an absolute step index."""
        obs = delay_period_obs(13, delays=(32,), steps=16)
        for channel in (2, 3):
            across_ep = obs[2:, :, channel].std(dim=1)   # over episodes per step
            self.assertGreater(float(across_ep.min()), .01,
                               f"channel {channel} looks like a shared clock")

    def test_same_delay_same_cue(self):
        env = IntervalEnv(32, "cpu", 4, {"delays": [48]})
        obs = env.reset()
        self.assertTrue(torch.allclose(obs[:, 1], torch.full((32,), 48 / 128.)))

    def test_target_step_not_in_observation(self):
        """Remaining time must never be readable from a single observation:
        two envs with different delays share identical obs statistics once
        the pulse has ended."""
        env_a = IntervalEnv(64, "cpu", 21, {"delays": [16]})
        env_b = IntervalEnv(64, "cpu", 21, {"delays": [16]})
        obs_a = env_a.reset(delay_override=torch.full((64,), 16))
        obs_b = env_b.reset(delay_override=torch.full((64,), 64))
        self.assertFalse(torch.equal(obs_a[:, 1], obs_b[:, 1]))  # cue differs
        for _ in range(4):
            obs_a = env_a.step(torch.zeros(64, dtype=torch.long))[0]
            obs_b = env_b.step(torch.zeros(64, dtype=torch.long))[0]
        # after the pulse the exogenous streams are identical draws
        self.assertTrue(torch.equal(obs_a, obs_b))


if __name__ == "__main__":
    unittest.main()
