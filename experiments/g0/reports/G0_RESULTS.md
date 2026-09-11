# G0 — Emergent Grounding: Results Report

**Question:** without any language labels, can reusable internal
concepts/categories form from sensation + transitions alone?

**Verdict: FAIL** — clean, reproducible, and sharper than v1: after
fixing the evaluation harness so that *learning is verified to happen*,
the trained representations still show no advantage over untrained
reservoirs on any invariance metric, and the two effects that survived
v2's controls are now themselves controlled away. Small learned gains
exist on in-context/appearance-level metrics — but nothing resembling
emergent latent-cause concepts.

Generated artifacts: `sweep_summary.json`, `controls_untrained.json`,
`oracle_reference.json`, `figs/*.png`; per-run JSONs in `runs/`.
The buggy pre-fix artifacts are preserved under `archive/v1/`.

## What was fixed between v1 → v2 → v3

| # | v1/v2 bug | fix | effect |
|---|---|---|---|
| C1 | `set_context()` left `_ctx_forced` set → `can_switch` false in eval; `midctx` measured a within-ctx0 probe + ~48% leaked-context train episodes | forced flag cleared on switch; `force_ctx` start uses episode-parity ctx (always a *train* ctx) | midctx is now a real ctx0→held-out-ctx6 test; 48/48 eps switch; train ctx coverage uniform |
| C2 | val split used `seed+777` as *env_seed* → different world; best-val pinned at step ~50, looked like "no learning" | val = held-out episodes of the SAME env (`env_seed=seed`), decorrelated noise via `rng_seed` | real learning curves restored; the "noise-dominated" diagnosis was wrong |
| C3 | no dynamical/invariance headroom references | `probes/dynamical.py` segment features + `dynfeat_canonical`/`dynfeat_obs` evals + nonlinear `dynseg_*` latent probes | headroom ~0.44–0.48 exists and is not reached by any learned rep |
| v3 | best_action "chance=0.25" baseline was wrong (TAP-majority ≈0.47); discrete stability had no untrained control; "no trained-vs-untrained gap" overstated; no empirical nulls | `best_action_maj_baseline`/`_delta`; `ae_vq_untrained` + `gru_untrained_km` controls; paired per-seed diffs (`controls.py`); label-shuffle nulls `midctx_acc_null` / `match_ood_ctx_null`; dynseg delta now uses `encode(next_obs)` (correct action pairing for instantaneous reps) | the two v2 "emerged" items dissolve (below) |

## Environment

6 latent causes (NEUTRAL/HEAT/COLD/OSC/PULSE/DRIFT) drive a 16-d
multi-channel observation through per-context signed-permutation (+gain)
matrices `C_j`: `obs = C_ctx·(W·(x_a v_a + x_b v_b) + distractors) +
noise`. Dynamics are cause-specific (decay, action responses,
oscillation, Poisson spikes, TAP-triggered spikes, drift). Single causes
or unordered pairs; 6 train contexts, ctx6 held out (permutation+gain),
ctx8 held out (dense mixing). Cause/context IDs are eval-only metadata —
training sees (obs, action, next_obs) only.

## Learning happens (verified)

| model | same-env naive | unpredictable floor | best val | reduction |
|---|---|---|---|---|
| AE (recon) | 0.606 | ≈0.023 (obs-noise dims) | 0.023 | ≈floor, ~100% of achievable |
| GRU (delta-pred) | 0.192 | ≈0.051 | 0.122 | 37% of total ≈ **50% of the above-floor predictable share** |

GRU best-val at steps ~1300–1850 of 3000 — real optimisation, not a
step-50 artifact. So the null result below cannot be explained by
"nothing was learned".

## Results (5 seeds, mean ± std)

`acc_*` = instantaneous cause probe; `dynseg_*` = nonlinear probe on
dynamical-signature features of the rep; `match_ood` = cross-context
same-cause centroid match, with `null` = label-shuffle floor;
`midctx` = probe trained pre-switch (ctx0), tested post-switch in held-out
ctx6 — the harder direction (train→held-out, not train→train);
`ba_delta` = best-action acc minus its majority-class baseline
(≈0.473 — TAP is argmax for 3/6 causes, so 1/4 is the wrong floor).

| rep | acc_in | loco | ood | dynseg loco | dynseg ood | NMI | match_ood | null | midctx | null | ba_delta | combo_oodctx | stab | MI |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| raw | .308 | .227 | .213 | .444 | .480 | .124 | .146 | .185 | .172 | .175 | −.002 | .520 | — | — |
| raw_win | .238 | .203 | .207 | .466 | .431 | .086 | .208 | .164 | .172 | .168 | −.112 | .517 | — | — |
| pca | .315 | .264 | .195 | .413 | .412 | .145 | .256 | .159 | .225 | .209 | −.018 | .514 | — | — |
| pca_win | .263 | .224 | .196 | .379 | .324 | .097 | .246 | .149 | .218 | .210 | −.065 | .511 | — | — |
| **ae** | .369 | .278 | .285 | .404 | .368 | .123 | .285 | .179 | .186 | .179 | +.011 | .532 | — | — |
| **ae_vq** | .339 | .271 | .199 | .331 | .262 | .123 | .272 | .187 | .198 | .179 | −.015 | .516 | .621 | .229 |
| **ae_vq_untr** | .281 | .288 | .222 | .272 | .261 | .101 | .274 | .190 | .226 | .203 | −.006 | .550 | **.809** | .087 |
| **gru** | .360 | .251 | .237 | .339 | .346 | .099 | .238 | .182 | .206 | .188 | +.014 | .514 | — | — |
| **gru_untr** | .359 | .248 | .280 | .383 | .419 | .115 | .246 | .200 | .204 | .186 | −.031 | .555 | — | — |
| **gru_km** | .338 | .276 | .221 | .264 | .254 | .088 | .256 | .187 | .188 | .179 | −.003 | .539 | .826 | .203 |
| **gru_untr_km** | .297 | .274 | .180 | .293 | .244 | .086 | .246 | .164 | .187 | .172 | +.012 | .521 | **.852** | .184 |
| **gru_vq** | .270 | .219 | .165 | .230 | .275 | .029 | .226 | .195 | .219 | .194 | −.044 | .515 | .286 | .123 |
| **ae_untr** | .327 | .287 | .246 | .336 | .331 | .127 | .315 | .192 | .253 | .203 | −.013 | .521 | — | — |
| dynfeat_obs* | seg .383 | .444 | .480 | — | — | — | — | — | — | — | — | — | — | — |
| dynfeat_canon* | seg .372 | .469 | .461 | — | — | — | — | — | — | — | — | — | — | — |

\* headroom references (fixed dynamical features, no representation
learning): even with **no learning at all**, `dynfeat_obs` on raw
observations reaches ~0.44–0.48 cross-context segment accuracy —
above every learned rep's `dynseg` row. The features are invariant to
signed permutation but *not* to the ctx6 gain rescaling (limitation).

Oracle references (5 seeds): canonical-signal instantaneous probe
0.35–0.49 in-ctx; segment 0.32–0.41; NMI 0.31–0.36; cross-context
matching 0.33–0.67 — instantaneous cause identity is intrinsically
hard (signed intensity, OSC/PULSE temporal ambiguity), so `acc_*` rows
understate what any rep could show; the `seg_*`/`dynseg_*` rows and the
matching/midctx columns are the decisive evidence.

## Paired trained − untrained diffs (per seed)

| pair | acc_in | acc_loco | acc_ood | dynseg_loco | midctx | match_ood | ba_delta | NMI | combo_ood |
|---|---|---|---|---|---|---|---|---|---|
| gru − untr | +.001 (3/5) | +.002 | **−.043 (0/5)** | −.044 (0/5) | +.002 | −.008 | **+.045 (5/5)** | −.016 | −.041 (0/5) |
| ae − untr | **+.042 (5/5)** | −.009 | +.038 (4/5) | **+.068 (5/5)** | −.067 | −.031 | +.025 (4/5) | −.004 | +.011 |
| ae_vq − untr | **+.058 (4/5)** | −.017 | −.023 | +.058 (4/5) | −.029 | −.003 | −.009 | +.022 | −.034 |
| gru_km − untr_km | **+.041 (5/5)** | +.003 | **+.041 (5/5)** | −.030 | +.001 | +.010 | −.015 | +.003 | +.019 |

The v2 claim "no trained-vs-untrained gap" was overstated: **small
learned gaps exist on appearance-level metrics** — in-context probing
(+0.04–0.06, 4–5/5 seeds), AE dynamical features (+0.068, 5/5), GRU
best-action over its own majority baseline (+0.045, 5/5), and gru_km's
held-out-ctx probe (+0.041, 5/5). But on the **direct invariance
measures** — LOCO, mid-episode context transfer, cross-context
same-cause matching, OOD dynamical probing, NMI, held-out-ctx
composition — the gap is ≈0 or negative, and `midctx`/`match_ood`/
`gru acc_ood_ctx` actually favor the *untrained* nets.

## Empirical nulls

Label-shuffle floors (per-episode label permutation for midctx;
centroid-label permutation for matching) give `midctx_null` ≈ 0.17–0.20
and `match_ood_null` ≈ 0.16–0.20. Observed `midctx` ≈ 0.17–0.25 sits
essentially **at** its null; `match_ood` ≈ 0.15–0.32 sits weakly above
its null — but the strongest above-null matcher is `ae_untrained`
(0.315), so this residual transfer is reservoir-level, not
learning-derived. Lag-resolved midctx shows real *short-lived*
carryover for recurrent reps: gru 0.239 at lag 1–4 decaying to ~0.20
by lag 17–31 (≈null) — and `gru_untrained` shows the same profile
(0.252 → 0.183). Consistent with hidden-state carryover, not concept
reuse.

## Per-criterion verdict

| Criterion | verdict | evidence |
|---|---|---|
| capture latent cause better than raw/compression | **PARTIAL→FAIL** | in-ctx ae .369 / gru .360 vs raw .308 (matched by ae_untrained .327, gru_untrained .359); under held-out ctx all reps ≈0.17–0.29 vs oracle's own ~0.4 ceiling; dynseg nonlinear probes don't rescue OOD |
| reuse after context change | **FAIL** | loco ≤.29; midctx ≈ label-shuffle null; match_ood above its null only at reservoir level (max = untrained) |
| generalize to unseen compositions | **FAIL** | combo presence AUC .51–.55 in-ctx and .51–.55 held-out-ctx (`combo_oodctx`) ≈ 0.5; set-mixture acc at chance; decoy confusability .26–.51 |
| resist sensor shortcuts | **FAIL** | same-cause/diff-appearance matching ~0.2–0.3 (null ~0.18); perm −2–−29pts; dropout −6–−13pts; latent-intervention selectivity **negative** (−0.26 to −0.65) for all trained models |
| replicate across seeds | **PASS** | 5 seeds, all conclusions stable; stds ≪ effect gaps; paired-diff sign patterns consistent |

## What emerged

**Essentially nothing beyond reservoir-level structure.** The two
survivors of the v2 report are controlled away:

- ~~best-action structure above "chance"~~ — vs the correct
  majority-class baseline (≈0.473), trained reps reach at most +0.014
  (gru) and raw/untrained sit at or below it. A *paired* learned gap
  exists (gru beats gru_untrained by +0.045, 5/5 seeds) but the
  absolute effect is ~1.4 points over majority voting — a thin,
  appearance-level signal, not a concept.
- ~~stable discrete codes~~ — untrained `ae_vq` is **more** stable
  than trained (0.809 vs 0.621); untrained `gru`+k-means likewise
  (0.852 vs 0.826); code–cause MI is trained 0.20–0.23 vs untrained
  0.09–0.18. Stability is temporal smoothness of the reservoir, and
  the small MI edge does not survive any transfer test (ae_vq loco
  diff −0.017).

What remains honestly attributable to learning: small in-context and
appearance-level gains over the untrained twins (acc_in +0.04–0.06;
ae dynseg_loco +0.068; gru best-action-delta +0.045; gru_km
acc_ood_ctx +0.041 — all 4–5/5 seeds positive). Real, consistent, and
far short of the claim.

## What did NOT emerge

- context-invariant cause categories (matching, LOCO, midctx all at
  reservoir/null level);
- discrete symbols aligned to causes (best VQ MI 0.23 while
  generalizing *worse* than continuous reps; untrained codes equally
  stable);
- compositional structure (held-out-composition AUC ≈ 0.5, in *and*
  out of context);
- intervention-consistent latents (selectivity negative);
- any learned advantage on invariance metrics.

## What can be explained by shortcuts

- **reservoir dynamics**: `gru_untrained` matches `gru` on nearly every
  metric, `ae_untrained` leads on midctx/match_ood — high-dimensional
  random nonlinear features + temporal smoothing explain essentially
  all above-null performance;
- **appearance statistics**: in-context probe gains and the
  in-ctx-only half of combo AUC track context-specific sensor patterns;
- **code stability ≠ semantics**: stability measures temporal
  smoothness, which untrained smooth dynamics provide *more* of;
- **the midctx lag-1–4 bump** is hidden-state carryover (identical in
  the untrained GRU), not abstraction.

## Caveats

- All probes are linear or fixed-feature (dynseg); a learned nonlinear
  probe could extract more — but `dynseg` already gives each rep the
  same hand-built dynamical features that reach 0.44–0.48 on raw obs.
- dynfeat is perm/sign-invariant, not gain-invariant; ctx6's 0.6–1.6
  rescaling degrades it somewhat — its ~0.46 OOD is therefore a
  *lower* bound on dynamical headroom.
- midctx tests only the ctx0→ctx6 direction (train→held-out) — the
  harder test, so weak numbers are expected, but the null-calibration
  and untrained parity hold regardless.
- AE best-val ≈ the reconstruction noise floor: reconstruction can be
  "perfect" while encoding nothing causal — another reason in-context
  probe deltas are the meaningful comparison, not loss.
- VQ collapse observed on some seeds (gru_vq stability 0.29); VQ
  results are a lower bound on what a better discrete method might do.
- Figures: `figs/probe_acc.png`, `figs/nmi_match.png`,
  `figs/causal.png`, `figs/ood_combo.png`.

## Bottom line

Under a verified-working training pipeline and a fixed evaluation
harness, sensation+transitions alone produced **no reusable,
context-invariant, compositional concept structure** in any of 11
representation families × 5 seeds — only environment-specific
appearance/prediction features plus generic reservoir structure. The
negative is informative: the failure is not "no learning" (val curves,
paired diffs, and best-action delta all show learning) but that what
is learned does not factor into cause variables that survive context
change. Closing the gap likely needs objectives/inductive biases
explicitly targeting invariance (e.g., contrastive cross-context
alignment, disentanglement pressure, or explicit segment/event
structure) rather than more prediction or reconstruction.
