# H0 — Artificial Homeostasis

Can an agent with no external task learn state-dependent, self-sustaining
behavior purely from the need to keep internal variables (energy, fatigue,
novelty, certainty, temperature) inside preferred ranges?

Reward is `-homeostatic_error` plus a survival-violation term only. No
action-specific rewards, no task labels.

## Layout

```
env/dynamics.py          all environment parameters + pure update functions
env/homeostasis_env.py   Gym-like env wrapper (reset/step, causal-test helpers)
agents/                  random, heuristic, MLP + GRU actor-critics
analysis/metrics.py      primary/secondary metrics
analysis/causality.py    C1-C4 + hidden-reset ablation battery
analysis/rhythm.py       transition matrix, dwell time, autocorrelation, PSD
analysis/plots.py        figure generation
train.py                 PPO (pre/best/final checkpoints, meta.json)
evaluate.py              episode runner + CLI, ablation hooks
sweep.py                 train/evaluate/causal orchestrator
configs/                 base.yaml + ood_{energy,temperature,risk,resource}.yaml
tests/                   dynamics, reward, determinism, ablation tests
runs/                    checkpoints + train logs (gitignored)
results/                 evaluation JSONs (gitignored)
reports/H0_RESULTS.md    final report
```

## Setup

```bash
uv venv .venv --python 3.12
uv pip install --python .venv/bin/python torch --index-url https://download.pytorch.org/whl/cpu
uv pip install --python .venv/bin/python numpy pyyaml pytest matplotlib
```

## Reproduce

```bash
# unit tests
.venv/bin/python -m pytest experiments/h0/tests/

# single training run (checkpoints + meta.json into runs/...)
.venv/bin/python -m experiments.h0.train \
    --config experiments/h0/configs/base.yaml \
    --agent gru --hidden 64 --seed 0 \
    --out experiments/h0/runs/gru64/seed0

# evaluate a checkpoint (ID or OOD config)
.venv/bin/python -m experiments.h0.evaluate \
    --config experiments/h0/configs/ood_energy.yaml \
    --agent gru --checkpoint experiments/h0/runs/gru64/seed0/best.pt \
    --episodes 20 --out results/gru64_seed0_ood_energy.json

# full matrix: train all agents x seeds, then ID+OOD eval, then causal battery
.venv/bin/python -m experiments.h0.sweep all --seeds 0,1,2,3,4
```

## Design notes

- `reward = -homeostatic_error`, terminal `-survival_violation` =
  `death_penalty + death_forfeit_rate * (episode_length - t)`. The forfeit
  term is mathematically an alive bonus expressed as a survival violation;
  without it, suicide is optimal whenever sustained error exceeds
  `penalty * (1 - gamma)`.
- `resource_availability` and `hazard_level` are partially endogenous:
  REST depletes resource, EXPLORE replenishes it, OBSERVE reduces hazard.
  This makes foraging/awareness instrumentally necessary rather than
  novelty-driven, and prevents a pure REST/WAIT cycle from solving the env.
- Ambient temperature is a sinusoid with amplitude 0.20, so it leaves the
  comfort zone [0.35, 0.65] every cycle — thermoregulation requires active
  behavior (WAIT shelters toward 0.5, REST couples to ambient, EXPLORE
  samples a microclimate).
- Best checkpoint is selected by `homeostatic_error_full` (post-death steps
  count at the death-state error), so a policy cannot look good by dying
  early in a good state.
