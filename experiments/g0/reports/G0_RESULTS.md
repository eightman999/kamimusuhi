# G0 — Emergent Grounding: Results (v2, post-review)

**Question:** without any language labels, can reusable internal
concepts/categories form from sensation + transitions alone?

**Verdict: FAIL** — cleanly, and more strongly than the v1 report
claimed. With the eval-harness bugs fixed, models genuinely *do* learn
(same-env validation improves 37–96% over naive), but the learned
latents carry no more latent-cause structure than an *untrained* random
recurrent/encoder network, and the corrected context-transfer test sits
at chance. The one v1 "emerged" item that looked real (mid-episode
context-switch transfer 0.46) was a data-collection bug.

v1 artifacts (buggy metrics) are preserved under `archive/v1/`.

## Review fixes applied

| # | Bug | Fix | Consequence |
|---|-----|-----|-------------|
| C1 | `set_context()` leaked its context into the entire next episode (`_ctx_forced` consumed by `reset()`) — 47/48 "midctx" episodes were single-context | `set_context` now only touches the live episode; regression tests added | midctx metric now measures true ctx0→ctx6 switches (48/48 episodes) |
| C2 | val split used `seed+777` as *env_seed* → different world; best-val pinned at step ~50, looked like "no learning" | val = held-out episodes of the SAME env (`env_seed=seed`), decorrelated noise via `rng_seed` | real learning curves restored; the "noise-dominated" diagnosis was wrong |
| C3 | oracle ceiling measured only instantaneous decodability (~0.4) | added `dynfeat_*` pseudo-reps: dynamical-signature segment features on canonical signal (headroom) and raw obs (baseline) | true dynamical headroom ≈ 0.44–0.48 segment acc across contexts |
| M1 | v1 wrongly claimed untrained-GRU lacked midctx metrics | corrected; those metrics exist (0.44–0.53 ≈ trained) and themselves showed midctx was a reservoir artifact | — |
| M2 | "~97% unpredictable noise" wrong | recomputed: same-env naive delta-MSE 0.192, noise floor ≈0.05 → ~74% predictable; GRU reaches 0.122 (37% reduction vs naive), AE 0.023 (96% reduction vs recon-naive) | learning happens; invariance doesn't |
| M4 | combo presence-AUC probed within train contexts (appearance detectors suffice) | added `combo_oodctx`: held-out pairs in the held-out context | strict test ≈ chance (see below) |
| M5 | no nonlinear/dynamical probe on latents | `dynseg_*` metrics: dynamical features of each rep's latents, segment-probed | headroom-vs-achieved gap quantified |
| minors | best_action TAP spike added to all actions (dead code); centroids missed seg_warmup; explore_policy function-attribute state; untrained control in /tmp | fixed; ExplorePolicy class; controls now in `sweep.py` (`--untrained`); oracle reference extended to 5 seeds | — |

## Setup (unchanged design)

6 latent causes with distinct dynamical signatures; 16-dim obs through
per-context sensor matrices (6 train ctxs; ctx 6 = held-out signed
permutation, ctx 8 = held-out dense rotation). Labels eval-only.
Reps: `raw, raw_win, pca, pca_win` (analytic), `ae, ae_vq, gru, gru_vq`
(self-supervised), `gru_km` (post-hoc k-means), `gru_untrained,
ae_untrained` (reservoir controls), `dynfeat_obs, dynfeat_canonical`
(dynamical-feature references). 5 seeds; env params re-drawn per seed.

## Main table (mean over 5 seeds; ±std in sweep_summary.json)

| rep               | acc_in | acc_loco | ood_ctx | dynseg_loco | dynseg_ood | nmi_pool | match_ood | midctx* | combo_oodctx_auc |
|-------------------|--------|----------|---------|-------------|------------|----------|-----------|---------|------------------|
| raw               | 0.308  | 0.227    | 0.213   | 0.428       | 0.477      | 0.124    | 0.146     | 0.172   | 0.520            |
| raw_win           | 0.238  | 0.203    | 0.207   | 0.440       | 0.390      | 0.086    | 0.208     | 0.172   | 0.517            |
| pca               | 0.315  | 0.264    | 0.195   | 0.391       | 0.411      | 0.145    | 0.256     | 0.225   | 0.514            |
| pca_win           | 0.263  | 0.224    | 0.196   | 0.340       | 0.338      | 0.097    | 0.246     | 0.218   | 0.511            |
| ae                | 0.369  | 0.278    | 0.285   | 0.376       | 0.367      | 0.123    | 0.285     | 0.186   | 0.532            |
| ae_vq             | 0.339  | 0.271    | 0.199   | 0.339       | 0.267      | 0.123    | 0.272     | 0.198   | 0.516            |
| ae_untrained      | 0.327  | 0.287    | 0.246   | 0.309       | 0.311      | 0.127    | 0.315     | 0.253   | 0.521            |
| gru               | 0.360  | 0.251    | 0.237   | 0.333       | 0.351      | 0.099    | 0.238     | 0.206   | 0.514            |
| gru_untrained     | 0.359  | 0.248    | 0.280   | 0.366       | 0.363      | 0.115    | 0.246     | 0.204   | 0.555            |
| gru_km            | 0.338  | 0.276    | 0.221   | 0.251       | 0.228      | 0.088    | 0.256     | 0.188   | 0.539            |
| gru_vq            | 0.270  | 0.219    | 0.165   | 0.239       | 0.251      | 0.029    | 0.226     | 0.219   | 0.515            |
| **dynfeat_obs**   | —      | —        | —       | —           | —          | —        | —         | —       | —                |
| **dynfeat_canon** | —      | —        | —       | —           | —          | —        | —         | —       | —                |

`dynfeat_*` rows occupy the segment-probe slots instead:
`dynfeat_obs` seg_acc_in 0.383 / seg_loco 0.444 / seg_ood 0.480;
`dynfeat_canonical` 0.372 / 0.469 / 0.461.

\* corrected metric (true mid-episode switches). Chance: cause probes
1/6 ≈ 0.167; presence AUC 0.5.

Oracle instantaneous reference (canonical signal, 5 seeds): per-step
acc_in 0.35–0.49, cross-context matching 0.33–0.67. Dynamical headroom
(dynfeat_*) is higher: LOCO ~0.45–0.47 — the causes are primarily
*identifiable through dynamics*, exactly what the reps fail to expose.

## Per-criterion verdict

**(1) Captures latent cause better than raw/simple compression — FAIL.**
Best trained rep `ae` acc_in 0.369±0.025 vs `raw` 0.308 — but
`ae_untrained` 0.327 and `gru` vs `gru_untrained` 0.360 vs 0.359
(identical). No learned rep separates from its reservoir control, and
the dynamical headroom (~0.47 seg_loco) is unexploited.

**(2) Reusable after context change — FAIL** (was PARTIAL in v1).
The corrected mid-episode switch test: raw 0.172, ae 0.186, gru 0.206,
gru_untrained 0.204, ae_untrained 0.253 — all ≈ chance 0.167, and the
best number belongs to an *untrained* net. LOCO/held-out-ctx probes sit
0.2–0.29 for everyone (trained and untrained alike).

**(3) Unseen compositions — FAIL** (downgraded). Within-ctx combo
presence AUC (0.55–0.62) collapses to 0.51–0.56 ≈ chance when the held-
out pairs are rendered in the held-out context (`combo_oodctx_auc`) —
the earlier signal was per-cause appearance detectors, not composition.

**(4) Not explainable by sensor shortcuts — FAIL.**
Cross-context same-cause matching `match_ood` 0.15–0.32 ≈ chance
(oracle 0.33–0.67 instantaneous, ~0.46+ dynamical); `ae_untrained`
(0.315) beats every trained rep. Aligned-shuffle (labels permuted with
steps) costs only Δ≈0.02–0.08 → most decodable content is instantaneous
appearance. Sensor permutation → 0.21–0.30. Learned latents' *own*
dynamical signatures (`dynseg_ood` 0.23–0.37) are BELOW the same
features computed on raw obs (0.48) — the reps discard dynamical
information the sensors already carry.

**(5) Replicates across seeds — PASS** (replicates a null; stds ±0.01–0.05).

## What emerged

- **Best-action structure**: ~0.43–0.49 (chance 0.25) for all reps —
  action-contingent regularities are captured (equally by untrained
  nets).
- **Stable discrete symbols**: within-segment code stability 0.62–0.83
  (`ae_vq`, `gru_km`) — but MI(code;cause) normalized ≤ 0.23, so the
  symbols aren't cause concepts.
- Nothing else survives. The v1 midctx item was the C1 bug.

## What did NOT emerge

- Context-invariant cause categories: matching ≈ chance, LOCO ≤0.29.
- Any trained-vs-untrained gap on any metric (the decisive null).
- Cause-aligned discrete codes (norm-MI ≤ 0.23).
- Intervention structure: decode-delta selectivity negative for all
  trained models (−0.40 to −0.56; untrained nets −0.25 to −0.28).
- Boundary/cause-switch detection beyond chance (AUC ≈ 0.5).

## What can be explained by shortcuts

- **Reservoir features**: untrained nets match (or beat) trained ones on
  essentially every metric — now shown under *healthy* training, so it
  is not a selection artifact.
- **Instantaneous appearance**: aligned-shuffle deltas small for all.
- **Appearance detectors masquerading as composition**: the within-ctx
  combo AUC evaporates in the held-out context.
- The v1 midctx "transfer" was a collection bug, not emergence.

## Corrected interpretation

The earlier "objective is noise-dominated, no learning happens" story
was wrong — an artifact of cross-world validation. The true picture:
**models learn plenty (37–96% of achievable loss reduction) but learn
env-specific appearance/prediction structure that does not factor into
context-invariant cause variables.** The dynamical signature that would
identify a cause is present in the sensor stream (handcrafted features
reach ~0.47 cross-context) yet is *not* what next-obs prediction or
reconstruction preferentially encodes. Emergent grounding requires more
than generic self-supervision at this scale — that is the substantive
negative finding.

## Caveats

- Probes are linear on latents; `dynseg_*` adds a fixed nonlinear
  dynamical readout but a learned nonlinear probe could differ.
- Same-env val means "held-out episodes of the same world"; train and
  val cause-segment schedules share the env-seed-derived stream before
  the rng decorrelation — verified episodes differ (test asserts it).
- `dynfeat_*` headroom is a reference point, not an upper bound.

## Reproduction

    $PY -m experiments.g0.sweep --seeds 0 1 2 3 4 --quiet

Artifacts: `runs/<model>__seed<i>/{eval,metrics,run}.json`,
`reports/sweep_summary.json`, `reports/oracle_reference.json`,
`reports/figs/*.png`, `archive/v1/` (pre-review results).
