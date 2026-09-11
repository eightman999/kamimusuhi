# S0 Results — Self / World Separation

Date: 2026-09-11. Branch: `exp/s0-self-world` (master merged at
ffb3808). Config: `configs/default.yaml` (16-dim obs, 4 actions,
episode_len 64). Training: 256 episodes, 2000 Adam steps, MSE on
next-obs (delta target). Evaluation: 48 fresh episodes per battery.

**RNG separation**: dynamics parameters come from `env_seed`; the
process/read-noise stream comes from an independent `noise_seed`.
Train/val/eval share the world (`env_seed`) but use disjoint noise
streams — no evaluation noise replay. 3 seeds per model.

Raw data: `reports/sweep_summary.json`, per-run `runs/*/eval.json`.

## Verdict: PASS on all 5 criteria; strong-PASS mostly holds, with two honest caveats

| # | Criterion | Result |
|---|-----------|--------|
| 1 | state+action > state-only | self MSE 0.004 vs 0.026 (6×) |
| 2 | action shuffle hurts | self MSE ×9–12 action, ×1.0 blind |
| 3 | latent probeable | action acc 1.00; self-component R² 0.89–0.92 |
| 4 | unseen disturbance keeps self prediction | self MSE ≤0.016 under 4×/3× disturbance |
| 5 | multi-seed | replicates on seeds 0–2 |

Strong-PASS: counterfactual cos ≈ 0.99, attribution AUC 0.90–0.97.
Caveats: S-C4/cross-world readaptation is matched by a scratch control
(cheap relearning, not demonstrated transfer), and `action_delay=2`
actively hurts action models (see below).

## Base prediction MSE by cause group (mean ± std, n=3)

| model | self | external | mixed | noise | overall |
|-------|------|----------|-------|-------|---------|
| mlp_state | .0267 | .0591 | .0311 | .2612 | .0672 |
| mlp_state_action | **.0043** | .0594 | .0223 | .2613 | .0581 |
| gru_state | .0260 | .0581 | .0303 | .2554 | .0658 |
| gru_state_action | **.0041** | .0585 | .0219 | .2597 | .0575 |
| gru128_state_action | **.0052** | .0684 | .0245 | .4152 | .0810 |

## Causal tests

| model | S-C1 shuffle ×base (self) | S-C5 mask ×base (self) | S-C2 cf cos |
|-------|--------------------------|------------------------|-------------|
| mlp_state | 1.00 | 1.00 | 0.000 |
| mlp_state_action | 10.82 | 6.56 | 0.991 |
| gru_state | 1.00 | 1.00 | 0.000 |
| gru_state_action | 12.02 | 7.85 | 0.996 |
| gru128_state_action | 9.33 | 5.90 | 0.993 |

## S-C4 permutation + cross-world, with scratch controls (S0-H5)

Self-dim MSE. `before` = intervened, `after` = 16 episodes/200 steps
readapt, `scratch` = fresh model on the same 16-episode budget.

| setting | model | before | after | scratch |
|---------|-------|--------|-------|---------|
| S-C4 perm | mlp_state_action | .0674 | .0048 | .0055 |
| S-C4 perm | gru_state_action | .0721 | .0057 | .0058 |
| S-C4 perm | gru128_sa | .0696 | .0075 | .0054 |
| cross-world | mlp_state_action | .1536 | .0077 | .0057 |
| cross-world | gru_state_action | .1760 | .0080 | .0061 |
| cross-world | gru128_sa | .1912 | .0105 | .0058 |

Reading: fast readaptation is real (behavioral strong-PASS), but a
scratch model reaches the same error on the same budget — the ~1k
transitions suffice to refit this near-linear world. So S0-H5 shows
*fast adaptation*, not *transfer of agency structure*. In a new world
the model's attribution map does not transfer either (`xw_attr_auc`
≈ 0.46 ≈ chance) — attribution is learned per-world coupling, as
expected.

## OOD (§10), self-dim MSE

| condition | action models | blind |
|-----------|---------------|-------|
| disturbance ×4 prob, ×3 gain | .009–.016 | .038–.044 |
| action_gain ×1.5 | ~.013 | .027 |
| sensor_coupling .4 | ~.006 | .027 |
| **action_delay 2** | **.045–.046** | **.025–.026** |

Correction vs the earlier report: delay is not merely "the hardest
OOD" — it is the one condition where action-aware models are *worse
than blind ones* on self dims. The model predicts the action's effect
in the next observation; with delay the effect lands later, so the
action input actively injects prediction error. A blind model is
immune. This is a genuine limitation of instantaneous action input;
an action-history input or learned delay would fix it.

## Attribution & probes

- `attr_auc_self_or_mix`: 0.90–0.97 (action models), **exactly 0.500**
  for blind models (tie-aware AUC; earlier 0.13–0.60 values were a
  tie-handling artifact).
- Action probe from latent: 1.00 vs chance .25.
- Self-component R² on self dims: .89–.92 action; .67–.69 blind.
- Disturbance-event probe (alignment-fixed: latent_t ↔ impulse at
  t−1): AUC .66–.93 — latents DO encode exogenous impulses. The
  earlier "agency > disturbance asymmetry" claim was an alignment bug.

## Failure modes checked (§12)

- action-ID memorization: S-C4 degrades on permutation — the model
  encodes the effect mapping, not action→obs pairs.
- fixed-position self dims: obs permutation is per-env-seed and the
  cross-world test shows in-world attribution doesn't carry over.
- time-only prediction: S-C1/S-C5 isolate the action channel.

## Answer to the S0 question

Yes: without self labels, action-conditioned dynamics models separate
self-caused from world-caused change — counterfactual effects (cos
≈ .99), latent decodability (action 1.0, self-component R² ≈ .9),
per-dim attribution (AUC ≈ .9+), robust to quadrupled disturbance.
Caveats for certification: fast post-intervention recovery is cheap
relearning rather than proven transfer, and delayed actuation breaks
the instantaneous-action assumption.

Figures: `reports/figs/{group_mse,sensitivity,probe_r2}.png`.

## Reproduce

```bash
cd experiments/s0
PY=/Users/eightman/dev/sandbox/kamimusuhi/.venv/bin/python
$PY -m pytest tests -q
$PY sweep.py --seeds 0 1 2 --max-steps 2000
```
