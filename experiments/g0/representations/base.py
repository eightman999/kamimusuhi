"""Representation interface + analytic baselines.

A representation maps episodes to per-step latents:

    Z = rep.encode(obs, act)     # obs (E,T,D) -> (E,T,H) float
    c = rep.codes(obs, act)      # -> (E,T,) int or None if not discrete

Everything is numpy at this interface; torch models are wrapped.
"""

from __future__ import annotations

from typing import Optional

import numpy as np
import torch

from ..env.dynamics import N_ACTIONS


class Rep:
    name = "rep"
    latent_dim = 0
    discrete = False

    def encode(self, obs: np.ndarray, act: np.ndarray) -> np.ndarray:
        raise NotImplementedError

    def codes(self, obs: np.ndarray, act: np.ndarray) -> Optional[np.ndarray]:
        return None


class RawRep(Rep):
    name = "raw"

    def __init__(self, obs_dim: int):
        self.latent_dim = obs_dim

    def encode(self, obs, act):
        return obs.astype(np.float64)


def _window_features(obs: np.ndarray, act: np.ndarray, w: int,
                     use_act: bool = True) -> np.ndarray:
    """Concat the last w steps of [obs, onehot(act)] per position."""
    e, t, d = obs.shape
    feat = [obs]
    if use_act:
        oh = np.zeros((e, t, N_ACTIONS))
        oh[np.arange(e)[:, None], np.arange(t)[None], act] = 1.0
        feat.append(oh)
    x = np.concatenate(feat, axis=-1)
    pad = np.zeros((e, w - 1, x.shape[-1]))
    xp = np.concatenate([pad, x], axis=1)          # (E, T+w-1, F)
    wins = np.stack([xp[:, i:i + t] for i in range(w)], axis=2)
    return wins.reshape(e, t, w * x.shape[-1])


class WindowRep(Rep):
    """Sliding-window concat of obs (+action one-hots)."""

    def __init__(self, obs_dim: int, window: int = 8, use_act: bool = True):
        self.name = "raw_win"
        self.window = window
        self.use_act = use_act
        self.latent_dim = window * (obs_dim +
                                    (N_ACTIONS if use_act else 0))

    def encode(self, obs, act):
        return _window_features(obs.astype(np.float64), act,
                                self.window, self.use_act)


class PCARep(Rep):
    """PCA on per-step obs (window=None) or on windowed features."""

    def __init__(self, obs_dim: int, n_comp: int = 8,
                 window: Optional[int] = None, use_act: bool = True):
        self.obs_dim = obs_dim
        self.n_comp = n_comp
        self.window = window
        self.use_act = use_act
        self.name = "pca" if window is None else "pca_win"
        self.latent_dim = n_comp
        self._mean = None
        self._comps = None

    def _featurize(self, obs, act):
        if self.window is None:
            return obs.astype(np.float64)
        return _window_features(obs.astype(np.float64), act,
                                self.window, self.use_act)

    def fit(self, train_obs: np.ndarray, train_act: np.ndarray) -> "PCARep":
        x = self._featurize(train_obs, train_act)
        x = x.reshape(-1, x.shape[-1])
        self._mean = x.mean(axis=0)
        xc = x - self._mean
        _, _, vt = np.linalg.svd(xc, full_matrices=False)
        self._comps = vt[: self.n_comp].T
        return self

    def encode(self, obs, act):
        x = self._featurize(obs, act)
        return (x - self._mean) @ self._comps


class TorchRep(Rep):
    """Wraps a trained checkpoint's latent output."""

    def __init__(self, name: str, model, device: str = "cpu"):
        self.name = name
        self.model = model.to(device).eval()
        self.device = device
        self.latent_dim = model.latent_dim
        self.discrete = hasattr(model, "vq")

    def encode(self, obs, act):
        with torch.no_grad():
            o = torch.as_tensor(obs, dtype=torch.float32,
                                device=self.device)
            a = torch.as_tensor(act, dtype=torch.long, device=self.device)
            _, lat, _ = self.model(o, a)
        return lat.cpu().numpy().astype(np.float64)

    def codes(self, obs, act):
        if not self.discrete:
            return None
        with torch.no_grad():
            o = torch.as_tensor(obs, dtype=torch.float32,
                                device=self.device)
            a = torch.as_tensor(act, dtype=torch.long, device=self.device)
            _, _, aux = self.model(o, a)
        return aux["codes"].cpu().numpy()


class KMeansRep(Rep):
    """Post-hoc discretization: k-means over a base rep's latents.

    Encode returns the one-hot cluster assignment (clearly bottlenecked).
    """

    def __init__(self, base: Rep, k: int = 16, seed: int = 0):
        self.base = base
        self.k = k
        self.seed = seed
        self.name = f"{base.name}_km"
        self.latent_dim = k
        self.discrete = True
        self._centers = None

    def fit(self, train_obs: np.ndarray, train_act: np.ndarray,
            max_points: int = 20000) -> "KMeansRep":
        from ..probes.clustering import kmeans
        z = self.base.encode(train_obs, train_act)
        z = z.reshape(-1, z.shape[-1])
        if z.shape[0] > max_points:
            idx = np.random.default_rng(self.seed).choice(
                z.shape[0], max_points, replace=False)
            z = z[idx]
        self._centers, _ = kmeans(z, self.k, seed=self.seed)
        return self

    def _assign(self, z: np.ndarray) -> np.ndarray:
        d = ((z[..., None, :] - self._centers) ** 2).sum(-1)
        return d.argmin(-1)

    def encode(self, obs, act):
        z = self.base.encode(obs, act)
        c = self._assign(z)
        oh = np.zeros(z.shape[:-1] + (self.k,))
        np.put_along_axis(oh, c[..., None], 1.0, axis=-1)
        return oh

    def codes(self, obs, act):
        return self._assign(self.base.encode(obs, act))


TRAINED_REP_NAMES = ("ae", "ae_vq", "gru", "gru_vq")
ANALYTIC_REP_NAMES = ("raw", "raw_win", "pca", "pca_win")
ALL_REP_NAMES = ANALYTIC_REP_NAMES + TRAINED_REP_NAMES + ("gru_km",)


def build_analytic_rep(name: str, obs_dim: int, mcfg, train_ds=None) -> Rep:
    if name == "raw":
        return RawRep(obs_dim)
    if name == "raw_win":
        return WindowRep(obs_dim, window=mcfg.window)
    if name == "pca":
        return PCARep(obs_dim, n_comp=mcfg.pca_dim).fit(
            train_ds["obs"], train_ds["actions"])
    if name == "pca_win":
        return PCARep(obs_dim, n_comp=mcfg.pca_win_dim,
                      window=mcfg.window).fit(train_ds["obs"],
                                              train_ds["actions"])
    raise KeyError(name)


def build_trained_rep(name: str, model, device: str = "cpu") -> Rep:
    return TorchRep(name, model, device)


def build_kmeans_rep(base_name: str, model, device: str, mcfg,
                     train_ds, seed: int = 0) -> Rep:
    base = TorchRep(base_name, model, device)
    return KMeansRep(base, k=mcfg.kmeans_k, seed=seed).fit(
        train_ds["obs"], train_ds["actions"])
