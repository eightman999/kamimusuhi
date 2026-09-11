# G0 — Emergent Grounding

**Research question:** without any language labels, can reusable internal
concepts/categories form from sensation + transitions alone?

The agent sees a stream of 16-dim sensor observations and chooses one of
4 actions. The world is driven by one (sometimes two) of 6 **latent
causes** (NEUTRAL, HEAT, COLD, OSC, PULSE, DRIFT). Each cause has a fixed
feature vector `v_c` and a fixed *behavioural signature* (decay,
per-action response, oscillation, spiking). Observations are rendered
through a per-**context** sensor matrix `C_k` (signed permutation or
dense rotation) on top of a canonical readout `W`, plus noise and
cause-independent distractor dims:

    x(t+1) = decay_c * x(t) + resp_c[action] + noise        (per cause)
    obs(t) = C_ctx @ (W @ sum_c x_c v_c  +  distractors) + read_noise

- The same cause looks **very different** across contexts (different
  appearance); different causes can look **similar** within a context
  (HEAT vs COLD are near-negated feature vectors). Instantaneous
  appearance is ambiguous; the *dynamical* signature is not.
- Cause ids, context ids, intensities, segment ids are logged for
  **evaluation only**. Training objectives use `obs`/`next_obs`/`actions`
  exclusively — there is no supervised pathway to cause labels.

## Layout

    env/dynamics.py         cause table, context matrices, EnvConfig
    env/latent_cause_env.py gym-like env + rollout helpers (eval hooks:
                            set_context / set_segment / set_action_gain /
                            set_pair_set / get_state / set_state)
    models/net.py, vq.py    ae, ae_vq (VQ-VAE), gru, gru_vq
    representations/base.py raw, raw_win, pca, pca_win, torch wrappers,
                            k-means discretization
    probes/                 logistic/ridge/few-shot probes, k-means,
                            NMI/purity, cross-context centroid matching
    analysis/metrics.py     eval helpers; analysis/plots.py figures
    data.py                 dataset collection (labels stored eval-only)
    train.py                self-supervised training loop
    evaluate.py             the full evaluation battery
    sweep.py                multi-seed driver -> reports/sweep_summary.json
    configs/{default,smoke}.yaml
    tests/                  env / model / pipeline tests
    runs/<model>__seed<i>/  ckpt.pt (gitignored), eval.json, metrics.json
    reports/                sweep_summary.json, figs/, G0_RESULTS.md
    archive/v1/             first-round results (pre-review; eval-harness
                            bugs C1/C2 still present — kept for audit)

## Representations compared

| rep      | kind                                   | supervision |
|----------|----------------------------------------|-------------|
| raw      | obs as-is                              | —           |
| raw_win  | last-8 [obs, action] concat            | —           |
| pca      | per-step PCA (8)                       | —           |
| pca_win  | windowed PCA (16)                      | —           |
| ae       | MLP autoencoder bottleneck (8)         | recon       |
| ae_vq    | VQ-VAE (discrete bottleneck, 24 codes) | recon       |
| gru      | action-conditioned GRU next-obs pred.  | prediction  |
| gru_vq   | GRU -> VQ code -> next-obs head        | prediction  |
| gru_km   | k-means (16) over GRU hidden states    | post-hoc    |

## Evaluation battery (`evaluate.py`)

- **probes**: linear cause probe (episode-held-out), leave-one-context-out
  (LOCO), held-out permutation context (ctx 6), held-out dense-rotation
  context (ctx 8), few-shot (1/5/20) in- and out-of-context, best-action
  transfer, boundary detection, intensity regression, segment-level
  "concept" probes.
- **clustering**: k-means vs true causes — NMI/purity pooled, per-context,
  and on the held-out context.
- **matching** (*same cause / different appearance*): per-cause latent
  centroids matched across contexts by nearest-centroid cosine —
  the critical test that appearance alone cannot pass.
- **causal**: time-shuffle (labels fixed + label-aligned variants),
  sensor-dim permutation, sensor dropout, mid-episode context switch
  (pre->post probe transfer + carryover by lag), decoy confusion rate.
- **ood**: unseen context, unseen dense context, noise shift, action-gain
  shift, novel cause compositions (held-out pairs -> per-cause presence
  AUC + exact pair-set accuracy), and held-out pairs rendered in the
  held-out context (true compositional + appearance transfer).
- **dynamical probes**: per-segment dynamical-signature features
  (autocorr/spectrum/action-conditional response/spike stats — all
  permutation- and sign-invariant) probed per rep (`dynseg_*`), plus
  `dynfeat_canonical` (true-signal headroom) and `dynfeat_obs`
  (no-learning baseline) pseudo-reps in the sweep.
- **controls**: `gru_untrained` / `ae_untrained` reservoir controls are
  evaluated by the sweep per seed.
- **discrete**: code usage, MI(cause; code), within-segment stability.
- **intervention**: decode-delta directions vs true per-context cause
  prototypes (selectivity = diag - max off-diag cosine).

## Reproduce

    PY=/Users/eightman/dev/sandbox/kamimusuhi/.venv/bin/python
    cd <worktree root>

    # tests
    $PY -m pytest experiments/g0/tests -q

    # quick wiring check
    $PY -m experiments.g0.sweep --config experiments/g0/configs/smoke.yaml \
        --seeds 0 --quiet

    # full run (trains 4 models x N seeds, evaluates 9 reps)
    $PY -m experiments.g0.sweep --seeds 0 1 2 3 4 --quiet

    # single model / single rep
    $PY -m experiments.g0.train --model gru_vq --seed 0 --quiet
    $PY -m experiments.g0.evaluate --run-dir experiments/g0/runs/gru_vq__seed0
    $PY -m experiments.g0.evaluate --rep raw_win --seed 0

Results: `experiments/g0/reports/sweep_summary.json` (per-run + mean±std),
figures in `reports/figs/`, analysis in `reports/G0_RESULTS.md`.
