"""Bayes oracle for U0: the difficulty-aware upper bound.

The oracle knows the episode's *difficulty* -- regime, sigma, support set
S and the true latent mean mu_ep -- but never the realized label y.  It
plans how many samples to take via the achievable accuracy acc(n) =
E[max_k P(y=k | xbar_n)] under the true generative parameters, and it
answers via the honest Bayesian posterior over y (marginalizing the
latent class under the regime-level prior).  Observing is worth its cost
exactly when acc(n) rises enough to pay for it, so the oracle uses all
three meta-actions: answer on easy episodes, observe on noisy/weak ones,
abstain on ambiguous/null ones.
"""

from __future__ import annotations

import math

import numpy as np

from ..env.uncertainty_env import (U0Config, U0Env, ambiguity_sets,
                                   _pattern_unit_vector)
from .base import Policy


class BayesOracle(Policy):
    """Per-env oracle; the eval loop calls ``decide_env``."""

    name = "oracle"
    uses_u = False                      # decides from env.latent, not u
    batched = False

    def __init__(self, cfg: U0Config, n_mc: int = 128, seed: int = 5):
        self.cfg = cfg
        self.n_mc = n_mc
        self.rng = np.random.default_rng(seed)
        self._acc_cache: dict = {}
        self._post = np.full(cfg.n_classes, 1.0 / cfg.n_classes)
        self._prec = self._pattern_precision()

    # -- generative hypotheses ----------------------------------------
    def _pattern_precision(self) -> np.ndarray | None:
        c = self.cfg
        if c.ood != "pattern":
            return None
        v = _pattern_unit_vector(c)
        sigma = c.sigma_easy
        cov = sigma ** 2 * (np.eye(c.feat_dim) +
                            c.pattern_rho ** 2 * np.outer(v, v))
        return sigma ** 2 * np.linalg.inv(cov)   # sigma^2 * Sigma^-1

    def _hyps(self, env: U0Env):
        """[(prior, mu, plabel)] under the *known* regime prior."""
        c = env.cfg
        K = c.n_classes
        reg = env.latent["regime"]
        prior = np.asarray(c.class_prior, dtype=np.float64)
        prior = prior / prior.sum()
        proto = env.proto_alt if c.ood == "shift" else env.proto
        if reg == "ambiguous":
            out = []
            for S in ambiguity_sets(c):
                mu = proto[:, list(S)].mean(axis=1)
                nrm = np.linalg.norm(mu)
                mu = c.amb_gain * c.signal * (mu / nrm if nrm > 1e-8 else mu)
                m = len(S)
                p_m = c.amb_m_probs[0] if m == 2 else c.amb_m_probs[1]
                p_s = sum(prior[k] for k in S) / math.comb(K - 1, m - 1) * p_m
                plabel = np.zeros(K)
                plabel[list(S)] = 1.0 / m
                out.append((p_s, mu, plabel))
            return out
        if reg == "null":
            return [(1.0, np.zeros(c.feat_dim), np.full(K, 1.0 / K))]
        gain = c.weak_gain if reg == "weak" else 1.0
        out = []
        for k in range(K):
            plabel = np.zeros(K)
            plabel[k] = 1.0
            out.append((prior[k], gain * proto[:, k], plabel))
        return out

    def _quad(self, x: np.ndarray, mu: np.ndarray, sigma: float,
              env: U0Env, n: int) -> np.ndarray:
        """-0.5 n (x-mu)^T Sigma^-1 (x-mu); x may be (D,) or (M,D)."""
        mask = env.latent.get("mask")
        mu_p = mu * mask if mask is not None else mu
        r = x - mu_p
        if self._prec is None:
            q = np.einsum("...d,...d->...", r, r)
        else:
            q = np.einsum("...d,de,...e->...", r, self._prec, r)
        return -0.5 * n * q / (sigma ** 2)

    def _posterior(self, env: U0Env, x: np.ndarray, n: int) -> np.ndarray:
        """P(y=k | xbar_n = x) under the regime prior; x (D,) or (M,D)."""
        K = env.cfg.n_classes
        m = 1 if x.ndim == 1 else x.shape[0]
        logp = np.full((m, K), -np.inf)
        for prior, mu, plabel in self._hyps(env):
            ll = self._quad(x, mu, env.latent["sigma"], env, n)
            for k in range(K):
                if plabel[k] > 0:
                    v = math.log(max(prior, 1e-12) * plabel[k]) + ll
                    logp[:, k] = np.logaddexp(logp[:, k], v)
        logp -= logp.max(axis=-1, keepdims=True)
        p = np.exp(logp)
        p = p / p.sum(axis=-1, keepdims=True)
        return p[0] if x.ndim == 1 else p

    def posterior(self, env: U0Env) -> np.ndarray:
        return self._posterior(env, env.samples.mean(axis=0),
                               max(1, env.n))

    # -- planning: achievable accuracy vs n ---------------------------
    def acc_at(self, env: U0Env, n: int) -> float:
        lat = env.latent
        key = (lat["regime"], tuple(lat["S"]), round(lat["sigma"], 4), n,
               None if lat.get("mask") is None else lat["mask"].tobytes())
        if key in self._acc_cache:
            return self._acc_cache[key]
        c = env.cfg
        xs = lat["mu"][None, :] + lat["sigma"] * self.rng.normal(
            size=(self.n_mc, c.feat_dim))
        if lat.get("mask") is not None:
            xs = xs * lat["mask"]
        p = self._posterior(env, xs, n)
        acc = float(p.max(axis=-1).mean())
        self._acc_cache[key] = acc
        return acc

    def plan(self, env: U0Env) -> int:
        """n* = argmax_k EV(k); 0 means abstain now."""
        c = env.cfg
        best_ev, best_k = c.reward_abstain, 0
        for k in range(1, c.max_obs + 1):
            acc = self.acc_at(env, k)
            ev = -c.cost_observe * (k - 1) + \
                acc * c.reward_correct + (1 - acc) * c.reward_wrong
            if ev > best_ev:
                best_ev, best_k = ev, k
        return best_k

    # -- policy interface ----------------------------------------------
    def decide_env(self, env: U0Env) -> int:
        n_star = self.plan(env)
        self._post = self.posterior(env)
        if n_star == 0:
            return env.cfg.abstain_id
        if env.n < n_star:
            return env.cfg.observe_id
        return int(np.argmax(self._post))

    # recorded u for analysis: the oracle's TRUE posterior confidence
    def u_env(self, env: U0Env) -> np.ndarray:
        top2 = np.sort(self._post)[::-1]
        var = env.samples.var(axis=0, ddof=1).mean() if env.n >= 2 else 0.0
        return np.array([self._post.max(), top2[0] - top2[1],
                         min(var, 3.0), env.n / env.cfg.max_obs])
