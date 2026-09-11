"""G0 latent-cause environment.

The agent observes a D-dim sensor vector and picks one of 4 actions.
At any time the world is driven by one latent cause (or an unordered
pair of causes). The agent NEVER receives the cause id.

Step API (gym-like, no gym):

    env = LatentCauseEnv(cfg, seed)
    obs = env.reset()
    obs, reward, done, info = env.step(action)

`reward` is always 0.0 (this experiment is about unsupervised concept
formation; downstream tasks are defined post-hoc on frozen latents).
`info` carries eval-only labels: cause_a, cause_b (-1 = none), ctx_id,
per-slot intensities, the pre-context canonical readout, segment id and
a `switch` flag (True on the first step of a new segment).

Runtime hooks (evaluation only): set_context, set_context_pool,
set_pair_set, set_action_gain, set_segment, get_state/set_state.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Dict, List, Optional, Sequence, Tuple

import numpy as np

from .dynamics import (
    CAUSE_TABLE,
    N_ACTIONS,
    NOOP,
    OOD_PAIRS,
    TAP,
    TRAIN_PAIRS,
    DynamicsParams,
    EnvConfig,
    make_dynamics_params,
)

PAIR_SETS = {"train": TRAIN_PAIRS, "ood": OOD_PAIRS,
             "all": TRAIN_PAIRS + OOD_PAIRS}


class LatentCauseEnv:
    def __init__(self, cfg: Optional[EnvConfig] = None, seed: int = 0):
        self.cfg = cfg or EnvConfig()
        self.params: DynamicsParams = make_dynamics_params(self.cfg, seed)
        self.rng = np.random.default_rng(seed + 1_000_003)
        # runtime hooks
        self._ctx_pool: Optional[List[int]] = None     # sampled at reset
        self._ctx_forced: Optional[int] = None         # set_context
        self._pair_set: str = "train"
        self._action_gain: float = 1.0
        self.reset()

    # ---------------- runtime intervention hooks (eval only) ----------

    def set_context(self, ctx_id: int) -> None:
        """Switch the sensor context immediately (mid-episode allowed)."""
        assert 0 <= int(ctx_id) < len(self.params.contexts)
        self._ctx_forced = int(ctx_id)
        self.ctx_id = int(ctx_id)

    def set_context_pool(self, ids: Sequence[int]) -> None:
        self._ctx_pool = [int(i) for i in ids]

    def set_pair_set(self, name: str) -> None:
        assert name in PAIR_SETS or name == "off"
        self._pair_set = name

    def set_action_gain(self, mul: float) -> None:
        self._action_gain = float(mul)

    def set_segment(self, cause_a: int, cause_b: int = -1,
                    seg_len: Optional[int] = None) -> None:
        """Force the current segment (resets intensities)."""
        self._start_segment(int(cause_a), int(cause_b), seg_len)

    # ---------------- state access -------------------------------------

    def get_state(self) -> dict:
        return {
            "x": self.x.copy(), "env_e": self.env_e.copy(),
            "phase": self.phase.copy(),
            "seg": dict(self.seg), "ar": self.ar_state.copy(),
            "ctx_id": self.ctx_id, "t": self.t, "seg_counter": self.seg_counter,
            "rng": self.rng.bit_generator.state,
        }

    def set_state(self, s: dict) -> None:
        self.x = s["x"].copy()
        self.env_e = s["env_e"].copy()
        self.phase = s["phase"].copy()
        self.seg = dict(s["seg"])
        self.ar_state = s["ar"].copy()
        self.ctx_id = int(s["ctx_id"])
        self._ctx_forced = self.ctx_id
        self.t = int(s["t"])
        self.seg_counter = int(s["seg_counter"])
        self.rng.bit_generator.state = s["rng"]

    # ---------------- segments -----------------------------------------

    def _sample_cause(self, exclude: int = -1) -> int:
        choices = [c for c in range(len(CAUSE_TABLE)) if c != exclude]
        return int(self.rng.choice(choices))

    def _start_segment(self, cause_a: int, cause_b: int = -1,
                       seg_len: Optional[int] = None) -> None:
        cfg = self.cfg
        if seg_len is None:
            seg_len = int(self.rng.integers(cfg.min_segment,
                                            cfg.max_segment + 1))
        self.seg = {"cause_a": cause_a, "cause_b": cause_b,
                    "t": 0, "len": seg_len}
        self.seg_counter += 1
        # reset per-slot latent state; oscillators get a random phase
        self.x[:] = 0.0
        self.env_e[:] = 0.5
        self.phase[:] = self.rng.uniform(0.0, 2.0 * np.pi, 2)

    def _maybe_switch(self) -> bool:
        """Advance segment clock; start a new segment if due."""
        cfg = self.cfg
        seg = self.seg
        seg["t"] += 1
        due = seg["t"] >= seg["len"] or (
            seg["t"] >= cfg.min_segment and self.rng.random()
            < cfg.switch_prob)
        if not due:
            return False
        prev = seg["cause_a"]
        if self._pair_set != "off" and self.rng.random() < cfg.pair_prob:
            pairs = PAIR_SETS[self._pair_set]
            a, b = pairs[int(self.rng.integers(len(pairs)))]
            if a == prev:
                a, b = b, a  # keep cause_a different from previous
            self._start_segment(a, b)
        else:
            self._start_segment(self._sample_cause(exclude=prev))
        return True

    # ---------------- core API ------------------------------------------

    def reset(self, ctx_id: Optional[int] = None) -> np.ndarray:
        cfg = self.cfg
        self.t = 0
        self.seg_counter = -1
        self.x = np.zeros(2)
        self.env_e = np.full(2, 0.5)
        self.phase = np.zeros(2)
        self.ar_state = self.rng.normal(
            0.0, cfg.ar_std, len(self.params.ar_dims))
        if self._ctx_forced is not None:
            self.ctx_id = self._ctx_forced
        elif ctx_id is not None:
            self.ctx_id = int(ctx_id)
        else:
            pool = (self._ctx_pool if self._ctx_pool is not None
                    else list(range(cfg.n_train_contexts)))
            self.ctx_id = int(pool[int(self.rng.integers(len(pool)))])
        self._ctx_forced = None
        self._start_segment(self._sample_cause())
        self._last_f = np.zeros(cfg.n_features)  # x=0 at segment start
        obs, canonical, signal = self._read()
        return obs

    def _cause_output(self, slot: int, cause: int, action: int) -> float:
        """Advance one active cause's intensity; return signal amplitude."""
        cfg = self.cfg
        spec = CAUSE_TABLE[cause]
        drive = spec.resp[action] * self._action_gain
        if spec.osc_freq > 0.0:
            # envelope dynamics; emitted amplitude = e * sin(phase)
            e = spec.decay * self.env_e[slot] + drive \
                + self.rng.normal(0.0, spec.proc)
            self.env_e[slot] = float(np.clip(e, 0.0, 2.5))
            self.phase[slot] += spec.osc_freq
            self.x[slot] = self.env_e[slot] * np.sin(self.phase[slot])
        else:
            x = spec.decay * self.x[slot] + drive \
                + self.rng.normal(0.0, spec.proc)
            if spec.spike_p > 0.0 and self.rng.random() < spec.spike_p:
                x += spec.spike_amp
            if action == TAP and spec.tap_spike_p > 0.0 \
                    and self.rng.random() < spec.tap_spike_p:
                x += spec.spike_amp
            self.x[slot] = float(np.clip(x, -cfg.x_clip, cfg.x_clip))
        return self.x[slot]

    def _features(self, action: int) -> np.ndarray:
        seg = self.seg
        f = self._cause_output(0, seg["cause_a"], action) \
            * self.params.v_table[seg["cause_a"]]
        if seg["cause_b"] >= 0:
            f = f + self._cause_output(1, seg["cause_b"], action) \
                * self.params.v_table[seg["cause_b"]]
        return f

    def _read(self) -> Tuple[np.ndarray, np.ndarray, np.ndarray]:
        """Return (obs, canonical_pre_context, signal_only)."""
        cfg, p, rng = self.cfg, self.params, self.rng
        signal = p.w @ self._last_f
        canonical = signal.copy()
        canonical[p.ar_dims] = self.ar_state
        canonical[p.noise_dims] = rng.normal(0.0, cfg.noise_dim_std,
                                             len(p.noise_dims))
        obs = p.contexts[self.ctx_id] @ canonical \
            + rng.normal(0.0, cfg.obs_noise, cfg.obs_dim)
        return obs, canonical, signal

    def step(self, action: int) -> Tuple[np.ndarray, float, bool, dict]:
        cfg = self.cfg
        action = int(action)
        switch = self._maybe_switch()

        self._last_f = self._features(action)

        # exogenous AR(1) distractors
        self.ar_state = cfg.ar_coef * self.ar_state \
            + np.sqrt(1.0 - cfg.ar_coef ** 2) * cfg.ar_std \
            * self.rng.normal(size=len(self.params.ar_dims))

        obs, canonical, signal = self._read()
        self.t += 1
        done = self.t >= cfg.episode_len

        info = {
            "cause_a": self.seg["cause_a"],
            "cause_b": self.seg["cause_b"],
            "ctx_id": self.ctx_id,
            "x_a": float(self.x[0]),
            "x_b": float(self.x[1] if self.seg["cause_b"] >= 0 else 0.0),
            "canonical": canonical,
            "signal": signal,
            "seg_id": self.seg_counter,
            "switch": bool(switch),
            "action": action,
        }
        return obs, 0.0, done, info

    @property
    def cause_names(self) -> Tuple[str, ...]:
        return tuple(c.name for c in CAUSE_TABLE)


# ------------------------ data collection helpers ----------------------


def rollout(env: LatentCauseEnv, policy, n_steps: int,
            seed: Optional[int] = None,
            ctx_id: Optional[int] = None,
            ctx_schedule: Optional[Dict[int, int]] = None) -> dict:
    """Collect one episode under `policy(rng, t, obs) -> action`.

    ctx_schedule: optional {t: ctx_id} — the sensor context is switched
    before producing the step-t observation (mid-episode context change).
    """
    rng = np.random.default_rng(seed)
    obs = env.reset(ctx_id=ctx_id)
    E: Dict[str, list] = {k: [] for k in (
        "obs", "next_obs", "actions", "cause_a", "cause_b", "x_a", "x_b",
        "seg_id", "switch", "canonical", "signal", "ctx")}
    for t in range(n_steps):
        if ctx_schedule and t in ctx_schedule:
            env.set_context(ctx_schedule[t])
            obs, _, _ = env._read()  # re-render current latent under new ctx
        a = int(policy(rng, t, obs))
        nxt, _, done, info = env.step(a)
        E["obs"].append(obs)
        E["next_obs"].append(nxt)
        E["actions"].append(a)
        E["cause_a"].append(info["cause_a"])
        E["cause_b"].append(info["cause_b"])
        E["x_a"].append(info["x_a"])
        E["x_b"].append(info["x_b"])
        E["seg_id"].append(info["seg_id"])
        E["switch"].append(info["switch"])
        E["canonical"].append(info["canonical"])
        E["signal"].append(info["signal"])
        E["ctx"].append(info["ctx_id"])
        obs = nxt
        if done:
            break
    out = {k: np.asarray(v) for k, v in E.items()}
    out["ctx_id"] = env.ctx_id
    return out


def random_policy(rng: np.random.Generator, t: int, obs: np.ndarray) -> int:
    return int(rng.integers(N_ACTIONS))


def explore_policy(rng: np.random.Generator, t: int, obs: np.ndarray,
                   lo: int = 4, hi: int = 10) -> int:
    """Temporally blocked policy: holds each action for a few steps so
    per-action response signatures are expressed clearly."""
    if t == 0 or t >= getattr(explore_policy, "_until", 0):
        explore_policy._last = int(rng.integers(N_ACTIONS))
        explore_policy._until = t + int(rng.integers(lo, hi + 1))
    return explore_policy._last
