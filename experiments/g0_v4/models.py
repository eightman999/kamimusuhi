"""G0-v4 models — one recurrent latent interface, four objectives.

Unified interface (compatible with `experiments.g0.representations.
TorchRep` and the G0 eval battery):

    pred, latent, aux = model(obs, act)
        obs    (E,T,D) float, act (E,T) long
        pred   (E,T,D) aux next-obs readout  (obs + head(latent))
        latent (E,T,H) per-step representation used by probes
        aux    dict

Every model carries a linear `head` (latent -> obs) trained with a small
auxiliary weight on the next-obs delta. Its ONLY role is to keep the G0
`intervention` (decode-delta selectivity) and `base_mse` metrics
computable; the representation objective itself is self-supervised in
latent space (InfoNCE / VICReg / JEPA) or, for the control, the G0 GRU
next-obs predictor.

  * gru     — control: `experiments.g0.models.net.GRUPredModel`
  * cpc     — z_t = step encoder, c_t = GRU(z, a); W_k c_t predicts
              z_{t+k} via InfoNCE over in-batch negatives
  * vicreg  — h_t = GRU(obs, a); two augmented views -> invariance +
              variance + covariance; plus latent forward prediction
  * jepa    — online GRU context encoder c_t; EMA target encoder;
              action-conditioned latent rollout predicts target latents
              t+1..t+K; variance/covariance collapse guard
"""

from __future__ import annotations

import copy
from typing import Optional

import torch
import torch.nn as nn
import torch.nn.functional as F

from experiments.g0.models.net import GRUPredModel

from .augment import make_view
from .losses import (covariance_penalty, default_next_loss,
                     infonce_future, variance_hinge, vicreg_terms)


class V4Base(nn.Module):
    """Shared plumbing: action-conditioned GRU encoder + aux readout."""

    kind = "next"          # pred approximates next_obs (aux readout only)
    is_recurrent = True
    use_action = True

    def _init_common(self, obs_dim: int, n_actions: int, hidden: int):
        self.obs_dim = obs_dim
        self.n_actions = n_actions
        self.latent_dim = hidden
        self.head = nn.Linear(hidden, obs_dim)   # aux readout for metrics

    def _readout(self, obs: torch.Tensor, lat: torch.Tensor
                 ) -> torch.Tensor:
        return obs + self.head(lat)

    def _readout_loss(self, obs: torch.Tensor, nxt: torch.Tensor,
                      lat: torch.Tensor) -> torch.Tensor:
        return F.mse_loss(self._readout(obs, lat), nxt)


class CPCModel(V4Base):
    """Contrastive predictive coding.

    z_t = step_enc(obs_t); c_t = GRU([z_t, a_t]). For each horizon k,
    f_k(c_t) must score the true z_{t+k} above all other step embeddings
    in the batch (episodes + timesteps) — InfoNCE.
    """

    def __init__(self, obs_dim: int, n_actions: int, hidden: int = 64,
                 z_dim: int = 64, horizon: int = 8,
                 temperature: float = 0.1, readout_weight: float = 0.1):
        super().__init__()
        self._init_common(obs_dim, n_actions, hidden)
        self.z_dim = z_dim
        self.horizon = horizon
        self.temperature = temperature
        self.readout_weight = readout_weight
        self.step_enc = nn.Sequential(
            nn.Linear(obs_dim, z_dim), nn.ReLU(),
            nn.Linear(z_dim, z_dim))
        self.gru = nn.GRU(z_dim + n_actions, hidden, batch_first=True)
        self.pred_k = nn.ModuleList(
            nn.Linear(hidden, z_dim, bias=False) for _ in range(horizon))

    def encode(self, obs, act, h0=None):
        z = self.step_enc(obs)
        x = torch.cat([z, F.one_hot(act, self.n_actions).float()], -1)
        c, _ = self.gru(x, h0)
        return c, z

    def forward(self, obs, act, h0=None):
        c, _ = self.encode(obs, act, h0)
        return self._readout(obs, c), c, {}

    def compute_loss(self, obs, nxt, act, cfg, gen=None):
        c, z = self.encode(obs, act)
        e, t, zd = z.shape
        z_all = z.reshape(e * t, zd)
        device = z.device
        ep_off = (torch.arange(e, device=device) * t).unsqueeze(1)
        total, logs = 0.0, {}
        n_terms = 0
        for k in range(1, self.horizon + 1):
            ctx = c[:, : t - k].reshape(-1, self.latent_dim)
            pred = self.pred_k[k - 1](ctx)
            targets = (ep_off + torch.arange(t - k, device=device)
                       .unsqueeze(0) + k).reshape(-1)
            lk = infonce_future(pred, z_all, targets, self.temperature)
            total = total + lk
            n_terms += 1
        nce = total / n_terms
        ro = self._readout_loss(obs, nxt, c)
        loss = nce + self.readout_weight * ro
        logs.update({"infonce": nce.item(), "readout": ro.item(),
                     "loss": loss.item()})
        return loss, logs


class VICRegModel(V4Base):
    """Temporal VICReg + latent forward prediction.

    Two views of each episode (obs jitter + sensor-dim dropout — no
    oracle context transform) are encoded; per-step embeddings are pulled
    together (invariance), spread (variance hinge), decorrelated
    (covariance), and made forward-predictable in latent space.
    """

    def __init__(self, obs_dim: int, n_actions: int, hidden: int = 64,
                 readout_weight: float = 0.1):
        super().__init__()
        self._init_common(obs_dim, n_actions, hidden)
        self.readout_weight = readout_weight
        self.gru = nn.GRU(obs_dim + n_actions, hidden, batch_first=True)
        # unbounded projection: the probed latent. VICReg's variance
        # hinge needs an unbounded space — on tanh-bounded h it forced
        # saturation to +-1 and rank-1 collapse (pilot finding).
        self.proj = nn.Linear(hidden, hidden)
        self.pred_net = nn.Sequential(
            nn.Linear(hidden + n_actions, hidden), nn.ReLU(),
            nn.Linear(hidden, hidden))

    def encode(self, obs, act, h0=None):
        x = torch.cat([obs, F.one_hot(act, self.n_actions).float()], -1)
        h, _ = self.gru(x, h0)
        return self.proj(h)

    def forward(self, obs, act, h0=None):
        z = self.encode(obs, act, h0)
        return self._readout(obs, z), z, {}

    def compute_loss(self, obs, nxt, act, cfg, gen=None):
        v = cfg.v4
        v1 = make_view(obs, v.aug_jitter, v.aug_drop, gen)
        v2 = make_view(obs, v.aug_jitter, v.aug_drop, gen)
        h1 = self.encode(v1, act)
        h2 = self.encode(v2, act)
        vic, logs = vicreg_terms(h1, h2, v.vicreg_sim, v.vicreg_var,
                                 v.vicreg_cov, v.vicreg_gamma)
        # latent forward model: (z_t, a_{t+1}) -> z_{t+1}, stop-grad
        act_next = F.one_hot(act[:, 1:], self.n_actions).float()
        pred_h = self.pred_net(torch.cat([h1[:, :-1], act_next], -1))
        lpred = F.mse_loss(pred_h, h1[:, 1:].detach())
        ro = self._readout_loss(obs, nxt, h1)
        loss = vic + v.vicreg_pred * lpred + self.readout_weight * ro
        logs.update({"pred": lpred.item(), "readout": ro.item(),
                     "loss": loss.item()})
        return loss, logs


class JEPAModel(V4Base):
    """JEPA-style latent predictor.

    Online encoder c_t = GRU([obs_t, a_t]); target encoder is an EMA
    copy. A GRUCell predictor rolls c_t forward over the *observed*
    action stream a_t..a_{t+K-1}, predicting target latents z_{t+1..t+K}
    in latent space (never pixels). Mild variance/covariance guard
    against collapse.
    """

    def __init__(self, obs_dim: int, n_actions: int, hidden: int = 64,
                 horizon: int = 8, ema_decay: float = 0.99,
                 readout_weight: float = 0.1):
        super().__init__()
        self._init_common(obs_dim, n_actions, hidden)
        self.horizon = horizon
        self.ema_decay = ema_decay
        self.readout_weight = readout_weight
        self.gru = nn.GRU(obs_dim + n_actions, hidden, batch_first=True)
        # unbounded projection = the probed latent (EMA'd with the GRU);
        # same anti-collapse rationale as VICRegModel.proj
        self.proj = nn.Linear(hidden, hidden)
        self.target_gru = copy.deepcopy(self.gru)
        self.target_proj = copy.deepcopy(self.proj)
        for mod in (self.target_gru, self.target_proj):
            for p in mod.parameters():
                p.requires_grad_(False)
        self.cell = nn.GRUCell(n_actions, hidden)
        self.pred_proj = nn.Linear(hidden, hidden)

    def _run_enc(self, gru, proj, obs, act, h0=None):
        x = torch.cat([obs, F.one_hot(act, self.n_actions).float()], -1)
        h, _ = gru(x, h0)
        return proj(h)

    def encode(self, obs, act, h0=None):
        return self._run_enc(self.gru, self.proj, obs, act, h0)

    def encode_target(self, obs, act):
        return self._run_enc(self.target_gru, self.target_proj, obs, act)

    @torch.no_grad()
    def ema_update(self):
        m = self.ema_decay
        for pt, po in zip(self.target_gru.parameters(),
                          self.gru.parameters()):
            pt.mul_(m).add_(po.detach(), alpha=1.0 - m)
        for pt, po in zip(self.target_proj.parameters(),
                          self.proj.parameters()):
            pt.mul_(m).add_(po.detach(), alpha=1.0 - m)

    def forward(self, obs, act, h0=None):
        c = self.encode(obs, act, h0)
        return self._readout(obs, c), c, {}

    def compute_loss(self, obs, nxt, act, cfg, gen=None):
        v = cfg.v4
        e, t, _ = obs.shape
        k_h = min(self.horizon, t - 1)
        c = self.encode(obs, act)
        with torch.no_grad():
            zt = self.encode_target(obs, act)
        s = t - k_h                       # start positions 0..T-K-1
        h = c[:, :s].reshape(e * s, self.latent_dim)
        total, n_terms = 0.0, 0
        for k in range(k_h):
            a_k = F.one_hot(act[:, k: k + s], self.n_actions) \
                .reshape(e * s, self.n_actions).float()
            h = self.cell(a_k, h)
            pred = self.pred_proj(h)
            tgt = zt[:, k + 1: k + 1 + s].reshape(e * s, -1).detach()
            total = total + F.mse_loss(pred, tgt)
            n_terms += 1
        lpred = total / n_terms
        cf = c.reshape(-1, self.latent_dim)
        var = variance_hinge(cf, 1.0)
        cov = covariance_penalty(cf)
        ro = self._readout_loss(obs, nxt, c)
        loss = lpred + v.jepa_var * var + v.jepa_cov * cov \
            + self.readout_weight * ro
        logs = {"jepa": lpred.item(), "var": var.item(),
                "cov": cov.item(), "readout": ro.item(),
                "loss": loss.item()}
        return loss, logs


# name -> builder. `gru` is the G0 predictor control (same class as
# experiments.g0 MODEL_REGISTRY["gru"], sized by v4.hidden).
MODEL_REGISTRY_V4 = ("gru", "cpc", "vicreg", "jepa")


def build_model(name: str, cfg, obs_dim: int, n_actions: int):
    v = cfg.v4
    if name == "gru":
        return GRUPredModel(obs_dim=obs_dim, n_actions=n_actions,
                            hidden=v.hidden,
                            target_delta=cfg.train.target_delta)
    if name == "cpc":
        return CPCModel(obs_dim, n_actions, hidden=v.hidden,
                        z_dim=v.z_dim, horizon=v.horizon,
                        temperature=v.temperature,
                        readout_weight=v.readout_weight)
    if name == "vicreg":
        return VICRegModel(obs_dim, n_actions, hidden=v.hidden,
                           readout_weight=v.readout_weight)
    if name == "jepa":
        return JEPAModel(obs_dim, n_actions, hidden=v.hidden,
                         horizon=v.horizon, ema_decay=v.ema_decay,
                         readout_weight=v.readout_weight)
    raise KeyError(name)


def compute_loss(model, name: str, obs, nxt, act, cfg, gen=None):
    """Dispatch to the method's self-supervised loss."""
    if hasattr(model, "compute_loss"):
        return model.compute_loss(obs, nxt, act, cfg, gen)
    return default_next_loss(model, obs, nxt, act)


def model_spec(name: str, cfg) -> dict:
    v = cfg.v4
    return {"name": name, "hidden": v.hidden, "z_dim": v.z_dim,
            "horizon": v.horizon, "temperature": v.temperature,
            "ema_decay": v.ema_decay,
            "readout_weight": v.readout_weight,
            "target_delta": cfg.train.target_delta,
            "vicreg": dict(sim=v.vicreg_sim, var=v.vicreg_var,
                           cov=v.vicreg_cov, pred=v.vicreg_pred,
                           gamma=v.vicreg_gamma),
            "jepa_guard": dict(var=v.jepa_var, cov=v.jepa_cov),
            "aug": dict(jitter=v.aug_jitter, drop=v.aug_drop)}
