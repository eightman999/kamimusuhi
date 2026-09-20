# G0-v4 — Predictive Invariant Grounding

**Research question:** G0 showed that reconstruction / one-step
prediction objectives produce *no* context-invariant latent-cause
structure beyond an untrained reservoir. Was the objective simply too
weak — or does this setting resist self-supervised latent abstraction in
general? v4 re-tests with stronger sequence objectives:

| method | objective | latent |
|---|---|---|
| `gru`    | next-obs delta MSE — the G0 predictor control | GRU hidden |
| `cpc`    | InfoNCE: `W_k c_t` predicts `z_{t+k}`; negatives = other episodes/timesteps | GRU context `c_t` |
| `vicreg` | two obs-augmented views → invariance+variance+covariance, plus latent forward prediction | GRU hidden `h_t` |
| `jepa`   | action-conditioned latent rollout vs EMA target encoder (t+1..t+K); var/cov collapse guard | GRU context `c_t` |

Environment, data protocol, eval battery, probes and metric names are
reused **unchanged** from `experiments/g0` — v4 imports them directly.

## Rules that differ from a typical run

- Training sees **only** `obs / next_obs / actions` — `train.sanitize`
  whitelists keys; `protocol_validator.py` statically scans the
  training-path modules for label access and audits `meta.json`.
- Every method is evaluated **trained and as an untrained twin** (same
  arch, same init seed). The central quantity is the paired
  `trained − untrained` diff, plus the raw/PCA/`dynfeat`/`gru` controls.
- Augmentations use obs jitter + sensor-dim dropout only — never the
  true context transform `C_j` (that would teach invariance, not let it
  emerge).
- Run artifacts: `latest.pt` `best.pt` `config.yaml` `meta.json`
  `metrics.jsonl` every `v4.log_every` updates. CUDA OOM halves the
  batch once; second OOM fails the run. `--device=cuda*` on a non-CUDA
  box aborts (no silent CPU fallback).
- Collapse monitoring: per-dim variance, effective rank, cosine
  distribution, covariance RMS in `metrics.jsonl` and `eval.json`
  (`health.*`). Collapsed runs are flagged, not averaged away.

## Layout

    config.py              Config = g0 sections + `v4:` hyper-params
    models.py              CPCModel / VICRegModel / JEPAModel (+ gru ctl)
    losses.py              InfoNCE, VICReg terms, variance/covariance
    augment.py             legal view augmentations (obs-only)
    health.py              collapse metrics
    train.py               trainer (artifact + OOM spec)
    evaluate.py            G0 battery wrapper + health
    sweep.py               methods x seeds driver -> reports/
    protocol_validator.py  leakage/label/artifact checks (exit≠0 = FAIL)
    report.py              writes reports/G0_V4_RESULTS.md
    configs/{default,smoke}.yaml
    tests/

## Reproduce

    PY=<venv python with torch+yaml+matplotlib>

    # tests
    $PY -m pytest experiments/g0_v4/tests -q

    # protocol self-check (data scope, eval isolation, label access)
    $PY -m experiments.g0_v4.protocol_validator \
        --config experiments/g0_v4/configs/default.yaml

    # stage 0: smoke wiring
    $PY -m experiments.g0_v4.train --method cpc \
        --config experiments/g0_v4/configs/smoke.yaml --device cuda

    # stage 1: pilot tournament (4 methods x seed 0, full battery)
    $PY -m experiments.g0_v4.sweep --seeds 0 --device cuda

    # stage 2: survivors x seeds 0..4
    $PY -m experiments.g0_v4.sweep --seeds 0 1 2 3 4 \
        --methods cpc jepa --device cuda --quiet

    # validate run artifacts + generate report
    $PY -m experiments.g0_v4.protocol_validator \
        --run-dirs experiments/g0_v4/runs/* --device cuda
    $PY -m experiments.g0_v4.report

Verdict thresholds (spec §10-14, §25) are hard-coded in `report.py`
(`H1_H2_DELTA`, `NULL_MARGIN`, `H4_AUC`) so they cannot drift after the
results are in.
