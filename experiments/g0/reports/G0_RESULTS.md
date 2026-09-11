# G0 — Emergent Grounding: Results

**Question:** without any language labels, can reusable internal
concepts/categories form from sensation + transitions alone?

**Verdict: FAIL** (documented negative result). No representation forms
cross-context-invariant cause concepts. All decodable cause information
is attributable to instantaneous appearance plus random-projection
(reservoir) features — training adds no measurable grounding over an
*untrained* recurrent net. Details and per-criterion evidence below.

## Setup

- Environment: 16-dim sensor stream driven by 6 latent causes
  (NEUTRAL/HEAT/COLD/OSC/PULSE/DRIFT) with distinct dynamical signatures;
  10 sensor contexts (6 trained, ctx 6 = held-out signed permutation,
  ctx 8 = held-out dense rotation). Mid-episode context switches provide
  the invariance signal. Cause ids never reach any training objective.
- Representations: `raw`, `raw_win`, `pca`, `pca_win` (analytic);
  `ae`, `ae_vq`, `gru`, `gru_vq` (self-supervised recon / next-obs);
  `gru_km` (k-means on GRU hidden); `gru_untrained` (random-init GRU,
  shortcut control). 5 seeds each (env params re-drawn per seed).
- All metrics: linear probes / k-means on frozen latents, labels used at
  eval only. Chance = 1/6 ≈ 0.167 for cause probes, 0.5 for presence AUC.
- Raw JSONs: `reports/sweep_summary.json` (per-run + mean±std),
  `runs/*/eval.json`, `reports/eval_gru_untrained.json`,
  `reports/oracle_reference.json`. Figures: `reports/figs/`.

## Main table (mean over 5 seeds; full ±std in sweep_summary.json)

| rep          | acc_in | acc_loco | ood_ctx | dense_ctx | nmi_pool | nmi_ood | match_ood | combo_auc | midctx_lag |
|--------------|--------|----------|---------|-----------|----------|---------|-----------|-----------|------------|
| raw          | 0.269  | 0.146    | 0.164   | 0.165     | 0.123    | 0.268   | 0.154     | 0.569     | 0.369      |
| raw_win      | 0.266  | 0.169    | 0.179   | 0.171     | 0.082    | 0.160   | 0.190     | 0.573     | 0.329      |
| pca          | 0.229  | 0.180    | 0.162   | 0.189     | 0.124    | 0.268   | 0.251     | 0.559     | 0.357      |
| pca_win      | 0.217  | 0.173    | 0.190   | 0.178     | 0.075    | 0.144   | 0.210     | 0.543     | 0.327      |
| ae           | 0.255  | 0.236    | 0.248   | 0.226     | 0.109    | 0.249   | 0.251     | 0.575     | 0.459      |
| ae_vq        | 0.244  | 0.243    | 0.214   | 0.212     | 0.097    | 0.141   | 0.262     | 0.571     | 0.310      |
| gru          | 0.337  | 0.200    | 0.236   | 0.197     | 0.110    | 0.228   | 0.190     | 0.607     | 0.469      |
| gru_untrained| 0.328  | 0.201    | 0.224   | 0.207     | 0.097    | 0.216   | 0.195     | 0.606     | —          |
| gru_km       | 0.240  | 0.205    | 0.201   | 0.195     | 0.073    | 0.132   | 0.231     | 0.605     | 0.280      |
| gru_vq       | 0.223  | 0.200    | 0.195   | 0.201     | 0.098    | 0.145   | 0.267     | 0.580     | 0.280      |

Oracle reference (probe directly on the env's pre-context canonical
signal, eval-only): per-step acc_in ≈ 0.37–0.41, NMI ≈ 0.28–0.32,
segment-pooled acc ≈ 0.43–0.46, cross-context centroid matching
0.67–0.83. So even the *true* signal tops out near 0.4 per-step —
instantaneous appearance is genuinely ambiguous (the sign of the
intensity flips the observation direction; OSC is zero-mean) and
dynamical/contextual information is required.

## Per-criterion verdict

**(1) Captures latent cause better than raw/simple compression — FAIL.**
Trained `gru` leads acc_in (0.337±0.030) over `raw` (0.269±0.016), but
`gru_untrained` scores 0.328±0.030 — statistically identical. The gap
over baselines is produced by the recurrent architecture, not by
learning. `ae` shows the only learned-looking edge (loco 0.236 vs raw
0.146), still far below the oracle ceiling (~0.39). Best-val checkpoint
selection was applied (final checkpoints overfit); GRU best-val is
reached at step 50/3000 on every seed — training never improves the
objective beyond near-init, because the next-obs loss is dominated by
unpredictable noise dims (naive "predict no change" MSE 0.147 vs best
achieved ~0.145).

**(2) Representation reusable after context change — PARTIAL (weak).**
Above-chance transfer exists: loco up to 0.243 (`ae_vq`), held-out
contexts up to 0.248 (`ae`), and mid-episode context-switch probe
transfer 0.46 (`ae`, `gru`) vs 0.36 (`raw`) — the latent does carry some
context-tolerant cause content within an episode. But absolute transfer
is ~1.4–2× the 0.167 floor, not the oracle's range, and segment-level
`seg_acc_ood_ctx` ≤ 0.30.

**(3) Generalization on unseen compositions — PARTIAL (weak).**
Per-cause presence AUC on held-out cause pairs: `gru`/`gru_km`
0.61±0.01–0.02 vs 0.5 chance — a real but small effect; exact pair-set
accuracy ≤ 0.17 (chance ≈ 0.07). Noise-shift (≤0.35) and gain-shift
(≤0.38) degrade gracefully but remain weak; sensor dropout
(≤0.25) ≈ chance.

**(4) Not explainable by sensor shortcuts — FAIL (critical test).**
Same-cause/different-appearance cross-context centroid matching is at
chance for every rep (`match_ood` 0.15–0.27 vs oracle 0.5–0.83, margin
≤ −0.04 for all). The label-aligned time shuffle barely hurts
(`shuffle_aligned_acc` ≈ `acc_in` for all reps) → essentially all
decodable cause content is instantaneous appearance, not dynamics.
Sensor permutation collapses everyone to ≈ chance (≤0.22). Decoy
confusion 0.30–0.80, with no consistent HEAT↔COLD dominance — confusion
is driven by whichever random directions align, not by the designed
look-alike pair. Discrete bottlenecks (`ae_vq`, `gru_vq`, `gru_km`) do
produce stable codes (within-segment stability up to 0.86) but
code↔cause normalized MI ≤ 0.19 — the symbols don't align with causes.

**(5) Replicates across seeds — PASS (but replicates a null).**
Seed stds are tight (acc_in ±0.02–0.04, combo_auc ±0.01–0.03). The
negative result is stable, not a seed artifact.

## What emerged

- **Partial context tolerance in recurrent/encoder latents**: midctx
  pre→post-switch probe transfer ≈ 0.46 (`ae`, `gru`) clearly above the
  raw-obs baseline (0.37) and maintained out to +17–31 steps after the
  switch — the single strongest sign of context-invariant content.
- **Weak compositional signal**: held-out-pair presence AUC ≈ 0.6 for
  `gru`-family reps — better than all analytic baselines (~0.55–0.57).
- **Stable discrete symbols**: VQ/k-means codebooks produce
  within-segment-stable codes (0.61–0.86) — temporally coherent symbols
  exist, they just aren't cause-aligned.
- **Some downstream-task structure**: best-action probe ≈ 0.46–0.49
  (chance 0.25) for most reps — action-contingent regularities are
  captured even where cause identity is not.

## What did NOT emerge

- Context-invariant cause categories: cross-context matching ≈ chance;
  LOCO/held-out-context probes ≈ 0.2 vs oracle ≈ 0.8 territory.
- Cause-aligned discrete concepts: normalized MI(cause; code) ≤ 0.19.
- Intervention structure: decode-delta selectivity is *negative* for
  every trained model (−0.37 to −0.68) — latent directions that separate
  causes do not decode to the corresponding observation directions.
- Learned advantage over random features: `gru` ≈ `gru_untrained` on
  every metric (Δ ≤ 0.01–0.02, within noise).

## What can be explained by shortcuts

- **Random reservoir features**: an untrained GRU matches the trained
  one everywhere. Recurrent random projections + linear probes already
  expose what little cause signal exists in the stream.
- **Instantaneous appearance**: the label-aligned shuffle (order
  destroyed, per-step labels kept) costs ≤ 0.02–0.08 accuracy — almost
  all decodability is per-frame appearance, not temporal dynamics.
- **Episode/context nuisance block**: episode-demeaned probes do not
  improve held-out-context transfer (often hurt), i.e. there is no
  clean cause-subspace hiding under a context offset.

## Interpretation / caveats

- The objective is the bottleneck, not capacity: next-obs MSE is ~97%
  unpredictable noise (noise dims + read noise + process noise), and
  reconstruction has no reason to factor appearance from cause. The
  pressure to form cause concepts was too weak — a fair negative answer
  to the RQ *for these objectives at this data scale*.
- Best-val GRU checkpoints are all at step 50 — "trained vs untrained"
  equivalence is partly tautological, but the equality of the *learning
  curve's endpoint* with init is itself the finding.
- Probes are linear and per-step by design; segment pooling and the
  oracle bound show the headroom that a genuinely invariant rep could
  exploit — none did.
- `midctx_lag_mean` for `gru_untrained` was not computed (control was
  run for probe/match/OOD metrics only).

## Reproduction

    $PY -m experiments.g0.sweep --seeds 0 1 2 3 4 --quiet
    # control: untrained GRU (script used: /tmp/g0_untrained.py logic —
    # build_model('gru') without training, eval_representation over seeds)

Artifacts: `runs/<model>__seed<i>/{eval.json,metrics.json,run.json}`,
`reports/sweep_summary.json`, `reports/eval_gru_untrained.json`,
`reports/oracle_reference.json`, `reports/figs/*.png`.
