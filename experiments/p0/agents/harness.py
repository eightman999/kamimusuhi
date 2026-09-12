"""Belief harness: assembles the agent input from env obs + predictor.

The env emits per-channel belief features grouped by kind:
    env_obs = [v(4), s(4), pv(4), g(4)]           (16,)

The harness rearranges them per channel, queries the predictor for the
surprise features (e = EMA prediction error, d = predicted error), and
returns the blocked agent input:

    agent_obs = [v_i, s_i, pv_i, g_i, e_i, d_i] x 4 channels   (24,)

Causal ablations are applied to agent_obs afterwards by the evaluator --
they never touch the env or the predictor.

Call order per step:
    inp = harness.agent_input(env_obs)     # stores predictor feats
    a   = policy.act(inp)
    env_obs2, r, done, info = env.step(a)
    harness.update(env_obs2)               # SGD on newly observed channels
"""

from __future__ import annotations

import numpy as np
import torch

from ..env import dynamics as dyn
from ..env.attention_env import AttentionEnv
from ..models.predictor import ChannelPredictor

F = AttentionEnv.FEATS_PER_CHANNEL          # 6
OBS_DIM = dyn.N_CHANNELS * F                # 24


class BeliefHarness:
    def __init__(self, predictor: ChannelPredictor, err_scale: float = 4.0):
        self.pred = predictor
        self.err_scale = err_scale
        self._feats: torch.Tensor | None = None  # feats used for last predict

    def reset(self, n_envs: int):
        self.pred.reset_state(n_envs)
        self._feats = None

    # ------------------------------------------------------------------
    @staticmethod
    def channel_feats(env_obs: np.ndarray) -> np.ndarray:
        """(n, 16) env obs -> (n, C, 4) per-channel predictor features."""
        n = env_obs.shape[0]
        C = dyn.N_CHANNELS
        v = env_obs[:, 0 * C:1 * C]
        s = env_obs[:, 1 * C:2 * C]
        pv = env_obs[:, 2 * C:3 * C]
        g = env_obs[:, 3 * C:4 * C]
        return np.stack([v, s, pv, g], axis=-1)  # (n, C, 4)

    def agent_input(self, env_obs: np.ndarray) -> np.ndarray:
        """env_obs (n,16) -> agent_obs (n,24) with blocked layout."""
        n = env_obs.shape[0]
        C = dyn.N_CHANNELS
        feats = self.channel_feats(env_obs)
        self._feats = torch.as_tensor(feats, dtype=torch.float32)
        e, d = self.pred.features(self._feats)
        # squash to ~[0,1]
        e = np.clip(self.err_scale * e, 0.0, 1.0)
        d = np.clip(self.err_scale * d, 0.0, 1.0)
        out = np.zeros((n, OBS_DIM), dtype=np.float64)
        out[:, 0::F] = env_obs[:, 0 * C:1 * C]             # v blocks
        out[:, 1::F] = env_obs[:, 1 * C:2 * C]             # s
        out[:, 2::F] = env_obs[:, 2 * C:3 * C]             # pv
        out[:, 3::F] = env_obs[:, 3 * C:4 * C]             # g
        out[:, 4::F] = e
        out[:, 5::F] = d
        return out

    def update(self, env_obs_new: np.ndarray,
               just_reset: np.ndarray | None = None,
               train: bool = True):
        """Train the predictor on channels observed in the *new* env obs.

        A channel was observed this step iff its staleness == 0. Target =
        the fresh belief value v. Also zeroes EMA-error state for envs that
        were just reset. With train=False the predictor weights are frozen
        (no SGD); the EMA error state still updates.
        """
        if self._feats is None:
            return 0.0, np.zeros((env_obs_new.shape[0], dyn.N_CHANNELS))
        C = dyn.N_CHANNELS
        mask = env_obs_new[:, 1 * C:2 * C] == 0.0           # staleness == 0
        targets = env_obs_new[:, 0:C]                        # v
        loss, err = self.pred.update(
            self._feats,
            torch.as_tensor(mask, dtype=torch.bool),
            torch.as_tensor(targets, dtype=torch.float32),
            train=train,
        )
        if just_reset is not None and just_reset.any():
            self.pred.e[just_reset] = 0.0
        return loss, err

    # ------------------------------------------------------------------
    def surprise(self, env_obs: np.ndarray) -> np.ndarray:
        """Current (e + d) surprise per channel (used by PE heuristic)."""
        feats = torch.as_tensor(self.channel_feats(env_obs), dtype=torch.float32)
        e, d = self.pred.features(feats)
        return e + d
