# Kamimusuhi S0 — Self / World Separation

Can an agent that only sees a sensor vector learn to separate
self-caused change from world-caused change, without ever being told
which sensors are which? S0 tests whether an **action-aware dynamics
model** acquires an internal representation of agency.

No RL. First stage is self-supervised next-observation prediction.

## Environment

`env/` — 16-dim observation, 4 actions (`MOVE_A`, `MOVE_B`,
`ACTUATE_X`, `NOOP`).

Latent state is split into `z_self` (actions only), `z_ext` (exogenous
AR dynamics + disturbance impulses) and `z_mix` (both). Observation
dims read from one block each, giving every dim a ground-truth cause
label — `self` / `external` / `mixed` / `noise` — used **only at
evaluation time**.

Attribution is exact: a shadow latent evolves with identical noise but
NOOP actions, so `action_component = W·(z − z_shadow)` is the true
self-caused observation signal even under nonlinearity.

## Layout

```
env/dynamics.py      latent dynamics params, EnvConfig, cause labels
env/agency_env.py    AgencyEnv (step/reset/get_state/set_state + hooks),
                     rollout, policies
models/              mlp.py, gru.py — state-only vs state+action variants
analysis/probes.py   linear probes: action, self/world component R2,
                     disturbance detection from latents
analysis/attribution.py  action-sensitivity → causal attribution AUC
analysis/plots.py    report figures
configs/default.yaml env + data + train + eval config
data.py              episode collection, config loading
train.py             supervised dynamics training
evaluate.py          causal battery S-C1..S-C5 + OOD + probes
sweep.py             multi-seed sweep -> reports/sweep_summary.json
tests/
reports/
```

## Models

`mlp_state`, `mlp_state_action`, `gru_state`, `gru_state_action`,
`gru128_state_action` — `state+action` variants receive the action
one-hot; `state` variants are the action-blind baselines.

## Run

```bash
PYTHON=/Users/eightman/dev/sandbox/kamimusuhi/.venv/bin/python

# unit tests
$PYTHON -m pytest experiments/s0/tests -q        # from repo root,
# or: cd experiments/s0 && $PYTHON -m pytest tests -q

# smoke (fast)
cd experiments/s0
S0_TIME_BUDGET=30 $PYTHON train.py --model gru_state_action --seed 0 \
    --max-steps 200
$PYTHON evaluate.py --run-dir runs/gru_state_action__seed0

# full sweep
S0_TIME_BUDGET=300 $PYTHON sweep.py --seeds 0 1 2
```

`S0_TIME_BUDGET` (seconds) caps training wall-time for smoke runs;
`--max-steps` caps gradient steps. Both are externalized so no code
change is needed between smoke and full runs.

## Evaluation contract

`evaluate.py` writes `eval.json` next to each checkpoint:

- `base_mse{self,external,mixed,noise,overall}` — per-cause-group MSE
- `sc1_shuffle` — action-sequence shuffle must hurt self-dim prediction
- `sc2_counterfactual` — predicted alt-action deltas vs true diffs
  (cosine + MSE on action-affected dims)
- `sc4_permutation` — actuator-effect permutation, before/after small
  readaptation (S0-H5)
- `sc5_mask` — action input → NOOP; should collapse to state-only level
- `ood` — unseen disturbance / action gain / coupling / delay
- `attribution` — model action-sensitivity AUC vs true cause labels
- `probes` — linear decodability of action, self/world components,
  disturbance events from latents

## Success criteria (spec §11)

| # | check | metric |
|---|-------|--------|
| 1 | state+action > state-only | base_mse self/mixed |
| 2 | action shuffle hurts | sc1_shuffle.self ≫ base.self |
| 3 | latent probeable | probes.r2_self_on_self_dims, action_probe_acc |
| 4 | unseen disturbance keeps self prediction | ood.disturbance.self |
| 5 | multi-seed reproducible | sweep_summary means/stds |

Strong PASS adds: `counterfactual_cos` high, `sc4` readaptation fast,
attribution AUC ≈ 1.
