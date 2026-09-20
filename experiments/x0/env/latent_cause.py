"""X0 latent-cause multimodal world.

A *scene* contains ``n_events`` events.  Each event is governed by a hidden
cause (a combination of discrete factors; never observed).  Every modality
emits one token per event: a noisy feature vector ``f_m(cause)`` delivered at
a jittered emission time ``onset + lag_{event, modality}``.

What the agent sees per scene (``Scene.observation()``)::

    {modality: {"times": (n,) int, "feats": (n, d_m) float}}

with each modality's token order independently shuffled.  The agent NEVER
sees cause ids, event ids, or pair labels.  Times are observable but cannot
uniquely identify pairs: different causes can co-occur at one time step and
same-cause emissions are lag-jittered (see tests/test_leak.py).

Scene sampling modes (``mode`` argument):

    standard   onsets ~ U{0..T-1}, lags ~ U{0..lag} (or fixed_shift)
    conflict   adversarial construction for X-C2: on the (vis, aud) pair,
               vis[e] @ 2e and aud[e] @ 2e + delta, so every co-timed
               vis/aud token belongs to *different* causes while every
               true partner is shifted by ``delta``.  Other modalities are
               jittered normally.  Timing heuristics score ~0 here.

Ground truth (cause ids, event ids, partner indices) lives on the Scene
object and is used ONLY by evaluation/tests — never by training losses.
"""

from __future__ import annotations

import itertools
from dataclasses import dataclass, field
from typing import Dict, List, Optional, Sequence

import numpy as np

from ..modalities.transforms import ModalityTransform, make_transforms


# ----------------------------------------------------------------------
# causes
# ----------------------------------------------------------------------
class CauseSpace:
    """Compositional cause space: the cartesian product of factor values.

    Each cause's latent code is the concatenation of per-factor one-hots, so
    ``z_dim = sum(factors)``.  A corner of the joint space
    (factor[0] >= holdout[0] AND factor[1] >= holdout[1]) is reserved as
    *held-out recombinations*: every factor value still appears during
    training, only the combination is new (X-C5).
    """

    def __init__(self, factors: Sequence[int] = (4, 4, 4),
                 holdout: Sequence[int] = (2, 2)):
        self.factors = tuple(int(f) for f in factors)
        self.holdout = tuple(int(h) for h in holdout)
        self.combos = np.asarray(
            list(itertools.product(*[range(f) for f in self.factors])),
            dtype=np.int64)                                    # (K, n_factors)
        self.n_causes = len(self.combos)
        self.z_dim = int(sum(self.factors))
        # one-hot latent codes
        z = np.zeros((self.n_causes, self.z_dim))
        off = 0
        for fi, f in enumerate(self.factors):
            z[np.arange(self.n_causes), off + self.combos[:, fi]] = 1.0
            off += f
        self.codes = z
        held = np.ones(self.n_causes, dtype=bool)
        for fi, h in enumerate(self.holdout):
            held &= self.combos[:, fi] >= h
        self.held_out = held                                   # recombination set
        self.train_ids = np.where(~held)[0]
        self.holdout_ids = np.where(held)[0]

    def pool(self, which: str) -> np.ndarray:
        if which == "train":
            return self.train_ids
        if which == "holdout":
            return self.holdout_ids
        if which == "all":
            return np.arange(self.n_causes)
        raise ValueError(which)


# ----------------------------------------------------------------------
# scenes
# ----------------------------------------------------------------------
@dataclass
class Emission:
    """One modality's detections within a scene (order = internal only)."""
    times: np.ndarray    # (n,) int emission times
    feats: np.ndarray    # (n, d_m) float features
    events: np.ndarray   # (n,) event index — GROUND TRUTH, eval only


@dataclass
class Scene:
    emissions: Dict[str, Emission]
    causes: np.ndarray       # (G,) cause id per event (GROUND TRUTH)
    onsets: np.ndarray       # (G,) event onset times   (GROUND TRUTH)
    mode: str = "standard"

    # -- agent-facing observation: shuffled, label-free -------------------
    def observation(self, rng: np.random.Generator) -> Dict[str, dict]:
        obs = {}
        for name, em in self.emissions.items():
            perm = rng.permutation(len(em.times))
            obs[name] = {"times": em.times[perm].copy(),
                         "feats": em.feats[perm].copy()}
        return obs

    # -- eval helpers -----------------------------------------------------
    def partner_index(self, modality_a: str, modality_b: str):
        """For each token i of modality_a, the index j of the token of
        modality_b emitted by the same event, or -1 if that event's token
        was dropped from modality_b."""
        ev_to_j = {}
        for j, e in enumerate(self.emissions[modality_b].events):
            ev_to_j[int(e)] = j
        out = np.full(len(self.emissions[modality_a].events), -1, dtype=np.int64)
        for i, e in enumerate(self.emissions[modality_a].events):
            out[i] = ev_to_j.get(int(e), -1)
        return out


@dataclass
class SceneParams:
    n_events: int = 6
    T: int = 16                       # onset range
    lag: int = 2                      # per-modality jitter ~ U{0..lag}
    fixed_shift: Optional[int] = None # if set: modality i lag = i*shift —
                                      # every same-cause pair is separated
                                      # by a deterministic nonzero offset
    token_drop_p: float = 0.1         # per (event, modality) drop prob
    modality_drop_p: float = 0.1      # whole-modality absent per scene
    noise: float = 0.1                # gaussian read noise on features
    conflict_delta: int = 2           # partner shift in "conflict" scenes
    conflict_pair: tuple = ("vis", "aud")


# ----------------------------------------------------------------------
# environment
# ----------------------------------------------------------------------
class LatentCauseEnv:
    """Generative world.  ``reset``/``sample_scene`` replace step-based
    interaction: an episode is a single scene of co-occurring emissions."""

    def __init__(self, params: SceneParams, factors=(4, 4, 4),
                 holdout=(2, 2), modalities=("vis", "aud", "temp", "mot"),
                 seed: int = 0, alt_transforms: bool = False,
                 noise_override: Optional[float] = None):
        self.params = params
        self.seed = seed
        self.causes = CauseSpace(factors, holdout)
        self.modalities = tuple(modalities)
        self.transforms = make_transforms(self.modalities, self.causes.z_dim,
                                          seed, alt=alt_transforms)
        self.rng = np.random.default_rng(seed + 55_000)
        if noise_override is not None:
            self.params = SceneParams(**{**self.params.__dict__,
                                         "noise": noise_override})

    # ------------------------------------------------------------------
    def _emit(self, cause_ids: np.ndarray, onsets: np.ndarray,
              lags: np.ndarray, rng: np.random.Generator,
              drop_mask: np.ndarray,
              wrap_T: Optional[int] = None,
              wrap_mods: tuple = ()) -> Dict[str, Emission]:
        """cause_ids (G,), onsets (G,), lags (G, M), drop_mask (G, M).
        ``wrap_T`` wraps emission times of ``wrap_mods`` modulo wrap_T
        (used by the conflict construction to form a timing ring)."""
        p = self.params
        out: Dict[str, Emission] = {}
        for mi, m in enumerate(self.modalities):
            keep = ~drop_mask[:, mi]
            idx = np.where(keep)[0]
            if len(idx) == 0:
                out[m] = Emission(np.zeros(0, np.int64),
                                  np.zeros((0, self.transforms[m].out_dim)),
                                  np.zeros(0, np.int64))
                continue
            z = self.causes.codes[cause_ids[idx]]
            feats = self.transforms[m](z)
            feats = feats + rng.normal(0.0, p.noise, feats.shape)
            times = onsets[idx] + lags[idx, mi]
            if wrap_T is not None and m in wrap_mods:
                times = times % wrap_T
            out[m] = Emission(times=times, feats=feats, events=idx)
        return out

    def sample_scene(self, rng: Optional[np.random.Generator] = None,
                     mode: str = "standard", cause_pool: str = "train",
                     lag: Optional[int] = None,
                     token_drop_p: Optional[float] = None,
                     modality_drop_p: Optional[float] = None) -> Scene:
        rng = rng or self.rng
        p = self.params
        G, M = p.n_events, len(self.modalities)
        pool = self.causes.pool(cause_pool)
        cause_ids = pool[rng.choice(len(pool), size=G, replace=False)]
        tdp = p.token_drop_p if token_drop_p is None else token_drop_p
        mdp = p.modality_drop_p if modality_drop_p is None else modality_drop_p

        if mode == "conflict":
            # X-C2 ring construction on the designated pair: vis[e] @ e and
            # aud[e] @ (e + delta) mod G, so EVERY co-timed vis/aud token
            # belongs to a different cause while each true partner sits
            # exactly ``delta`` steps away (delta < G required).
            d = p.conflict_delta
            ma, mb = p.conflict_pair
            assert 0 < d < G, "conflict_delta must be in (0, n_events)"
            onsets = np.arange(G)
            lags = np.zeros((G, M), dtype=np.int64)
            for mi, m in enumerate(self.modalities):
                if m == mb:
                    lags[:, mi] = d                        # partner shifted
                elif m == ma:
                    lags[:, mi] = 0
                else:
                    lags[:, mi] = rng.integers(0, p.lag + 1, size=G)
        else:
            onsets = rng.integers(0, p.T, size=G)
            if p.fixed_shift is not None:
                lags = np.tile(np.arange(M) * int(p.fixed_shift), (G, 1))
            else:
                lmax = p.lag if lag is None else int(lag)
                lags = rng.integers(0, lmax + 1, size=(G, M))

        drop = rng.random((G, M)) < tdp
        if mode == "conflict":
            # keep the adversarial structure deterministic: no token drops
            # on the conflict pair
            ma, mb = p.conflict_pair
            for mi, m in enumerate(self.modalities):
                if m in (ma, mb):
                    drop[:, mi] = False
        present = rng.random(M) >= mdp
        present = np.broadcast_to(present, (G, M)).copy()
        drop |= ~present

        wrap_T = G if mode == "conflict" else None
        wrap_mods = p.conflict_pair if mode == "conflict" else ()
        emissions = self._emit(cause_ids, onsets, lags, rng, drop,
                               wrap_T=wrap_T, wrap_mods=wrap_mods)
        return Scene(emissions=emissions, causes=cause_ids,
                     onsets=onsets, mode=mode)

    def sample_scenes(self, n: int, seed: Optional[int] = None,
                      **kwargs) -> List[Scene]:
        rng = np.random.default_rng(seed) if seed is not None else self.rng
        return [self.sample_scene(rng, **kwargs) for _ in range(n)]

    # minimal gym-ish surface: reset returns one scene observation
    def reset(self) -> Dict[str, dict]:
        self._last_scene = self.sample_scene(self.rng)
        return self._last_scene.observation(self.rng)
