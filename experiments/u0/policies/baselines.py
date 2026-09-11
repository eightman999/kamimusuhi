"""Baseline policies for U0.

All baselines share the Policy interface (u / probs / decide).  The
gated baselines use the same three-way rule on their own uncertainty
signal: answer when confident, abstain when hopeless, observe while
uncertain and budget remains -- thresholds tuned on validation episodes
by ``tune_gates`` so each baseline is shown at its best.

    always_answer    never uses u (degenerate control)
    always_observe   observes to budget then answers (degenerate control)
    always_abstain   never answers (degenerate control)
    random           uniform random meta-action each step
    threshold        answer iff max_prob >= tau else abstain
    entropy_gate     softmax-entropy gated answer/observe/abstain
    ensemble         3-net vote-share gated
    mcdropout        MC-dropout predictive-entropy gated
    conf_gated       learned ConfNet gated (rule, not learned policy)
"""

from __future__ import annotations

import numpy as np
import torch

from ..env.uncertainty_env import U0Config
from ..models.nets import U_CONF, U_MARGIN, U_N, U_VAR
from .base import Policy, u_from_probs

META_ANSWER, META_OBSERVE, META_ABSTAIN = 0, 1, 2


class AlwaysAnswer(Policy):
    name = "always_answer"
    uses_u = False

    def __init__(self, clf, cfg: U0Config):
        self.clf, self.cfg = clf, cfg

    def u(self, obs):
        with torch.no_grad():
            probs = torch.softmax(self.clf(obs), -1)
        return u_from_probs(probs, probs.max(-1).values, obs,
                            self.cfg.feat_dim)

    def probs(self, obs):
        with torch.no_grad():
            return torch.softmax(self.clf(obs), -1)

    def decide(self, obs, u):
        with torch.no_grad():
            return self.clf(obs).argmax(-1)


class AlwaysAbstain(Policy):
    name = "always_abstain"
    uses_u = False

    def __init__(self, cfg: U0Config):
        self.cfg = cfg

    def u(self, obs):
        return torch.zeros(obs.shape[0], 4)

    def probs(self, obs):
        return torch.full((obs.shape[0], self.cfg.n_classes),
                          1.0 / self.cfg.n_classes)

    def decide(self, obs, u):
        return torch.full((obs.shape[0],), self.cfg.abstain_id,
                          dtype=torch.long)


class AlwaysObserve(AlwaysAnswer):
    """Observe until the budget is exhausted, then answer."""
    name = "always_observe"

    def decide(self, obs, u):
        n = obs[:, self.cfg.feat_dim + 1]
        ans = super().decide(obs, u)
        return torch.where(n < 1.0,
                           torch.full_like(ans, self.cfg.observe_id), ans)


class RandomPolicy(Policy):
    name = "random"
    uses_u = False

    def __init__(self, clf, cfg: U0Config, seed: int = 0):
        self.clf, self.cfg = clf, cfg
        self.gen = torch.Generator().manual_seed(seed)

    def u(self, obs):
        return torch.zeros(obs.shape[0], 4)

    def probs(self, obs):
        with torch.no_grad():
            return torch.softmax(self.clf(obs), -1)

    def decide(self, obs, u):
        meta = torch.randint(0, 3, (obs.shape[0],), generator=self.gen)
        ans = self.clf(obs).argmax(-1)
        out = torch.where(meta == META_ANSWER, ans,
                          torch.where(meta == META_OBSERVE,
                                      torch.full_like(ans, self.cfg.observe_id),
                                      torch.full_like(ans, self.cfg.abstain_id)))
        # at the budget boundary OBSERVE degrades to abstain anyway
        return out


class _GatedPolicy(Policy):
    """Answer if conf >= tau_a; abstain if conf <= tau_b; else observe
    while budget remains (the gated instantiation of a confidence rule).
    ``three_way=False`` collapses it to a pure selective-answer rule
    (conf >= tau_a -> answer, else abstain; never observes)."""

    three_way = True

    def __init__(self, cfg: U0Config, tau_a: float = 0.8,
                 tau_b: float = 0.35):
        self.cfg, self.tau_a, self.tau_b = cfg, tau_a, tau_b

    def _conf(self, obs) -> torch.Tensor:
        raise NotImplementedError

    def u(self, obs):
        conf = self._conf(obs)
        return u_from_probs(self.probs(obs), conf, obs, self.cfg.feat_dim)

    def decide(self, obs, u):
        conf = u[:, U_CONF]
        n = obs[:, self.cfg.feat_dim + 1]
        ans = self.probs(obs).argmax(-1)
        abstain = torch.full_like(ans, self.cfg.abstain_id)
        if not self.three_way:
            return torch.where(conf >= self.tau_a, ans, abstain)
        return torch.where(
            conf >= self.tau_a, ans,
            torch.where(conf <= self.tau_b, abstain,
                        torch.where(n < 1.0,
                                    torch.full_like(ans, self.cfg.observe_id),
                                    abstain)))


class ThresholdPolicy(_GatedPolicy):
    """Fixed confidence threshold on softmax max-prob (never observes)."""

    name = "threshold"
    three_way = False

    def __init__(self, clf, cfg: U0Config, **kw):
        super().__init__(cfg, **kw)
        self.clf = clf

    def _conf(self, obs):
        return self.probs(obs).max(-1).values

    def probs(self, obs):
        with torch.no_grad():
            return torch.softmax(self.clf(obs), -1)


class EntropyGate(_GatedPolicy):
    """Softmax-entropy gate: conf = 1 - H/H_max."""

    name = "entropy"

    def __init__(self, clf, cfg: U0Config, **kw):
        super().__init__(cfg, **kw)
        self.clf = clf

    def _conf(self, obs):
        p = self.probs(obs)
        h = -(p * torch.log(p.clamp_min(1e-12))).sum(-1)
        return 1.0 - h / np.log(self.cfg.n_classes)

    def probs(self, obs):
        with torch.no_grad():
            return torch.softmax(self.clf(obs), -1)


class EnsemblePolicy(_GatedPolicy):
    """Deep-ensemble uncertainty: conf = vote share of the modal class."""

    name = "ensemble"

    def __init__(self, members, cfg: U0Config, **kw):
        super().__init__(cfg, **kw)
        self.members = members

    def _member_probs(self, obs):
        with torch.no_grad():
            return torch.stack([torch.softmax(m(obs), -1)
                                for m in self.members])     # (M,B,K)

    def probs(self, obs):
        return self._member_probs(obs).mean(0)

    def _conf(self, obs):
        votes = self._member_probs(obs).argmax(-1)          # (M,B)
        K = self.cfg.n_classes
        share = torch.stack([(votes == k).float().mean(0)
                             for k in range(K)], -1).max(-1).values
        return share


class MCDropoutPolicy(_GatedPolicy):
    """MC-dropout uncertainty: conf = 1 - predictive entropy / log K."""

    name = "mcdropout"

    def __init__(self, model, cfg: U0Config, passes: int = 12, **kw):
        super().__init__(cfg, **kw)
        self.model, self.passes = model, passes
        model.eval()
        model.enable_dropout()

    def _pass_probs(self, obs):
        with torch.no_grad():
            return torch.stack([torch.softmax(self.model(obs), -1)
                                for _ in range(self.passes)])

    def probs(self, obs):
        return self._pass_probs(obs).mean(0)

    def _conf(self, obs):
        p = self._pass_probs(obs).mean(0)
        h = -(p * torch.log(p.clamp_min(1e-12))).sum(-1)
        return 1.0 - h / np.log(self.cfg.n_classes)


class ConfGated(_GatedPolicy):
    """Learned confidence head + fixed rule (not a learned policy)."""

    name = "conf_gated"

    def __init__(self, clf, conf_net, cfg: U0Config, **kw):
        super().__init__(cfg, **kw)
        self.clf, self.conf_net = clf, conf_net

    def _conf(self, obs):
        with torch.no_grad():
            return self.conf_net(obs)

    def probs(self, obs):
        with torch.no_grad():
            return torch.softmax(self.clf(obs), -1)


class LearnedPolicy(Policy):
    """The learned agent: frozen classifier + conf net feed the u vector;
    a small PPO-trained meta-policy turns u into ANSWER/OBSERVE/ABSTAIN."""

    name = "learned"

    def __init__(self, clf, conf_net, meta, cfg: U0Config,
                 deterministic: bool = True):
        self.clf, self.conf_net, self.meta, self.cfg = clf, conf_net, meta, cfg
        self.deterministic = deterministic

    def u(self, obs):
        from ..models.nets import uncertainty_features
        with torch.no_grad():
            logits = self.clf(obs)
            conf = self.conf_net(obs)
        return uncertainty_features(logits, conf, obs, self.cfg.feat_dim)

    def probs(self, obs):
        with torch.no_grad():
            return torch.softmax(self.clf(obs), -1)

    def decide(self, obs, u):
        with torch.no_grad():
            logits, _ = self.meta(u)
            meta = logits.argmax(-1) if self.deterministic else \
                torch.distributions.Categorical(logits=logits).sample()
        ans = self.probs(obs).argmax(-1)
        return torch.where(
            meta == META_ANSWER, ans,
            torch.where(meta == META_OBSERVE,
                        torch.full_like(ans, self.cfg.observe_id),
                        torch.full_like(ans, self.cfg.abstain_id)))


def tune_gates(policies: list, bundle: dict, cfg: U0Config,
               episodes: int, seed: int, device: str = "cpu") -> dict:
    """Grid-search (tau_a, tau_b) per gated policy on validation episodes.

    Each gated policy is evaluated at every grid point; the pair with the
    best mean reward wins.  Returns {policy_name: (tau_a, tau_b)}.
    """
    from ..evaluate import run_eval  # local import to avoid cycle

    best = {}
    grid_a = np.linspace(0.55, 0.99, 12)
    grid_b = np.linspace(-0.05, 0.60, 12)
    for pol in policies:
        top, top_score = (pol.tau_a, pol.tau_b), -np.inf
        for ta in grid_a:
            for tb in (grid_b if pol.three_way else [ta]):
                if pol.three_way and tb >= ta:
                    continue
                pol.tau_a, pol.tau_b = float(ta), float(tb)
                res = run_eval(pol, cfg, episodes=episodes, seed=seed,
                               device=device, batch=64)
                if res["metrics"]["mean_reward"] > top_score:
                    top_score = res["metrics"]["mean_reward"]
                    top = (float(ta), float(tb))
        pol.tau_a, pol.tau_b = top
        best[pol.name] = top
    return best
