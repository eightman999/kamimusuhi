"""Model variants for G0.

Unified interface:  pred, latent, aux = model(obs, act)
    obs   (E,T,D) float, act (E,T) long
    pred  (E,T,D) — reconstruction for kind=="recon", next-obs for "next"
    latent(E,T,H) — representation used by probes
    aux   dict    — "loss_extra" (added to train loss), "codes" (VQ)

Kinds:
    ae     : MLP autoencoder, bottleneck latent (instantaneous)
    ae_vq  : VQ-VAE, discrete bottleneck latent (instantaneous)
    gru    : action-conditioned GRU next-obs predictor, latent = hidden
    gru_vq : same but hidden -> VQ codebook -> head (discrete predictive)
"""

from __future__ import annotations

import torch
import torch.nn as nn
import torch.nn.functional as F

from .vq import VectorQuantizerEMA


class AEModel(nn.Module):
    kind = "recon"
    is_recurrent = False
    use_action = False

    def __init__(self, obs_dim: int, hidden: int = 64, latent: int = 8):
        super().__init__()
        self.obs_dim = obs_dim
        self.encoder = nn.Sequential(
            nn.Linear(obs_dim, hidden), nn.ReLU(),
            nn.Linear(hidden, hidden), nn.ReLU(),
            nn.Linear(hidden, latent))
        self.decoder = nn.Sequential(
            nn.Linear(latent, hidden), nn.ReLU(),
            nn.Linear(hidden, obs_dim))
        self.latent_dim = latent

    def forward(self, obs, act=None, h0=None):
        z = self.encoder(obs)
        return self.decoder(z), z, {}


class AEVQModel(nn.Module):
    kind = "recon"
    is_recurrent = False
    use_action = False

    def __init__(self, obs_dim: int, hidden: int = 64, latent: int = 8,
                 n_codes: int = 24, commitment: float = 0.25):
        super().__init__()
        self.obs_dim = obs_dim
        self.encoder = nn.Sequential(
            nn.Linear(obs_dim, hidden), nn.ReLU(),
            nn.Linear(hidden, hidden), nn.ReLU(),
            nn.Linear(hidden, latent))
        self.vq = VectorQuantizerEMA(n_codes, latent, commitment)
        self.decoder = nn.Sequential(
            nn.Linear(latent, hidden), nn.ReLU(),
            nn.Linear(hidden, obs_dim))
        self.latent_dim = latent

    def forward(self, obs, act=None, h0=None):
        z = self.encoder(obs)
        z_q_st, codes, vq_loss, z_q = self.vq(z)
        pred = self.decoder(z_q_st)
        return pred, z_q, {"loss_extra": vq_loss, "codes": codes}


class GRUPredModel(nn.Module):
    """Action-conditioned next-observation predictor; latent = h_t."""

    kind = "next"
    is_recurrent = True
    use_action = True

    def __init__(self, obs_dim: int, n_actions: int, hidden: int = 32,
                 target_delta: bool = True):
        super().__init__()
        self.obs_dim = obs_dim
        self.n_actions = n_actions
        self.target_delta = target_delta
        self.gru = nn.GRU(obs_dim + n_actions, hidden, batch_first=True)
        self.head = nn.Linear(hidden, obs_dim)
        self.latent_dim = hidden

    def forward(self, obs, act, h0=None):
        x = torch.cat([obs, F.one_hot(act, self.n_actions).float()], -1)
        h, hT = self.gru(x, h0)
        out = self.head(h)
        pred = obs + out if self.target_delta else out
        return pred, h, {}


class GRUVQModel(nn.Module):
    """GRU -> VQ bottleneck -> next-obs head.

    Prediction passes through a discrete code, forcing the recurrent
    state to be summarized by one of K reusable symbols.
    """

    kind = "next"
    is_recurrent = True
    use_action = True

    def __init__(self, obs_dim: int, n_actions: int, hidden: int = 32,
                 n_codes: int = 24, commitment: float = 0.25,
                 target_delta: bool = True):
        super().__init__()
        self.obs_dim = obs_dim
        self.n_actions = n_actions
        self.target_delta = target_delta
        self.gru = nn.GRU(obs_dim + n_actions, hidden, batch_first=True)
        self.vq = VectorQuantizerEMA(n_codes, hidden, commitment)
        self.head = nn.Linear(hidden, obs_dim)
        self.latent_dim = hidden

    def forward(self, obs, act, h0=None):
        x = torch.cat([obs, F.one_hot(act, self.n_actions).float()], -1)
        h, hT = self.gru(x, h0)
        z_q_st, codes, vq_loss, z_q = self.vq(h)
        out = self.head(z_q_st)
        pred = obs + out if self.target_delta else out
        return pred, z_q, {"loss_extra": vq_loss, "codes": codes}
