"""Latent dynamics parameters for the S0 agency environment.

World model (conceptually, never given to the agent):

    z(t+1) = f_world(z(t)) + f_action(a(t)) + noise
    obs(t) = W @ z(t) + read_noise

Latent z is split into three groups:

    z_self : driven only by the agent's actions (plus decay and small noise)
    z_ext  : driven only by exogenous AR dynamics and disturbance impulses
    z_mix  : driven by both

Each observation dimension reads from exactly one of the blocks
{self, external, mixed, noise}, which defines its ground-truth cause
label. Labels are evaluation-only; the agent never sees them.
"""

from __future__ import annotations

from dataclasses import dataclass, field

import numpy as np

N_ACTIONS = 4
ACTION_NAMES = ("MOVE_A", "MOVE_B", "ACTUATE_X", "NOOP")
NOOP = 3


@dataclass
class EnvConfig:
    obs_dim: int = 16
    n_self_dims: int = 5
    n_ext_dims: int = 5
    n_mix_dims: int = 4
    # remaining dims are pure i.i.d. noise

    z_self_dim: int = 4
    z_ext_dim: int = 4
    z_mix_dim: int = 4

    self_decay: float = 0.92
    ext_ar_coef: float = 0.95       # spectral radius of external AR matrix
    mix_decay: float = 0.90
    ext_coupling: float = 0.4       # off-diagonal coupling of external AR

    action_gain: float = 1.0        # scales f_action
    self_act_coef: float = 0.35     # per-step action increment on z_self
    mix_act_coef: float = 0.15      # per-step action increment on z_mix

    disturbance_prob: float = 0.08  # prob. of an external impulse per step
    disturbance_gain: float = 0.8   # impulse magnitude on z_ext
    mix_disturbance_gain: float = 0.3

    process_noise: float = 0.02     # latent process noise std
    obs_noise: float = 0.05         # read noise std
    noise_dim_std: float = 0.5      # std of pure-noise obs dims

    sensor_coupling: float = 0.0    # 0 = block reads; >0 leaks across blocks
    action_delay: int = 0           # steps of actuator delay (OOD knob)
    nonlinearity: float = 0.3       # tanh squashing on latent update; 0 = linear

    episode_len: int = 64


@dataclass
class DynamicsParams:
    """Fixed random draws of the environment, derived from a seed."""

    ext_ar: np.ndarray            # (z_ext, z_ext) stable AR matrix
    action_map: np.ndarray        # (n_actions, z_self) action -> z_self delta
    mix_action_map: np.ndarray    # (n_actions, z_mix)
    w_self: np.ndarray            # (n_self_dims, z_self)
    w_ext: np.ndarray             # (n_ext_dims, z_ext)
    w_mix: np.ndarray             # (n_mix_dims, z_self + z_ext + z_mix)
    cause_labels: np.ndarray      # (obs_dim,) int codes of CauseLabels
    obs_perm: np.ndarray          # (obs_dim,) permutation scrambling dim order


class CauseLabels:
    SELF = 0
    EXTERNAL = 1
    MIXED = 2
    NOISE = 3
    NAMES = ("self", "external", "mixed", "noise")


def _stable_ar(rng: np.random.Generator, dim: int, radius: float,
               coupling: float) -> np.ndarray:
    a = np.eye(dim) * radius + rng.normal(0.0, coupling, (dim, dim))
    # rescale to keep spectral radius <= radius
    spec = max(1e-8, float(np.max(np.abs(np.linalg.eigvals(a)))))
    return a * (radius / spec)


def _normalized_rows(rng: np.random.Generator, rows: int, cols: int) -> np.ndarray:
    w = rng.normal(size=(rows, cols))
    return w / np.linalg.norm(w, axis=1, keepdims=True)


def make_dynamics_params(cfg: EnvConfig, seed: int) -> DynamicsParams:
    rng = np.random.default_rng(seed)

    ext_ar = _stable_ar(rng, cfg.z_ext_dim, cfg.ext_ar_coef, cfg.ext_coupling)

    # MOVE_A / MOVE_B push z_self along two opposite-ish directions,
    # ACTUATE_X kicks a different subspace, NOOP does nothing.
    action_map = np.zeros((N_ACTIONS, cfg.z_self_dim))
    d1 = rng.normal(size=cfg.z_self_dim); d1 /= np.linalg.norm(d1)
    d2 = rng.normal(size=cfg.z_self_dim); d2 /= np.linalg.norm(d2)
    action_map[0] = d1                       # MOVE_A
    action_map[1] = -d1 + 0.3 * d2           # MOVE_B
    action_map[2] = d2                       # ACTUATE_X
    # row NOOP stays zero

    mix_action_map = np.zeros((N_ACTIONS, cfg.z_mix_dim))
    m1 = rng.normal(size=cfg.z_mix_dim); m1 /= np.linalg.norm(m1)
    mix_action_map[0] = 0.5 * m1
    mix_action_map[2] = m1

    w_self = _normalized_rows(rng, cfg.n_self_dims, cfg.z_self_dim)
    w_ext = _normalized_rows(rng, cfg.n_ext_dims, cfg.z_ext_dim)
    z_total = cfg.z_self_dim + cfg.z_ext_dim + cfg.z_mix_dim
    w_mix = rng.normal(0.0, 1.0 / np.sqrt(z_total), (cfg.n_mix_dims, z_total))
    # balance self vs external contribution in mixed dims
    w_mix[:, : cfg.z_self_dim] *= 1.0
    w_mix[:, cfg.z_self_dim: cfg.z_self_dim + cfg.z_ext_dim] *= 1.0

    n_noise = cfg.obs_dim - cfg.n_self_dims - cfg.n_ext_dims - cfg.n_mix_dims
    labels = np.array(
        [CauseLabels.SELF] * cfg.n_self_dims
        + [CauseLabels.EXTERNAL] * cfg.n_ext_dims
        + [CauseLabels.MIXED] * cfg.n_mix_dims
        + [CauseLabels.NOISE] * n_noise,
        dtype=np.int64,
    )
    obs_perm = rng.permutation(cfg.obs_dim)  # dims not positionally sorted
    labels = labels[obs_perm]                # labels aligned to obs order

    return DynamicsParams(
        ext_ar=ext_ar,
        action_map=action_map,
        mix_action_map=mix_action_map,
        w_self=w_self,
        w_ext=w_ext,
        w_mix=w_mix,
        cause_labels=labels,
        obs_perm=obs_perm,
    )
