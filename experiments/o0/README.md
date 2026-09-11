# O0 — Object Permanence

**Question.** After an object disappears from observation behind an
occluder, can an agent maintain its existence, position, and identity as
an internal representation? This is about *persistent world
representation under occlusion*, not general memory.

## World

A 1D bounded world `[0, L]` (`L=4`). One episode:

1. **pre-appearance**: no target (distractors may wander). `exist=0`.
2. **visible**: target appears, moves with near-constant velocity
   (drag `0.005`, per-episode acceleration `N(0, 5e-4)`, process noise).
3. **occluded**: target enters a static occluder interval (bounds are
   always observable — a visible wall); all target channels read
   exactly 0 for `N ~ U(4,16)` steps (train range; OOD up to 48).
4. **reappearance**: it exits the far side (same object, or an
   identity-swapped imposter, `p_swap=0.3`).  **Training uses
   `p_gone=0`**: boundary absorption while hidden was moved to the
   eval-only preset `gone20` after review showed that edge-touching
   occluders made realized hidden bouts reach 60+ steps, contaminating
   both the headline metric and the "trained on 4-16" claim.
5. **decoy takeover** (eval-only, `decoy_takeover_prob`): an ambush
   distractor timed to exit the occluder's far edge while the target is
   still inside is reported *on the target channel* (the sensor locks
   onto the wrong object).  `same=0` on those steps; position labels
   still track the true (hidden) target.

Distractor objects (0–4 per episode) wander, bounce off boundaries, and
are occluded by the same interval.

### Observation (float64, dim 20)

| channels | content |
|---|---|
| 0–3 | target: visible flag, x/L, v (scaled), appearance — **exactly 0 while occluded** |
| 4–15 | 4 distractor slots: visible, x/L, appearance |
| 16–17 | occluder bounds / L (always visible) |
| 18–19 | ambient OU channel, i.i.d. noise |

Forbidden while occluded: hidden position/velocity, object id,
visibility ground truth, time-since-occlusion, time-until-reappearance.
`tests/test_leakage.py` audits this (channels are exactly 0; no channel
decodes hidden position beyond the occluder bounds; counterfactual
hidden trajectories give identical observations).

## Tasks

- **O0-A position**: predict current x every step (evaluated on the
  hidden window; headline = error on the last hidden step before
  reappearance).
- **O0-B existence**: is the target still in the world? Non-trivial
  because gone episodes end existence while invisible.
- **O0-C identity**: is the visible target the one that disappeared?
  Swap episodes change the appearance signature at reappearance.

## Models & baselines

Learned (supervised, multi-task): `mlp`, `gru64`, `gru128`, `lstm64`,
`ssm` (tiny leaky-integrator bank). Same heads; recurrent state is a
single tensor `[B,H]` (LSTM packs h||c) so hidden-state interventions
are uniform.

Heuristics (`agents/heuristics.py`): `prior` (occluder-mid guess),
`lastobs`, `constvel`, `corridor` (constvel clipped to the occluder
bounds — the honest strong baseline), `openloop` (true dynamics,
open-loop integration — a reference point, *not* a Bayes-optimal
ceiling), `kalman` (linear Kalman filter over [x, v, a] with true noise
statistics — the closest available optimal-filter approximation).

## Causal tests & OOD

Interventions (`evaluate.py`): hidden-state reset / noise at
per-episode anchors (`mid_occl`, `occl_start`, `pre_reappear`,
`mid_vis` control). **Key test**: reset mid-occlusion must degrade
tracking much more than reset mid-visible (position can be re-read from
the observation when visible, but not when hidden).

OOD presets: `plen24/32/48` (pure occlusion-length shift — only the
bout-length bounds change), `occ24/32/48` (calibrated variants that
also narrow velocity/visible-window/spawn so the geometry is feasible),
`gone20` (boundary absorption while hidden — never seen in training),
`fast_v`, `slow_v`, `drag_x3`, `v_flip`, `distractors4`, `ambush`,
`decoytk` (decoy on the target channel), `app_jitter`, `swap_half`.

Metrics decompose position error by episode type (`pos_mae_occ_*`:
`persistent`, `gone`, `le16`, `gt16`, `persist_le16`) — the headline is
`pos_mae_occ_persist_le16`, error on ordinary occlusions of trained
length.

## Usage

```bash
PY=/Users/eightman/dev/sandbox/kamimusuhi/.venv/bin/python
$PY -m pytest experiments/o0/tests                       # tests
$PY -m experiments.o0.train --config experiments/o0/configs/smoke.yaml \
    --run-id smoke --artifacts experiments/o0/artifacts  # <60s smoke
$PY -m experiments.o0.evaluate --config experiments/o0/configs/default.yaml \
    --checkpoint experiments/o0/artifacts/runs/gru64_s0/best.pt \
    --preset occ32 --episodes 512
$PY -m experiments.o0.sweep --config experiments/o0/configs/default.yaml \
    --seeds 0 1 2 3 4 --models mlp gru64 gru128 lstm64     # full sweep
```

Budgets override via `O0_UPDATES` / `O0_BATCH_SIZE` / `O0_EPISODES` env
vars or `--updates/--batch-size/--episodes` CLI flags.

Results: `reports/O0_RESULTS.md`, raw JSON in `reports/raw/`, figures in
`reports/*.png`, checkpoints in `artifacts/runs/*/`.
