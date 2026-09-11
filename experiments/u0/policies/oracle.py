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

from ..env.uncertainty_env import U0Config, U0Env, ambiguity_sets
from .base import Policy


def _log_gauss(x: np.ndarray, mu: np.ndarray, sigma: float,
               prec_scale: np.ndarray | None) -> float:
    """-0.5 * n * (x-mu)^T Sigma^-1 (x-mu), constants dropped.

    prec_scale, when given, is sigma^2 * Sigma^-1 (i.e. the normalized
    precision), so the result is -0.5 * n/sigma^2 * (x-mu)^T P (x-mu).
    """
    r = x - mu
    if prec_scale is None:
        return -0.5 * float(r @ r) / (sigma ** 2)
    return -0.5 * float(r @ prec_scale @ r) / (sigma ** 2)


class BayesOracle(Policy):
    """Per-env oracle; the eval loop calls ``decide_env``."""

    name = "oracle"
    uses_u = False                      # decides from env.latent, not u
    batched = False

    def __init__(self, cfg: U0Config, n_mc: int = 192, seed: int = 5):
        self.cfg = cfg
        self.n_mc = n_mc
        self.rng = np.random.default_rng(seed)
        self._acc_cache: dict = {}
        self._plan: int | None = None
        self._post = np.full(cfg.n_classes, 1.0 / cfg.n_classes)
        self._prec = self._pattern_precision()

    # -- generative hypotheses ----------------------------------------
    def _pattern_precision(self) -> np.ndarray | None:
        c = self.cfg
        if c.ood != "pattern":
            return None
        from ..env.uncertainty_env import _pattern_unit_vector
        v = _pattern_unit_vector(c)
        sigma = c.sigma_easy
        cov = sigma ** 2 * (np.eye(c.feat_dim) +
                            c.pattern_rho ** 2 * np.outer(v, v))
        return sigma ** 2 * np.linalg.inv(cov)

    def _hyps(self, env: U0Env):
        """[(prior, mu, p_label)] under the *known* regime prior."""
        c = env.cfg
        lat = env.latent
        K = c.n_classes
        prior = np.asarray(c.class_prior, dtype=np.float64)
        prior = prior / prior.sum()
        proto = env.proto_alt if c.ood == "shift" else env.proto
        reg = lat["regime"]
        if reg == "ambiguous":
            out = []
            for S in ambiguity_sets(c):
                mu = proto[:, list(S)].mean(axis=1)
                nrm = np.linalg.norm(mu)
                mu = c.amb_gain * c.signal * (mu / nrm if nrm > 1e-8 else mu)
                # P(S): pick primary c ~ prior, then |S|-1 others uniformly
                m = len(S)
                p_m = c.amb_m_probs[0] if m == 2 else c.amb_m_probs[1]
                n_other = math.comb(K - 1, m - 1) if K > 1 else 1
                p_s = sum(prior[k] for k in S) / n_other * p_m
                plabel = np.zeros(K)
                plabel[list(S)] = 1.0 / m
                out.append((p_s, mu, plabel))
            return out
        if reg == "null":
            return [(1.0, np.zeros(c.feat_dim), np.full(K, 1.0 / K))]
        # single-class regimes (easy/noisy/weak + all OOD modes)
        gain = c.weak_gain if reg == "weak" else 1.0
        out = []
        for k in range(K):
            plabel = np.zeros(K)
            plabel[k] = 1.0
            out.append((prior[k], gain * proto[:, k], plabel))
        return out

    def _obs_mean(self, env: U0Env) -> np.ndarray:
        x = env.samples.mean(axis=0)
        lat = env.latent
        if lat.get("mask") is not None:
            # restrict the likelihood to observed dims by projecting mus
            x = x.copy()
        return x

    def _project(self, mu: np.ndarray, env: U0Env) -> np.ndarray:
        mask = env.latent.get("mask")
        if mask is None:
            return mu
        return mu * mask                    # missing dims carry no signal

    def posterior(self, env: U0Env) -> np.ndarray:
        lat = env.latent
        x = self._obs_mean(env)
        n = max(1, env.n)
        sigma = lat["sigma"]
        K = env.cfg.n_classes
        logp = np.full(K, -np.inf)
        for prior, mu, plabel in self._hyps(env):
            mu_p = self._project(mu, env)
            ll = n * _log_gauss(x, mu_p, sigma, self._prec)
            for k in range(K):
                if plabel[k] > 0:
                    v = np.log(max(prior, 1e-12) * plabel[k]) + ll
                    logp[k] = np.logaddexp(logp[k], v)
        logp -= logp.max()
        p = np.exp(logp)
        return p / p.sum()

    # -- planning: achievable accuracy vs n ---------------------------
    def acc_at(self, env: U0Env, n: int) -> float:
        lat = env.latent
        key = (lat["regime"], tuple(lat["S"]), round(lat["sigma"], 4), n,
               None if lat.get("mask") is None else lat["mask"].tobytes())
        if key in self._acc_cache:
            return self._acc_cache[key]
        c = env.cfg
        mu = lat["mu"]
        sigma = lat["sigma"]
        mask = lat.get("mask")
        accs = []
        for _ in range(self.n_mc):
            x = mu + sigma * self.rng.normal(size=c.feat_dim)
            if mask is not None:
                x = x * mask
            fake = {"n": n, "mean": x}
            # posterior with n samples concentrated at x ~ xbar_n
            p = self._posterior_at(env, x, n)
            accs.append(float(p.max()))
        acc = float(np.mean(accs))
        self._acc_cache[key] = acc
        return acc

    def _posterior_at(self, env: U0Env, x: np.ndarray, n: int) -> np.ndarray:
        lat = env.latent
        sigma = lat["sigma"]
        K = env.cfg.n_classes
        logp = np.full(K, -np.inf)
        for prior, mu, plabel in self._hyps(env):
            mu_p = self._project(mu, env)
            ll = n * _log_gauss(x, mu_p, sigma, self._prec)
            for k in range(K):
                if plabel[k] > 0:
                    v = np.log(max(prior, 1e-12) * plabel[k]) + ll
                    logp[k] = np.logaddexp(logp[k], v)
        logp -= logp.max()
        p = np.exp(logp)
        return p / p.sum()

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
        if self._plan is None:
            self._plan = self.plan(env)
        self._post = self.posterior(env)
        if self._plan == 0:
            return env.cfg.abstain_id
        if env.n < self._plan:
            return env.cfg.observe_id
        return int(np.argmax(self._post))

    def reset(self) -> None:
        self._plan = None

    # recorded u for analysis: the oracle's TRUE posterior confidence
    def u_env(self, env: U0Env) -> np.ndarray:
        top2 = np.sort(self._post)[::-1]
        return np.array([self._post.max(), top2[0] - top2[1],
                         min(env.samples.var(axis=0, ddof=1).mean()
                             if env.n >= 2 else 0.0, 3.0),
                         env.n / env.cfg.max_obs])
