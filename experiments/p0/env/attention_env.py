"""P0 multi-channel attention environment.

Minimal Gym-like API (no gym dependency):

    env = AttentionEnv(params, seed=0)
    obs = env.reset()
    obs, reward, done, info = env.step(action)

The agent cannot fully observe the world each step. Actions:

    IGNORE       (free, observes nothing)
    OBSERVE_A .. OBSERVE_D  (cost_observe each; reveal that channel's value)
    OBSERVE_ALL  (cost_observe_all; reveals every channel)

`obs` returned to the agent is the *belief state* the env maintains,
concat(last_observed_value[4], staleness_norm[4], prev_value[4],
gap_norm[4]) (float64, (16,)): `last_observed_value` is the noisy read made
at observation time (channel prior mean if never observed),
`staleness_norm` = steps-since-last-obs / STALENESS_CAP, `prev_value` the
read before that, and `gap_norm` the staleness that preceded the last read
(so a velocity estimate is recoverable). Unobserved channels stay stale --
the current true value is NEVER revealed unless observed this step.

The predictor/surprise features (per-channel prediction error and expected
error) are appended by the agent harness, not the env, so they can be
ablated in causal tests without touching the world.

Reward = detect_bonus*(new detection) - miss_penalty*(event expired
undetected) - observation cost. info carries ground truth for analysis
(true x, latent D phase, detection/miss flags) and is never agent-visible.
"""

from __future__ import annotations

from typing import Optional

import numpy as np

from . import dynamics as dyn


class AttentionEnv:
    def __init__(self, params: Optional[dyn.EnvParams] = None, seed: int = 0):
        self.params = params or dyn.EnvParams()
        self.seed_value = seed
        self.rng = np.random.default_rng(seed)
        self.t = 0
        self.x = np.zeros(dyn.N_CHANNELS)                 # true world state
        self.last_val = np.zeros(dyn.N_CHANNELS)          # belief: last obs
        self.staleness = np.zeros(dyn.N_CHANNELS)         # steps since obs
        self.prev_val = np.zeros(dyn.N_CHANNELS)          # belief: obs before last
        self.gap = np.zeros(dyn.N_CHANNELS)               # staleness before last obs
        self.b_phase = 0.0
        self.d = dyn.DState()
        self.done = True

    # ------------------------------------------------------------------
    def reset(self) -> np.ndarray:
        self.rng = np.random.default_rng(self.seed_value)
        self.t = 0
        self.x = dyn.initial_channels(self.rng, self.params)
        self.b_phase = float(self.rng.uniform(0.0, 2.0 * np.pi))
        self.d = dyn.DState()
        # beliefs start at channel priors, maximally stale
        self.last_val = dyn.channel_priors(self.params)
        self.staleness = np.full(dyn.N_CHANNELS, dyn.STALENESS_CAP)
        self.prev_val = dyn.channel_priors(self.params)
        self.gap = np.full(dyn.N_CHANNELS, dyn.STALENESS_CAP)
        self.done = False
        return self._obs()

    def reseed(self, seed: int) -> np.ndarray:
        """Reset with a new seed (used between episodes in rollouts)."""
        self.seed_value = seed
        return self.reset()

    # ------------------------------------------------------------------
    def _obs(self) -> np.ndarray:
        return np.concatenate(
            [
                self.last_val,
                self.staleness / dyn.STALENESS_CAP,
                self.prev_val,
                self.gap / dyn.STALENESS_CAP,
            ]
        ).astype(np.float64)

    @property
    def obs_dim(self) -> int:
        return 4 * dyn.N_CHANNELS

    def step(self, action: int):
        if self.done:
            raise RuntimeError("step() called on a finished episode; call reset()")
        p = self.params
        action = int(action)
        if action < 0 or action >= dyn.N_ACTIONS:
            raise ValueError(f"unknown action {action}")
        observed = dyn.ACTION_CHANNELS[action]

        # world advances; D FSM handles detection bookkeeping
        self.x, detected, missed = dyn.update_channels(
            self.rng, self.x, self.t, self.b_phase, self.d, p,
            d_observed=(dyn.CH_D in observed),
        )
        self.t += 1

        # belief update: observed channels get a noisy fresh read
        self.staleness = np.minimum(self.staleness + 1.0, dyn.STALENESS_CAP)
        for ch in observed:
            self.prev_val[ch] = self.last_val[ch]
            self.gap[ch] = self.staleness[ch]
            self.last_val[ch] = self.x[ch] + self.rng.normal(0.0, p.obs_noise)
            self.last_val[ch] = float(np.clip(self.last_val[ch], 0.0, 1.0))
            self.staleness[ch] = 0.0

        reward = -dyn.action_cost(action, p)
        if detected:
            reward += p.detect_bonus
        if missed:
            reward -= p.miss_penalty
        if self.t >= p.episode_length:
            self.done = True

        info = {
            "t": self.t,
            "x": self.x.copy(),               # ground truth (analysis only)
            "d_phase": self.d.phase,
            "event_active": self.d.phase == dyn.D_EVENT,
            "detected": detected,
            "missed": missed,
            "cost": dyn.action_cost(action, p),
            "observed_channels": observed,
        }
        return self._obs(), float(reward), self.done, info

    # ------------------------------------------------------------------
    # Causal-test helpers: operate on the *agent input* (belief+surprise
    # vector assembled by the harness), never on the env state.
    #
    # Agent-input layout (see agents/harness.py): per channel i a block
    #   [v_i, s_i, pv_i, g_i, e_i, d_i]
    # (belief value, staleness, previous value, obs gap, EMA pred-error,
    #  predicted surprise). Env supplies the first four per channel; the
    # harness appends e_i and d_i from the predictor.
    # ------------------------------------------------------------------
    FEATS_PER_CHANNEL = 6
    V, S, PV, G, E, D = range(6)  # offsets inside a channel block

    @staticmethod
    def shuffle_surprise(obs: np.ndarray, perm: np.ndarray) -> np.ndarray:
        """C1: permute the (e, d) surprise entries across channels."""
        out = obs.copy()
        n = dyn.N_CHANNELS
        f = AttentionEnv.FEATS_PER_CHANNEL
        for i in range(n):
            j = int(perm[i])
            out[i * f + AttentionEnv.E] = obs[j * f + AttentionEnv.E]
            out[i * f + AttentionEnv.D] = obs[j * f + AttentionEnv.D]
        return out

    @staticmethod
    def mask_surprise(obs: np.ndarray) -> np.ndarray:
        """C2: zero all prediction-error features. Works on (24,) or (n, 24)."""
        out = obs.copy()
        f = AttentionEnv.FEATS_PER_CHANNEL
        for i in range(dyn.N_CHANNELS):
            out[..., i * f + AttentionEnv.E] = 0.0
            out[..., i * f + AttentionEnv.D] = 0.0
        return out

    @staticmethod
    def mask_surprise_ch(obs: np.ndarray, channel: int) -> np.ndarray:
        """C2b: zero prediction-error features of ONE channel only."""
        out = obs.copy()
        f = AttentionEnv.FEATS_PER_CHANNEL
        out[..., channel * f + AttentionEnv.E] = 0.0
        out[..., channel * f + AttentionEnv.D] = 0.0
        return out

    @staticmethod
    def permute_channels(obs: np.ndarray, perm: np.ndarray) -> np.ndarray:
        """C3: permute whole channel blocks (v, s, e, d move together)."""
        out = obs.copy()
        f = AttentionEnv.FEATS_PER_CHANNEL
        for i in range(dyn.N_CHANNELS):
            j = int(perm[i])
            out[i * f:(i + 1) * f] = obs[j * f:(j + 1) * f]
        return out

    @staticmethod
    def mask_belief(obs: np.ndarray, channel: int) -> np.ndarray:
        """Auxiliary ablation: zero the belief features of one channel."""
        out = obs.copy()
        f = AttentionEnv.FEATS_PER_CHANNEL
        for off in (AttentionEnv.V, AttentionEnv.S, AttentionEnv.PV, AttentionEnv.G):
            out[channel * f + off] = 0.0
        return out
