"""O0 object-permanence world dynamics (pure numpy, fully vectorized).

A 1D bounded world ``[0, world_len]``.  A target object appears at a
random position, travels with near-constant velocity (mild drag, a small
per-episode constant acceleration, and per-step process noise), passes
behind a static occluder interval where it is invisible for a while, and
reappears on the far side -- unless its trajectory carries it out of the
world while it is hidden.  "Gone" episodes place the occluder flush with
the world edge in the direction of travel, so the object is absorbed by
the boundary while unseen and never reappears.  Distractor objects
wander the world and are occluded by the same interval.

The observation is a compact feature vector (``OBS_DIM`` floats,
float64).  While the target is inside the occluder *all* of its channels
read exactly 0: no hidden position, no velocity, no appearance, no
time-since-occlusion, no time-until-reappearance.  The occluder bounds
are always observable (it is a visible wall), which constrains a hidden
position to an interval -- that is legitimate world geometry, and the
leakage test verifies nothing beyond it is decodable.

``generate_batch`` returns the observation sequence plus trainer-only
labels (exist / pos / vel / same / masks).  Labels are *never* part of
the observation.  Reward is meaningless here; the task is prediction.
"""

from __future__ import annotations

from dataclasses import dataclass, replace
from typing import Optional, Tuple

import numpy as np

# ---------------------------------------------------------------------------
# Observation layout
# ---------------------------------------------------------------------------

T_VIS, T_X, T_V, T_APP = 0, 1, 2, 3
D_SLOTS = 4                      # fixed distractor slots keep OBS_DIM constant
D0 = 4                           # distractor slot i occupies D0 + 3*i .. +2
OCC_LO_IDX = D0 + 3 * D_SLOTS    # 16
OCC_HI_IDX = OCC_LO_IDX + 1      # 17
AMBIENT_IDX = OCC_HI_IDX + 1     # 18
NOISE_IDX = AMBIENT_IDX + 1      # 19
OBS_DIM = NOISE_IDX + 1          # 20

OBS_NAMES = (
    ["target_visible", "target_x", "target_v", "target_app"]
    + sum(
        ([f"d{i}_visible", f"d{i}_x", f"d{i}_app"] for i in range(D_SLOTS)),
        [],
    )
    + ["occluder_lo", "occluder_hi", "ambient", "noise"]
)
assert len(OBS_NAMES) == OBS_DIM

VEL_SCALE = 10.0  # obs velocity channel = v_world * VEL_SCALE / world_len


# ---------------------------------------------------------------------------
# Parameters
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class EnvParams:
    """All tunable environment parameters (OOD presets override a subset)."""

    world_len: float = 4.0
    horizon: int = 96

    # episode phase structure (in steps)
    t_appear_lo: int = 2
    t_appear_hi: int = 6
    vis_lo: int = 3                  # visible steps before occluder entry
    vis_hi: int = 8
    occ_lo: int = 4                  # planned occlusion steps
    occ_hi: int = 16
    post_lo: int = 6                 # steps after reappearance
    post_hi: int = 10

    # target motion
    v_lo: float = 0.04               # |v| range, world units / step
    v_hi: float = 0.09
    drag: float = 0.005              # v <- v*(1-drag) each step
    accel_std: float = 0.0005        # per-episode constant acceleration
    v_noise: float = 0.002           # process noise on v, per step
    v_max: float = 0.25

    # appearance feature
    app_lo: float = 0.2
    app_hi: float = 0.9
    app_swap_min: float = 0.25       # |app_new - app_old| after an identity swap
    reappear_app_jitter: float = 0.0  # OOD: noise on appearance at reappearance

    # episode kinds
    p_gone: float = 0.2              # absorbed by boundary while occluded
    p_swap: float = 0.3              # appearance swap at reappearance (O0-C)
    v_flip_prob: float = 0.0         # OOD: velocity reverses mid-occlusion

    # occluder geometry
    occ_margin: float = 0.06         # persistent occluders stay inside
                                     # [m*L, (1-m)*L]; gone occluders touch edge
    gone_span_lo: float = 1.0        # occluder->boundary distance for gone eps
    gone_span_hi: float = 2.0
    spawn_frac_lo: float = 0.02      # appearance position range (fraction of L)
    spawn_frac_hi: float = 0.25

    # distractors
    distractor_probs: Tuple[float, ...] = (0.2, 0.3, 0.3, 0.1, 0.1)
    v_distractor_lo: float = 0.03
    v_distractor_hi: float = 0.10
    ambush_prob: float = 0.0         # OOD: decoy timed to exit near reappearance

    # observation noise / extra channels
    obs_noise: float = 0.005

    def with_overrides(self, **kw) -> "EnvParams":
        return replace(self, **kw)


# ---------------------------------------------------------------------------
# Episode generation (vectorized over `batch` episodes)
# ---------------------------------------------------------------------------


def _resample_app(rng: np.random.Generator, old: np.ndarray, p: EnvParams):
    """New appearance a moderate distance from `old`: |delta| in
    [app_swap_min, app_swap_min + 0.2] (reflected into bounds).  A
    moderate change keeps O0-C non-trivial: detecting it requires
    comparing against the *remembered* appearance."""
    delta = rng.uniform(p.app_swap_min, p.app_swap_min + 0.2, size=old.shape)
    sign = rng.choice(np.array([-1.0, 1.0]), size=old.shape)
    new = old + sign * delta
    # reflect out-of-range draws back inside [app_lo, app_hi]
    over = new > p.app_hi
    new = np.where(over, old - np.abs(new - old), new)
    under = new < p.app_lo
    new = np.where(under, old + np.abs(new - old), new)
    return np.clip(new, p.app_lo, p.app_hi)


def generate_batch(
    p: EnvParams,
    batch: int,
    rng: np.random.Generator,
) -> dict:
    """Generate `batch` episodes of length `p.horizon`.

    Returns a dict of arrays.  Observation: obs[T, B, OBS_DIM].
    Labels (trainer only): exist, pos (normalized), vel (normalized),
    same, occluded, visible, id_mask -- all [T, B].  Per-episode meta:
    t_appear, t_occl, t_reapp, n_occl, mid_occl, mid_vis, gone, swapped,
    direction, v0, occ_lo, occ_hi, ambush -- all [B].
    """
    T, L, B = p.horizon, p.world_len, batch
    margin = p.occ_margin * L

    # --- per-episode draws -------------------------------------------------
    dirn = rng.choice(np.array([-1.0, 1.0]), size=B)
    speed = rng.uniform(p.v_lo, p.v_hi, B)
    v0 = dirn * speed
    accel = rng.normal(0.0, p.accel_std, B)
    t_app = rng.integers(p.t_appear_lo, p.t_appear_hi + 1, B)
    vis_steps = rng.integers(p.vis_lo, p.vis_hi + 1, B)
    n_plan = rng.integers(p.occ_lo, p.occ_hi + 1, B)
    gone = rng.random(B) < p.p_gone
    swap_flag = (rng.random(B) < p.p_swap) & ~gone
    flip_flag = (rng.random(B) < p.v_flip_prob) & ~gone
    app0 = rng.uniform(p.app_lo, p.app_hi, B)

    # --- occluder geometry -------------------------------------------------
    width = np.abs(v0) * n_plan * rng.uniform(0.95, 1.15, B)
    width = np.maximum(width, 0.05)
    x_s = np.where(
        dirn > 0,
        rng.uniform(p.spawn_frac_lo * L, p.spawn_frac_hi * L, B),
        rng.uniform((1.0 - p.spawn_frac_hi) * L, (1.0 - p.spawn_frac_lo) * L, B),
    )
    entry = x_s + v0 * vis_steps  # approx position at occluder entry
    # persistent occluder: [entry, entry + dir*width], clipped inside margins
    lo_p = np.where(dirn > 0, entry, entry - width)
    hi_p = np.where(dirn > 0, entry + width, entry)
    lo_p = np.clip(lo_p, margin, L - margin)
    hi_p = np.clip(hi_p, margin, L - margin)
    bad = hi_p - lo_p < 0.05  # pathological clip: fall back to a small window
    lo_p = np.where(bad, np.clip(entry - 0.05, margin, L - margin), lo_p)
    hi_p = np.where(bad, np.clip(entry + 0.05, margin, L - margin), hi_p)
    # gone occluder: spans from the entry edge to the boundary ahead
    span = rng.uniform(p.gone_span_lo, p.gone_span_hi, B)
    lo_g = np.where(dirn > 0, np.minimum(entry, L - span), 0.0)
    hi_g = np.where(dirn > 0, L, np.maximum(entry, span))
    occ_lo = np.where(gone, lo_g, lo_p)
    occ_hi = np.where(gone, hi_g, hi_p)

    # planned flip step (approximate mid-occlusion)
    t_occl_plan = t_app + vis_steps
    flip_step = t_occl_plan + n_plan // 2

    # --- distractors --------------------------------------------------------
    probs = np.asarray(p.distractor_probs, dtype=np.float64)
    probs = probs / probs.sum()
    n_dist = rng.choice(len(probs), size=B, p=probs)
    d_x = rng.uniform(0.0, L, (B, D_SLOTS))
    d_v = rng.choice(np.array([-1.0, 1.0]), size=(B, D_SLOTS)) * rng.uniform(
        p.v_distractor_lo, p.v_distractor_hi, (B, D_SLOTS)
    )
    d_app = rng.uniform(p.app_lo, p.app_hi, (B, D_SLOTS))
    ambush = (rng.random(B) < p.ambush_prob) & (n_dist > 0) & ~gone
    # ambush: distractor slot 0 times its exit of the occluder's far edge to
    # a few steps before the target's planned reappearance.
    t_reapp_plan = t_occl_plan + n_plan
    delta = rng.integers(1, 5, B).astype(np.float64)
    far_edge = np.where(dirn > 0, occ_hi, occ_lo)
    dv0 = np.abs(v0) * rng.uniform(0.85, 1.15, B) * dirn
    xa = far_edge - dv0 * (t_reapp_plan - delta)
    ok = (xa > 0.0) & (xa < L) & ambush
    d_x[:, 0] = np.where(ok, xa, d_x[:, 0])
    d_v[:, 0] = np.where(ok, dv0, d_v[:, 0])

    # --- rollout ------------------------------------------------------------
    obs = np.zeros((T, B, OBS_DIM), dtype=np.float64)
    exist_l = np.zeros((T, B))
    pos_l = np.zeros((T, B))
    vel_l = np.zeros((T, B))
    same_l = np.ones((T, B))
    occ_m = np.zeros((T, B), dtype=bool)
    vis_m = np.zeros((T, B), dtype=bool)
    id_mask = np.zeros((T, B), dtype=bool)

    x = np.zeros(B)
    v = np.zeros(B)
    app = app0.copy()
    spawned = np.zeros(B, dtype=bool)
    alive = np.zeros(B, dtype=bool)
    ever_inside = np.zeros(B, dtype=bool)
    inside_prev = np.zeros(B, dtype=bool)
    reappeared = np.zeros(B, dtype=bool)
    imposter = np.zeros(B, dtype=bool)
    flipped = np.zeros(B, dtype=bool)
    t_occl = np.full(B, -1, dtype=np.int64)
    t_reapp = np.full(B, -1, dtype=np.int64)
    n_occl = np.zeros(B, dtype=np.int64)
    ambient = rng.uniform(0.3, 0.7, B)

    for t in range(T):
        # spawn
        just = (~spawned) & (t >= t_app)
        x[just] = x_s[just]
        v[just] = v0[just]
        spawned |= just
        alive |= just
        # motion: accel -> drag -> noise -> move
        v[alive] += accel[alive]
        v[alive] *= 1.0 - p.drag
        v[alive] += rng.normal(0.0, p.v_noise, int(alive.sum()))
        np.clip(v, -p.v_max, p.v_max, out=v)
        x[alive] += v[alive]
        # boundary absorption
        out = alive & ((x < 0.0) | (x > L))
        alive[out] = False
        # occlusion state
        inside = alive & (x >= occ_lo) & (x <= occ_hi)
        # planned mid-occlusion velocity reversal (OOD motion shift);
        # applies from the next step's motion update.
        doflip = flip_flag & ~flipped & inside & (t >= flip_step)
        v[doflip] = -v[doflip]
        flipped |= doflip
        first_in = inside & ~ever_inside
        t_occl[first_in] = t
        ever_inside |= inside
        n_occl += inside.astype(np.int64)
        reapp = inside_prev & ~inside & alive & ever_inside & ~reappeared
        # identity swap / appearance jitter at the reappearance step
        do_swap = reapp & swap_flag
        if do_swap.any():
            app[do_swap] = _resample_app(rng, app[do_swap], p)
        imposter |= do_swap
        if p.reappear_app_jitter > 0:
            jit = reapp & ~swap_flag
            app[jit] += rng.normal(0.0, p.reappear_app_jitter, int(jit.sum()))
            np.clip(app, 0.0, 1.0, out=app)
        t_reapp[reapp] = t
        reappeared |= reapp
        inside_prev = inside

        # distractor motion (constant velocity, bounce off boundaries)
        d_x += d_v
        hit_lo = d_x < 0.0
        hit_hi = d_x > L
        d_x[hit_lo] = -d_x[hit_lo]
        d_v[hit_lo] = -d_v[hit_lo]
        d_x[hit_hi] = 2.0 * L - d_x[hit_hi]
        d_v[hit_hi] = -d_v[hit_hi]
        d_inside = (d_x >= occ_lo[:, None]) & (d_x <= occ_hi[:, None])

        # ambient channel: slow OU process, unrelated to the task
        ambient += 0.05 * (0.5 - ambient) + rng.normal(0.0, 0.03, B)
        np.clip(ambient, 0.0, 1.0, out=ambient)

        # --- labels (trainer only) -----------------------------------------
        exist_l[t] = alive.astype(np.float64)
        pos_l[t] = x / L
        vel_l[t] = v * VEL_SCALE / L
        occ_m[t] = inside
        vis = alive & ~inside
        vis_m[t] = vis
        id_mask[t] = vis & ever_inside
        same_l[t] = (~imposter).astype(np.float64)

        # --- observation ----------------------------------------------------
        o = np.zeros((B, OBS_DIM), dtype=np.float64)
        nz = p.obs_noise
        o[vis, T_VIS] = 1.0
        o[vis, T_X] = x[vis] / L + rng.normal(0.0, nz, int(vis.sum()))
        o[vis, T_V] = v[vis] * VEL_SCALE / L + rng.normal(0.0, nz, int(vis.sum()))
        o[vis, T_APP] = app[vis] + rng.normal(0.0, nz, int(vis.sum()))
        for s in range(D_SLOTS):
            dvis = (s < n_dist) & ~d_inside[:, s]
            base = D0 + 3 * s
            o[dvis, base] = 1.0
            o[dvis, base + 1] = d_x[dvis, s] / L + rng.normal(
                0.0, nz, int(dvis.sum())
            )
            o[dvis, base + 2] = d_app[dvis, s] + rng.normal(
                0.0, nz, int(dvis.sum())
            )
        o[:, OCC_LO_IDX] = occ_lo / L
        o[:, OCC_HI_IDX] = occ_hi / L
        o[:, AMBIENT_IDX] = ambient
        o[:, NOISE_IDX] = rng.normal(0.0, 0.05, B)
        obs[t] = o

    # outcome meta: absorption while hidden, first-occlusion-bout length
    ever_spawned = np.arange(T)[:, None] >= t_app[None, :]
    dead = ever_spawned & (exist_l == 0.0)
    has_dead = dead.any(axis=0)
    t_absorb = np.where(has_dead, np.argmax(dead, axis=0), -1)
    absorbed = has_dead & (t_occl >= 0) & ~reappeared
    bout_len = np.where(
        t_occl >= 0,
        np.where(t_reapp > 0, t_reapp, np.where(t_absorb > 0, t_absorb, T))
        - t_occl,
        0,
    )
    mid_occl = np.where(
        t_occl >= 0, t_occl + np.maximum(1, bout_len) // 2, -1
    )
    mid_vis = np.where(
        t_occl > 0, t_app + np.maximum(1, (t_occl - t_app) // 2), t_app // 2
    )

    return {
        "obs": obs,
        "exist": exist_l,
        "pos": pos_l,
        "vel": vel_l,
        "same": same_l,
        "occluded": occ_m,
        "visible": vis_m,
        "id_mask": id_mask,
        # per-episode meta
        "t_appear": t_app,
        "t_occl": t_occl,
        "t_reapp": t_reapp,
        "n_occl": n_occl,
        "mid_occl": mid_occl,
        "mid_vis": mid_vis,
        "gone": gone,                # geometry: occluder touches far boundary
        "absorbed": absorbed,        # outcome: died while hidden, never reappeared
        "t_absorb": t_absorb,
        "bout_len": bout_len,        # steps from first occlusion to exit/absorb/end
        "swapped": swap_flag & reappeared,
        "direction": dirn,
        "v0": v0,
        "occ_bounds": np.stack([occ_lo / L, occ_hi / L], axis=1),
        "n_distractors": n_dist,
        "ambush": ambush,
    }
