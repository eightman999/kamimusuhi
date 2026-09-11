"""U0 uncertainty / metacognition environment.

Each episode is a small sequential decision problem: the agent observes
noisy samples drawn from a latent class and must either commit to an
answer, pay to draw another sample from the *same* latent class, or
abstain ("I don't know").

Generative model (per episode):

    regime  ~ regime_probs            easy | noisy | weak | ambiguous | null
    c       ~ class_prior             intended class
    S       = {c}                     label-support set (easy/noisy/weak)
            = {c} + 1-2 random others (ambiguous)
            = all K classes           (null)
    mu_ep   = signal * P[:,c]                    (easy/noisy)
            = weak_gain * signal * P[:,c]        (weak)
            = amb_gain * signal * mean(P[:,S])   (ambiguous)
            = 0                                  (null)
    sigma   = per-regime observation noise
    y       ~ Uniform(S)              the graded label; for |S|=1, y = c
    x_i     ~ N(mu_ep, sigma^2 I)     iid samples

For `ambiguous` episodes the label itself is genuinely stochastic (the
data-generating process cannot determine it), which is the honest form of
irreducible aleatoric uncertainty: extra samples sharpen the estimate of
mu_ep but never resolve y.  `null` episodes carry no signal at all.

Observation at decision point n (n = samples seen so far, 1..max_obs):

    obs = [mean_i x_i  (feat_dim),  var_z,  n/max_obs]      (feat_dim + 2)

`var_z` is the mean per-dimension sample variance (0 at n=1) scaled by a
fixed constant.  It lets the agent *estimate* observability from data --
difficulty is statistically inferable but never explicitly labeled:
`env.latent` (regime / S / y / sigma / mu_ep) is instrumentation for the
oracle and the analysis only, and `obs` is a pure function of the sample
set plus the sample count (see tests/test_env.py::test_no_label_leak).

Actions (flat space, K+2 = 6):
    0..K-1  ANSWER_k   terminal; reward_correct if k == y else reward_wrong
    K       ABSTAIN    terminal; reward_abstain
    K+1     OBSERVE    cost_observe; draws another sample.  At max_obs the
                       budget is exhausted -> terminal forced abstain
                       (reward_abstain, no extra cost).

Eval-only OOD modes (env cfg `ood`): "noise" (unseen sigma), "shift"
(samples generated from a rotated prototype basis), "missing" (a random
half of the feature dims zeroed), "pattern" (rank-1 correlated noise).
"""

from __future__ import annotations

from dataclasses import dataclass, field
from itertools import combinations
from typing import Optional

import numpy as np

# meta action names (flat ids depend on n_classes)
ANSWER = "answer"
ABSTAIN = "abstain"
OBSERVE = "observe"
META_ACTIONS = (ANSWER, OBSERVE, ABSTAIN)

REGIMES = ("easy", "noisy", "weak", "ambiguous", "null")
OOD_MODES = ("noise", "shift", "missing", "pattern")

VAR_SCALE = 2.0          # fixed normalizer for the sample-variance feature
VAR_CLIP = 3.0


@dataclass
class U0Config:
    n_classes: int = 4
    feat_dim: int = 8
    signal: float = 3.0
    max_obs: int = 5

    sigma_easy: float = 0.5
    sigma_noisy: float = 1.5
    sigma_weak: float = 1.0
    sigma_ambiguous: float = 0.8
    sigma_null: float = 1.0
    sigma_ood: float = 2.4            # ood="noise": unseen noise level

    weak_gain: float = 0.5            # signal attenuation for "weak"
    amb_gain: float = 0.9             # signal scale for ambiguous means
    amb_m_probs: tuple = (0.4, 0.6)   # P(|S|=2), P(|S|=3)
    regime_probs: tuple = (0.35, 0.15, 0.15, 0.20, 0.15)
    class_prior: tuple = (0.25, 0.25, 0.25, 0.25)

    reward_correct: float = 1.0
    reward_wrong: float = -1.0
    reward_abstain: float = 0.05
    cost_observe: float = 0.10

    ood: Optional[str] = None         # None | noise | shift | missing | pattern
    missing_frac: float = 0.5         # ood="missing": fraction of dims dropped
    pattern_rho: float = 2.0          # ood="pattern": rank-1 noise gain
    noise_scale: float = 1.0          # U-C3: multiplies every sigma
    prototype_seed: int = 12345
    seed: int = 0

    @property
    def obs_dim(self) -> int:
        return self.feat_dim + 2

    @property
    def n_actions(self) -> int:
        return self.n_classes + 2

    @property
    def abstain_id(self) -> int:
        return self.n_classes

    @property
    def observe_id(self) -> int:
        return self.n_classes + 1


def make_prototypes(cfg: U0Config, alt: bool = False) -> np.ndarray:
    """(feat_dim, n_classes) orthonormal-ish prototype basis * signal.

    `alt` is the shifted basis used by the ood="shift" eval mode.
    """
    rng = np.random.default_rng(cfg.prototype_seed + (999 if alt else 0))
    a = rng.normal(size=(cfg.feat_dim, cfg.n_classes))
    q, _ = np.linalg.qr(a)
    return (q * cfg.signal).astype(np.float64)


def ambiguity_sets(cfg: U0Config) -> list[tuple[int, ...]]:
    """All support sets S the ambiguous sampler can produce (|S| in {2,3})."""
    k = cfg.n_classes
    return [tuple(s) for m in (2, 3) for s in combinations(range(k), m)]


def _pattern_unit_vector(cfg: U0Config) -> np.ndarray:
    rng = np.random.default_rng(cfg.prototype_seed + 555)
    v = rng.normal(size=cfg.feat_dim)
    return v / np.linalg.norm(v)


class U0Env:
    """Single-episode env; reset() draws a fresh latent episode."""

    def __init__(self, config: U0Config):
        self.cfg = config
        self.rng = np.random.default_rng(config.seed)
        self.proto = make_prototypes(config)
        self.proto_alt = make_prototypes(config, alt=True)
        self.pat_vec = _pattern_unit_vector(config)
        self.n = 0
        self.samples = np.zeros((0, config.feat_dim))
        self.done = True
        self.latent: dict = {}
        self.action_counts = np.zeros(config.n_actions, dtype=np.int64)
        self.total_observe_cost = 0.0
        self._ep_seed = 0

    # ------------------------------------------------------------------
    def _sigmas(self) -> dict:
        c = self.cfg
        s = c.noise_scale
        return {
            "easy": c.sigma_easy * s,
            "noisy": c.sigma_noisy * s,
            "weak": c.sigma_weak * s,
            "ambiguous": c.sigma_ambiguous * s,
            "null": c.sigma_null * s,
            "ood_noise": c.sigma_ood,
            "ood_shift": c.sigma_easy,
            "ood_missing": c.sigma_easy,
            "ood_pattern": c.sigma_easy,
        }

    def _sample_latent(self, rng: np.random.Generator) -> dict:
        c = self.cfg
        prior = np.asarray(c.class_prior, dtype=np.float64)
        prior = prior / prior.sum()
        K = c.n_classes
        proto = self.proto_alt if c.ood == "shift" else self.proto
        sigmas = self._sigmas()

        if c.ood is not None:
            regime = f"ood_{c.ood}"
            y = int(rng.choice(K, p=prior))
            S = (y,)
            mu = proto[:, y].copy()
            sigma = sigmas[regime]
            mask = None
            if c.ood == "missing":
                n_miss = int(round(c.feat_dim * c.missing_frac))
                mask = np.ones(c.feat_dim, dtype=bool)
                mask[rng.choice(c.feat_dim, n_miss, replace=False)] = False
            return {"regime": regime, "c": y, "S": S, "y": y, "mu": mu,
                    "sigma": float(sigma), "mask": mask}

        regime = REGIMES[rng.choice(len(REGIMES), p=np.asarray(c.regime_probs))]
        c_int = int(rng.choice(K, p=prior))
        if regime == "ambiguous":
            m = 2 if rng.random() < c.amb_m_probs[0] else 3
            others = [k for k in range(K) if k != c_int]
            S = tuple(sorted([c_int] + list(rng.choice(others, m - 1,
                                                     replace=False))))
            direction = proto[:, list(S)].mean(axis=1)
            norm = np.linalg.norm(direction)
            direction = direction / norm if norm > 1e-8 else proto[:, c_int]
            mu = c.amb_gain * c.signal * direction
            y = int(rng.choice(np.asarray(S)))
        elif regime == "null":
            S = tuple(range(K))
            mu = np.zeros(c.feat_dim)
            y = int(rng.choice(K))
        else:
            S = (c_int,)
            gain = c.weak_gain if regime == "weak" else 1.0
            mu = gain * proto[:, c_int]
            y = c_int
        return {"regime": regime, "c": c_int, "S": S, "y": y, "mu": mu,
                "sigma": float(sigmas[regime]), "mask": None}

    # ------------------------------------------------------------------
    def reset(self, seed: Optional[int] = None,
              latent: Optional[dict] = None) -> np.ndarray:
        if seed is not None:
            self.rng = np.random.default_rng(seed)
        self._ep_seed = int(self.rng.integers(2**31 - 1))
        self.rng = np.random.default_rng(self._ep_seed)
        self.latent = latent if latent is not None else self._sample_latent(self.rng)
        self.n = 0
        self.done = False
        self.action_counts[:] = 0
        self.total_observe_cost = 0.0
        self.samples = np.zeros((0, self.cfg.feat_dim))
        self._draw_sample()
        return self._obs()

    def _draw_sample(self) -> None:
        c = self.cfg
        lat = self.latent
        eps = self.rng.normal(size=c.feat_dim)
        if c.ood == "pattern":
            eps = eps + c.pattern_rho * float(self.rng.normal()) * self.pat_vec
        x = lat["mu"] + lat["sigma"] * eps
        if lat.get("mask") is not None:
            x = x * lat["mask"]
        self.samples = np.vstack([self.samples, x[None, :]])
        self.n += 1

    def _obs(self) -> np.ndarray:
        c = self.cfg
        mean = self.samples.mean(axis=0)
        if self.n >= 2:
            var = float(self.samples.var(axis=0, ddof=1).mean())
        else:
            var = 0.0
        var_z = min(var / VAR_SCALE, VAR_CLIP)
        return np.concatenate(
            [mean, [var_z, self.n / c.max_obs]]).astype(np.float64)

    # ------------------------------------------------------------------
    def step(self, action: int):
        if self.done:
            raise RuntimeError("step() on finished episode; call reset()")
        c = self.cfg
        action = int(action)
        if not 0 <= action < c.n_actions:
            raise ValueError(f"action {action} out of range")
        self.action_counts[action] += 1
        info: dict = {"n_obs": self.n, "regime": self.latent["regime"]}
        reward = 0.0
        outcome = None

        if action < c.n_classes:                       # ANSWER_k
            correct = action == self.latent["y"]
            reward = c.reward_correct if correct else c.reward_wrong
            outcome = "answer"
            info.update(correct=bool(correct), answer=action,
                        y=int(self.latent["y"]))
            self.done = True
        elif action == c.abstain_id:                   # ABSTAIN
            reward = c.reward_abstain
            outcome = "abstain"
            self.done = True
        else:                                          # OBSERVE_MORE
            if self.n < c.max_obs:
                self._draw_sample()
                reward = -c.cost_observe
                self.total_observe_cost += c.cost_observe
            else:                                      # budget exhausted
                reward = c.reward_abstain
                outcome = "forced_abstain"
                info["forced_abstain"] = True
                self.done = True

        info["outcome"] = outcome
        if self.done:
            info["ep_record"] = self.episode_record(outcome)
        obs = self._obs() if not self.done else np.zeros(c.obs_dim)
        return obs, float(reward), self.done, info

    # ------------------------------------------------------------------
    def episode_record(self, outcome: Optional[str]) -> dict:
        """Per-episode bookkeeping for analysis (not part of obs)."""
        lat = self.latent
        return {
            "regime": lat["regime"],
            "c": int(lat["c"]), "y": int(lat["y"]),
            "S": list(lat["S"]),
            "sigma": float(lat["sigma"]),
            "n_obs": int(self.n),
            "outcome": outcome,
            "observe_cost": float(self.total_observe_cost),
            "action_counts": self.action_counts.tolist(),
        }


class VecU0Env:
    """Batched wrapper for rollouts (PPO + batched causal evaluation).

    No auto-reset inside step(): callers track `done` per slot so exactly
    one episode record is produced per episode.
    """

    def __init__(self, config: U0Config, num_envs: int, seed: int):
        self.envs = []
        for i in range(num_envs):
            c = U0Config(**{**config.__dict__, "seed": seed + 1000 * i})
            self.envs.append(U0Env(c))
        self.num_envs = num_envs
        self.obs_dim = config.obs_dim
        self.cfg = config

    def reset(self) -> np.ndarray:
        return np.stack([e.reset() for e in self.envs])

    def step(self, actions: np.ndarray):
        obs, rew, done, infos = [], [], [], []
        for e, a in zip(self.envs, actions):
            if e.done:                       # dead slot: no-op
                obs.append(np.zeros(e.cfg.obs_dim))
                rew.append(0.0)
                done.append(True)
                infos.append({})
                continue
            o, r, d, i = e.step(int(a))
            obs.append(o)
            rew.append(r)
            done.append(d)
            infos.append(i)
        return (np.stack(obs), np.asarray(rew, dtype=np.float32),
                np.asarray(done, dtype=bool), infos)
