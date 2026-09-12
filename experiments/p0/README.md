# P0 — Predictive Surprise: selective attention over costly channels

**Question.** Can an agent learn to selectively attend to channels it cannot
predict, instead of observing everything at high cost?

## Environment (`env/`)

Four scalar channels in [0, 1] (`env/dynamics.py`):

| ch | dynamics | role |
|----|----------|------|
| A | nearly constant (tiny OU wiggle) | trivially predictable |
| B | sinusoid + small noise | predictable in principle |
| C | OU + occasional large jumps | chronically unpredictable, but a **leading indicator**: each jump boosts the D-event hazard for `cd_coupling_window` steps (not a pure distractor) |
| D | latent FSM QUIET→RAMP→EVENT→QUIET | rare, **reward-relevant** events |

- D events: base hazard `p_event` per step (elevated `cd_coupling_mult`× for
  `cd_coupling_window` steps after a C jump). RAMP is a short precursor regime
  with modestly elevated mean/variance (~0.32 vs ~0.15 baseline — overlapping
  marginals, but detectable BOTH as prediction error and as a raw value
  threshold on `v_D`); EVENT is a **single-step** window (v2 — see below).
- **Detection**: observing D (OBSERVE_D or OBSERVE_ALL) during the 1-step
  EVENT yields `detect_bonus` once; an event expiring unobserved costs
  `miss_penalty`. Events still in flight at episode end are censored.
- **v2 redesign note.** With a 3-step window a pure staleness clock
  (poll D every 2–3 steps) reached det=1.0 at cost 0.005–0.007 — surprise was
  worthless. With a 1-step window a poll-k clock lands on the event with
  probability ~1/k: the clock frontier is (det≈1/k, cost 0.02/k), flat at
  efficiency ≈50.
- **v3 correction (belief-reactive frontier).** Beating the clock does NOT
  require prediction: raw-value/belief reactivity suffices. `vtrigger`
  (burst D when the last read `v_D ≥ τ`) reaches eff ≈140–170 — far above
  the surprise-driven `trigger` (eff ≈79–94) — and `cwatch` (poll C, burst D
  on detected C jumps) reaches det ≈0.8. No env feature currently makes
  calibrated prediction load-bearing in principle.

Actions: `IGNORE` (free) · `OBSERVE_A..D` (`cost_observe`) · `OBSERVE_ALL`
(`cost_observe_all`). Unobserved channels are never revealed — the env returns
stale beliefs: per channel `(v, s, pv, g)` = last read, staleness, previous
read, pre-read gap (`env/attention_env.py`).

## Surprise mechanism

`models/predictor.py`: a small MLP shared across channels maps
`(v, s, pv, g) → (x̂, ŝ)` (predicted value, predicted error) and is trained
online by masked MSE on **observed channels only** — it never sees unobserved
truth (leakage-free by construction; `tests/test_leakage.py` audits this).

Agent input = 24-dim blocked vector, per channel `(v, s, pv, g, e, d)`:
`e` = fast EMA of measured `|x̂−v|` at observation times; `d` = `ŝ`
anticipated error (computable without ground truth). Harness:
`agents/harness.py`.

## Agents (`agents/`, `models/policies.py`)

`never` · `random` · `roundrobin` · `always` (OBSERVE_ALL) · `fixed`
(staleness-clock poll of D + surprise trigger) · `clockK` (pure step-grid
D clock, K=1..5 — the non-reactive frontier) · `pe_heuristic`
(hand-coded argmax-surprise attention, never ignores) · `pe_thresh`
(argmax-surprise iff > θ else IGNORE) · `trigger` (sparse D polling +
surprise-gated burst) · `vtrigger` / `vtrigger22` (same burst machinery but
triggered on the raw belief value `v_D` — no predictor) · `cwatch` (poll C,
burst D on detected C jumps — exploits the C→D coupling, no predictor) ·
`mlp` / `gru` (PPO-learned) · `mlp`+`pe_mask` (belief-only control,
configs/base_bonly.yaml).

## Reproduce

```bash
PY=/Users/eightman/dev/sandbox/kamimusuhi/.venv/bin/python
cd <worktree root>
# smoke (<60 s)
P0_SMOKE=1 $PY -m experiments.p0.train --config experiments/p0/configs/base.yaml
# full training (one seed)
$PY -m experiments.p0.train --config experiments/p0/configs/base.yaml --seed 0
# eval any agent
$PY -m experiments.p0.evaluate --config experiments/p0/configs/base.yaml \
    --agent mlp --checkpoint experiments/p0/runs/base_mlp_s0/ckpt_best.pt \
    --episodes 12 --ablation pe_mask
# sweep: baselines + causal + OOD across seeds
$PY -m experiments.p0.sweep --seeds 0 1 2 3 4 --phase all
# figures
$PY -m experiments.p0.analysis.plots
```

Budgets override via CLI (`--total-steps`), env (`P0_SMOKE`, `P0_TOTAL_STEPS`,
`P0_NUM_ENVS`, `P0_EVAL_EPISODES`), or config YAML.

## Causal & OOD tests

Ablations on the agent input (env/predictor untouched): `pe_shuffle`
(permute e,d across channels, ×3 distinct permutations + `pe_shuffle_D`
which must move D's features), `pe_mask` (zero all e,d), `pe_mask_ch`
(zero e,d on D only), `ch_permute` (permute channel blocks, ×3),
`mask_belief` (zero a channel's belief features), `fresh_pred`
(trained policy + re-initialized predictor). Predictor-provenance battery:
`trained_pred` / `trained_frozen` / `fresh_frozen` (trained-vs-fresh
predictor, with/without eval-time SGD). `learned_final_stoch` /
`bonly_final_stoch` are sampled-action evals of the final checkpoints
(separates policy collapse from an argmax artifact). The `bonly` agent is
the belief-only control: e,d masked at train AND eval — if it matches the
learned agent, surprise features are not load-bearing.
OOD via `configs/ood_*.yaml`: event frequency ↑↓, noise ↑, dynamics change,
cost ↑.

## Metrics

`detection_rate` (decided D events caught — primary), `mean_cost`,
`info_efficiency = detection_rate / mean_cost`, `total_reward`, attention
allocation, per-channel surprise, predictor error. Raw per-run JSON in
`reports/json/`, all rows in `reports/results.jsonl`; run artifacts
(checkpoints, metrics.jsonl, meta.json) under `runs/`.

See `reports/P0_RESULTS.md` for results and the PASS/FAIL verdict.
