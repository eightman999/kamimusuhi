"""S0 agency environment.

The agent observes a 16-dim sensor vector and chooses one of 4 actions.
Some sensors depend on the agent's action, some on exogenous dynamics,
some on both. The agent is never told which is which.

Exact attribution is maintained with a shadow state: a parallel latent
evolves with identical noise and disturbances but NOOP actions. The
per-step decomposition is then exact even under nonlinearity:

    obs(t) = world_component + action_component + noise_component

where action_component = permute(couple(W @ (z - z_shadow))).
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Optional

import numpy as np

from .dynamics import (
    N_ACTIONS,
    CauseLabels,
    DynamicsParams,
    EnvConfig,
    make_dynamics_params,
)


@dataclass
class StepInfo:
    cause_labels: np.ndarray       # (obs_dim,) eval-only
    action_component: np.ndarray   # (obs_dim,) self-caused part of obs
    world_component: np.ndarray    # (obs_dim,) exogenous signal part
    noise_component: np.ndarray    # (obs_dim,) i.i.d. read/noise part
    disturbance: bool              # external impulse fired this step
    action_effective: int          # action id whose effect landed this step


class AgencyEnv:
    def __init__(self, cfg: Optional[EnvConfig] = None, seed: int = 0):
        self.cfg = cfg or EnvConfig()
        self.params: DynamicsParams = make_dynamics_params(self.cfg, seed)
        self.rng = np.random.default_rng(seed + 1_000_003)
        # sensor coupling mixer: zero-diagonal random matrix, only used
        # when cfg.sensor_coupling > 0
        c_rng = np.random.default_rng(seed + 2_000_003)
        m = c_rng.normal(size=(self.cfg.obs_dim, self.cfg.obs_dim))
        np.fill_diagonal(m, 0.0)
        m /= max(1e-8, float(np.abs(m).sum(axis=1).max()))
        self._couple_m = m

        self._action_perm = np.arange(N_ACTIONS)  # S-C4 hook
        self._action_gain_mul = 1.0               # OOD hook
        self._disturb_prob_mul = 1.0              # OOD hook
        self._disturb_gain_mul = 1.0              # OOD hook
        self._pending: list[np.ndarray] = []      # delayed action effects
        self.reset()

    # ---- runtime intervention hooks (evaluation only) ----

    def set_action_permutation(self, perm: np.ndarray) -> None:
        """S-C4: permute which action id triggers which effect."""
        perm = np.asarray(perm, dtype=np.int64)
        assert sorted(perm.tolist()) == list(range(N_ACTIONS))
        self._action_perm = perm

    def set_action_gain(self, mul: float) -> None:
        self._action_gain_mul = float(mul)

    def set_disturbance(self, prob_mul: float, gain_mul: float) -> None:
        self._disturb_prob_mul = float(prob_mul)
        self._disturb_gain_mul = float(gain_mul)

    # ---- state access for counterfactual evaluation ----

    def get_state(self) -> dict:
        return {
            "z_self": self.z_self.copy(),
            "z_ext": self.z_ext.copy(),
            "z_mix": self.z_mix.copy(),
            "z_self_sh": self.z_self_sh.copy(),
            "z_mix_sh": self.z_mix_sh.copy(),
            "pending": [p.copy() for p in self._pending],
            "t": self.t,
            "rng": self.rng.bit_generator.state,
        }

    def set_state(self, state: dict) -> None:
        self.z_self = state["z_self"].copy()
        self.z_ext = state["z_ext"].copy()
        self.z_mix = state["z_mix"].copy()
        self.z_self_sh = state["z_self_sh"].copy()
        self.z_mix_sh = state["z_mix_sh"].copy()
        self._pending = [p.copy() for p in state["pending"]]
        self.t = state["t"]
        self.rng.bit_generator.state = state["rng"]

    # ---- core API ----

    def reset(self) -> np.ndarray:
        cfg = self.cfg
        self.z_self = self.rng.normal(0.0, 0.3, cfg.z_self_dim)
        self.z_ext = self.rng.normal(0.0, 0.5, cfg.z_ext_dim)
        self.z_mix = self.rng.normal(0.0, 0.3, cfg.z_mix_dim)
        self.z_self_sh = self.z_self.copy()
        self.z_mix_sh = self.z_mix.copy()
        self._pending = [np.zeros(cfg.z_self_dim + cfg.z_mix_dim)
                         for _ in range(cfg.action_delay)]
        self.t = 0
        obs, _, _, _ = self._read_obs_decomposed()
        return obs

    @property
    def cause_labels(self) -> np.ndarray:
        return self.params.cause_labels

    def _action_latent(self, action: int) -> np.ndarray:
        """Effect of an action on (z_self, z_mix), after permutation/gain."""
        a = int(self._action_perm[int(action)])
        eff_self = self.params.action_map[a] * self.cfg.self_act_coef
        eff_mix = self.params.mix_action_map[a] * self.cfg.mix_act_coef
        return np.concatenate([eff_self, eff_mix]) * (
            self.cfg.action_gain * self._action_gain_mul
        )

    def _squash(self, x: np.ndarray) -> np.ndarray:
        k = self.cfg.nonlinearity
        return x if k == 0.0 else x * (1.0 - k) + k * np.tanh(x)

    def step(self, action: int) -> tuple[np.ndarray, StepInfo]:
        cfg = self.cfg
        rng = self.rng

        # queue this action's latent effect; pop whatever lands now
        self._pending.append(self._action_latent(action))
        due = self._pending.pop(0)
        act_self, act_mix = due[: cfg.z_self_dim], due[cfg.z_self_dim:]

        # exogenous disturbance impulse
        dist = rng.random() < cfg.disturbance_prob * self._disturb_prob_mul
        d_ext = np.zeros(cfg.z_ext_dim)
        d_mix = np.zeros(cfg.z_mix_dim)
        if dist:
            d_ext = rng.normal(size=cfg.z_ext_dim)
            d_ext *= cfg.disturbance_gain * self._disturb_gain_mul
            d_mix = rng.normal(size=cfg.z_mix_dim)
            d_mix *= cfg.mix_disturbance_gain * self._disturb_gain_mul

        # draw process noise once; real and shadow states share it so the
        # action residue (z - z_shadow) stays exactly attributable
        n_self = rng.normal(0.0, cfg.process_noise, cfg.z_self_dim)
        n_mix = rng.normal(0.0, cfg.process_noise, cfg.z_mix_dim)

        self.z_self = self._squash(self.z_self * cfg.self_decay) + act_self + n_self
        self.z_self_sh = self._squash(self.z_self_sh * cfg.self_decay) + n_self
        self.z_ext = self.params.ext_ar @ self.z_ext + d_ext \
            + rng.normal(0.0, cfg.process_noise, cfg.z_ext_dim)
        self.z_mix = self._squash(self.z_mix * cfg.mix_decay) + act_mix + d_mix + n_mix
        self.z_mix_sh = self._squash(self.z_mix_sh * cfg.mix_decay) + d_mix + n_mix

        obs, world, act, noise = self._read_obs_decomposed()
        self.t += 1

        return obs, StepInfo(
            cause_labels=self.params.cause_labels,
            action_component=act,
            world_component=world,
            noise_component=noise,
            disturbance=dist,
            action_effective=int(action),
        )

    def _canonical_read(self, z_self, z_ext, z_mix, noise_dims):
        p, cfg = self.params, self.cfg
        z_cat = np.concatenate([z_self, z_ext, z_mix])
        return np.concatenate([
            p.w_self @ z_self,
            p.w_ext @ z_ext,
            p.w_mix @ z_cat,
            noise_dims,
        ])

    def _couple_and_permute(self, x: np.ndarray) -> np.ndarray:
        c = self.cfg.sensor_coupling
        if c > 0.0:
            x = x + c * (self._couple_m @ x)
        return x[self.params.obs_perm]

    def _read_obs_decomposed(self):
        cfg = self.cfg
        n_noise = cfg.obs_dim - cfg.n_self_dims - cfg.n_ext_dims - cfg.n_mix_dims

        noise_dims = self.rng.normal(0.0, cfg.noise_dim_std, n_noise)
        read_noise = self.rng.normal(0.0, cfg.obs_noise, cfg.obs_dim)

        signal = self._canonical_read(self.z_self, self.z_ext, self.z_mix,
                                      noise_dims)
        obs = self._couple_and_permute(signal + read_noise)

        res_cat = np.concatenate([
            self.z_self - self.z_self_sh,
            np.zeros(cfg.z_ext_dim),
            self.z_mix - self.z_mix_sh,
        ])
        act = self._couple_and_permute(np.concatenate([
            self.params.w_self @ res_cat[: cfg.z_self_dim],
            np.zeros(cfg.n_ext_dims),
            self.params.w_mix @ res_cat,
            np.zeros(n_noise),
        ]))

        sh_cat = (self.z_self_sh, self.z_ext, self.z_mix_sh)
        world_sig = self._canonical_read(*sh_cat, np.zeros(n_noise))
        world = self._couple_and_permute(world_sig)

        # remainder: i.i.d. noise dims + read noise (+ coupling cross-terms)
        noise = obs - world - act
        return obs, world, act, noise


def rollout(env: AgencyEnv, policy, n_steps: int,
            seed: Optional[int] = None) -> dict:
    """Collect one trajectory under `policy(rng, t, obs) -> action`."""
    rng = np.random.default_rng(seed)
    obs = env.reset()
    obs_buf, act_buf = [obs], []
    world_buf, actc_buf, noise_buf, dist_buf = [], [], [], []
    for t in range(n_steps):
        a = int(policy(rng, t, obs))
        obs, info = env.step(a)
        obs_buf.append(obs)
        act_buf.append(a)
        world_buf.append(info.world_component)
        actc_buf.append(info.action_component)
        noise_buf.append(info.noise_component)
        dist_buf.append(info.disturbance)
    return {
        "obs": np.stack(obs_buf[:-1]),          # (T, obs_dim) inputs
        "next_obs": np.stack(obs_buf[1:]),      # (T, obs_dim) targets
        "actions": np.asarray(act_buf),         # (T,)
        "world": np.stack(world_buf),           # components of next_obs
        "action_comp": np.stack(actc_buf),
        "noise": np.stack(noise_buf),
        "disturbance": np.asarray(dist_buf),
    }


def random_policy(rng: np.random.Generator, t: int, obs: np.ndarray) -> int:
    return int(rng.integers(N_ACTIONS))


def habitual_policy(rng: np.random.Generator, t: int, obs: np.ndarray,
                    repeat: float = 0.7) -> int:
    """Temporally correlated policy: repeats the previous action often."""
    if t > 0 and rng.random() < repeat and hasattr(habitual_policy, "_last"):
        return habitual_policy._last
    a = int(rng.integers(N_ACTIONS))
    habitual_policy._last = a
    return a
