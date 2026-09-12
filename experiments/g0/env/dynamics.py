"""Latent-cause dynamics parameters for the G0 grounding environment.

World model (never given to the agent):

    latent cause c(t) in {0..n_causes-1} (or an unordered pair),
    per-cause intensity state x(t) with cause-specific dynamics,

        x(t+1) = decay_c * x(t) + response_c[action] + special_c + noise

    u(t)   = W @ (x_a v_a [+ x_b v_b])          canonical sensor signal
    obs(t) = C_ctx @ (u(t) + distractor(t)) + read_noise

- Each cause has a fixed *feature vector* v_c over F virtual features and
  a fixed *behavioural signature*: decay rate, per-action response,
  process noise, and optional oscillation / spiking. The signature is
  context-invariant; the appearance is not.
- A *context* is a sensor-harness change: either a signed permutation
  (permute + per-dim gain/sign) or a dense mixing matrix applied to the
  canonical readout. The same cause therefore produces very different
  raw observations in different contexts.
- Some observation dims are cause-independent distractors (pure noise or
  exogenous AR(1)) to make raw sensor clustering imperfect.

Cause ids, context ids, intensities and the canonical signal are recorded
per step for EVALUATION ONLY. Training data uses obs + actions only.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import List, Optional, Tuple

import numpy as np

N_ACTIONS = 4
ACTION_NAMES = ("APPROACH", "WITHDRAW", "TAP", "NOOP")
APPROACH, WITHDRAW, TAP, NOOP = range(N_ACTIONS)


@dataclass(frozen=True)
class CauseSpec:
    """Static behavioural description of one latent cause."""

    name: str
    v: Tuple[float, ...]      # feature vector over F virtual features
    decay: float              # intensity retention per step
    resp: Tuple[float, float, float, float]  # additive drive per action
    proc: float               # intensity process noise std
    osc_freq: float = 0.0     # rad/step; >0 => x = envelope * sin(phase)
    spike_p: float = 0.0      # spontaneous spike probability per step
    spike_amp: float = 0.0    # spontaneous spike amplitude
    tap_spike_p: float = 0.0  # spike probability triggered by TAP


# The latent cause table. Deliberately designed so that instantaneous
# appearance is ambiguous across contexts while the *dynamical* signature
# (decay, action responses, oscillation, spiking) is distinct.
#
#   HEAT vs COLD: nearly negated feature vectors (superficially similar
#   under a sign-flipping context) but different decay + action profile.
#   HEAT vs DRIFT: both smooth/persistent but *inverted* action response.
#   OSC: unique oscillation; instantaneous signal crosses zero -> a
#   single frame is uninformative, only a window reveals it.
#   PULSE: mostly silent with rare spikes -> per-step appearance is close
#   to NEUTRAL unless history (or TAP) reveals the event character.
CAUSE_TABLE: Tuple[CauseSpec, ...] = (
    CauseSpec("NEUTRAL", (0.0, 0.0, 0.0, 0.0), 0.90, (0.0, 0.0, 0.0, 0.0),
              0.005),
    CauseSpec("HEAT", (1.0, 0.5, 0.2, 0.0), 0.96, (0.45, -0.25, 0.08, 0.0),
              0.02),
    CauseSpec("COLD", (-1.0, -0.5, -0.2, 0.0), 0.82, (-0.10, 0.15, -0.55, 0.0),
              0.02),
    CauseSpec("OSC", (0.1, 1.0, 0.3, 0.0), 0.995, (0.30, -0.10, 0.55, 0.0),
              0.01, osc_freq=0.45),
    CauseSpec("PULSE", (0.2, 0.1, 1.0, 0.5), 0.55, (0.05, 0.0, 0.15, 0.0),
              0.01, spike_p=0.10, spike_amp=1.3, tap_spike_p=0.9),
    CauseSpec("DRIFT", (0.0, 0.3, 0.5, 1.0), 0.97, (-0.20, 0.40, 0.0, 0.0),
              0.07),
)
N_CAUSES = len(CAUSE_TABLE)
NEUTRAL = 0

# Cause pairs used in composition segments. Indices into CAUSE_TABLE.
# Train pairs are seen during training (with pair_prob); OOD pairs are
# only ever generated for evaluation data.
TRAIN_PAIRS: Tuple[Tuple[int, int], ...] = ((1, 3), (1, 5), (2, 4), (2, 5),
                                            (3, 4))
OOD_PAIRS: Tuple[Tuple[int, int], ...] = ((1, 2), (1, 4), (2, 3), (3, 5),
                                          (4, 5))


@dataclass
class EnvConfig:
    obs_dim: int = 16
    n_features: int = 4          # virtual feature channels F
    n_noise_dims: int = 3        # dims with no signal loading (pure noise)
    n_ar_dims: int = 2           # dims driven by cause-independent AR(1)

    n_train_contexts: int = 6    # contexts the agent trains on
    n_contexts: int = 10         # total contexts generated (>= train + ood)
    context_dense_from: int = 8  # contexts >= this index are dense mixes

    episode_len: int = 64
    min_segment: int = 10        # min steps before a cause switch is allowed
    max_segment: int = 28        # forced switch at this length
    switch_prob: float = 0.05    # random switch prob after min_segment
    pair_prob: float = 0.20      # prob a new segment contains two causes

    obs_noise: float = 0.05      # per-dim read noise std
    noise_dim_std: float = 0.35  # std of pure-noise dims
    ar_coef: float = 0.90        # distractor AR(1) coefficient
    ar_std: float = 0.25         # distractor AR(1) stationary-ish std
    x_clip: float = 3.0          # intensity clamp

    ctx_gain_lo: float = 0.6     # signed-permutation context gain range
    ctx_gain_hi: float = 1.6


@dataclass
class DynamicsParams:
    """Fixed random draws of the environment, derived from a seed."""

    w: np.ndarray               # (obs_dim, n_features) canonical readout
    contexts: List[np.ndarray]  # (obs_dim, obs_dim) context matrices C_k
    context_kind: List[str]     # "perm" | "dense"
    signal_dims: np.ndarray     # dims with nonzero W rows
    noise_dims: np.ndarray
    ar_dims: np.ndarray
    v_table: np.ndarray         # (n_causes, n_features) feature vectors


def _normalized_rows(rng: np.random.Generator, rows: int,
                     cols: int) -> np.ndarray:
    w = rng.normal(size=(rows, cols))
    return w / np.linalg.norm(w, axis=1, keepdims=True)


def _haar(rng: np.random.Generator, dim: int) -> np.ndarray:
    q, r = np.linalg.qr(rng.normal(size=(dim, dim)))
    q *= np.sign(np.diag(r))  # uniform on O(dim)
    return q


def make_dynamics_params(cfg: EnvConfig, seed: int) -> DynamicsParams:
    rng = np.random.default_rng(seed)

    w = _normalized_rows(rng, cfg.obs_dim, cfg.n_features)
    n_zero = cfg.n_noise_dims + cfg.n_ar_dims
    zero_idx = rng.choice(cfg.obs_dim, size=n_zero, replace=False)
    w[zero_idx] = 0.0
    noise_dims = np.sort(zero_idx[: cfg.n_noise_dims])
    ar_dims = np.sort(zero_idx[cfg.n_noise_dims:])
    signal_dims = np.sort(np.setdiff1d(np.arange(cfg.obs_dim), zero_idx))

    contexts: List[np.ndarray] = []
    kinds: List[str] = []
    for k in range(cfg.n_contexts):
        if k < cfg.context_dense_from:
            perm = rng.permutation(cfg.obs_dim)
            gains = rng.uniform(cfg.ctx_gain_lo, cfg.ctx_gain_hi,
                                cfg.obs_dim)
            signs = rng.choice([-1.0, 1.0], cfg.obs_dim)
            c = np.diag(gains * signs)[:, perm]  # obs = C @ canonical
            kinds.append("perm")
        else:
            c = _haar(rng, cfg.obs_dim)
            kinds.append("dense")
        contexts.append(c)

    v_table = np.stack([np.asarray(c.v, dtype=np.float64)
                        for c in CAUSE_TABLE])
    return DynamicsParams(w=w, contexts=contexts, context_kind=kinds,
                          signal_dims=signal_dims, noise_dims=noise_dims,
                          ar_dims=ar_dims, v_table=v_table)


def cause_names() -> Tuple[str, ...]:
    return tuple(c.name for c in CAUSE_TABLE)
