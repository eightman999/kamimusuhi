# O0 — Object Permanence: Results (v3, second-review revision)

**Question.** After a target disappears behind an occluder, can an agent
maintain its existence, position, and identity in an internal state —
i.e. track something it cannot currently see?

**Verdict: PARTIAL.** The central phenomenon is real and now verified
two independent ways — a causal intervention (hidden-state reset) and a
linear probe (the recurrent state decodes true hidden position at
R² ≈ 0.9–1.0).  But the *comparative* claim fails: a hand-crafted
baseline that knows both the occlusion bounds and the true motion model
(`corridordyn`, added in this review round) beats every learned model on
position tracking in essentially every condition, and the earlier
"corridor" const-velocity baseline still beats or matches the best
learned model almost everywhere.  What the learned models genuinely
provide: memory-dependent tracking from raw observations (the MLP cannot
do it at all), near-perfect re-identification, graceful degradation to
3–4× the nominal trained occlusion length, and partial pull-resistance
under decoy takeovers.  What they do not provide: exit/death detection
(except SSM's duration-prior approximation) and any advantage over a
two-trick heuristic on this simple dynamics.

v1 (PGONE=0.2 training distribution) is preserved under
`artifacts/runs_v1_pgone02/` and `reports/v1_pgone02/` — its numbers are
superseded by the corrected no-gone distribution below.

---

## Environment (v2, post-review)

- **Training distribution**: `occl_len ~ U{4..16}`, `p_gone=0`,
  `p_swap=0.3`, 2 distractors, drag 0.4–1.2, |v| 0.02–0.12.
  **Disclosed caveat (round 2): "trained on 4–16" is approximate.**
  Drag slows the target and occluder width is clipped to the world, so
  realized bouts exceed the sampled range: mean realized bout ≈ 12,
  16.5% of episodes have bouts >16, 13% of hidden steps sit at depth
  k>16, tail to ~89 steps (eval p99 ≈ 37).  Length-transfer claims
  should therefore be read as "extrapolation beyond the bulk of
  training", not strict zero-shot beyond 16.
- **Boundary absorption** (`gone20` eval preset): edge-touching
  occluders hide the target reaching the world boundary while occluded
  → permanently absorbed (`exist=0` on hidden steps).
- **Takeover channel** (`ambush`/`decoytk` presets): during occlusion a
  moving distractor may enter the occluder and occupy the *target
  channel* while the true target remains hidden
  (`decoy_takeover_prob`; 30% of ambush episodes).  Decoy steps are
  `visible=1` in the label sense but the reported position/appearance
  belong to the decoy — `data["decoy"]=1` marks them.  In decoytk the
  true target keeps its own motion and *persists* after the decoy exits
  (re-appearance resumes).
- Leakage tests: counterfactual hidden-position/path/speed,
  reappearance-time independence, distractor zeroing — all pass
  (26 tests).

## Models

| model | params | note |
|---|---|---|
| mlp | 23k | stateless baseline |
| ssm | 8k | gated scalar-channel SSM (64 ch) |
| gru64 | 27k | GRU, h=64 |
| lstm64 | 35k | LSTM, h=64 (state packed h‖c) |
| gru128 | 105k | GRU, h=128 |

## Heuristic baselines (all parameter-free, use true world params)

| name | position | identity | existence |
|---|---|---|---|
| prior | predict 0.5 | match | always exists |
| lastobs | last seen x | match | always exists |
| constvel | dead-reckon last x,v̂ | match | gone iff x+v̂ leaves world |
| corridor | constvel clipped to [occ_lo,occ_hi] | match | gone iff guess exits bounds |
| openloop | propagate x,v under true drag; no corrections while hidden | match | same exit rule |
| corridordyn | openloop dynamics + clip to occluder bounds (round-2) | match | same exit rule |
| kalman | constant-velocity Kalman on true x,v (approximate: diagonal Q, accel modeled as random walk though true accel is per-episode constant) | match | same exit rule |

Note on interventions: `hidden_reset`/`hidden_noise` act on each
predictor's state — model h (and c for LSTM), heuristic `last_x,last_v`.
The operation is equivalent in spirit, not strictly identical across
predictors (reviewer minor).

## Headline: occlusion position MAE (mean ± sd over 5 seeds; heuristics deterministic)

Canonical `id` preset, hidden steps of the first bout, alive steps only.

| predictor | pos_mae_occluded | persistent ≤16 | bouts >16 | gone | pre-reappear err | exist_acc_hidden | id_acc |
|---|---|---|---|---|---|---|---|
| prior | 0.0498 | 0.0440 | 0.0619 | – | 0.0800 | 1.000 | 1.000 |
| lastobs | 0.1103 | 0.0966 | 0.1394 | – | 0.1740 | 1.000 | 1.000 |
| constvel | 0.0198 | 0.0077 | 0.0455 | – | 0.0206 | 0.996 | 1.000 |
| corridor | 0.0109 | **0.0070** | 0.0192 | – | 0.0076 | 0.996 | 1.000 |
| openloop | 0.0164 | 0.0080 | 0.0344 | – | 0.0174 | 0.996 | 0.998 |
| **corridordyn** | **0.0107** | 0.0076 | **0.0172** | – | 0.0116 | 0.996 | 0.998 |
| kalman | 0.0199 | 0.0113 | 0.0381 | – | 0.0295 | 0.998 | 1.000 |
| mlp | 0.0510 ± .0004 | 0.0453 | 0.0629 | – | 0.0767 | 0.000 | 0.607 ± .026 |
| ssm | 0.0226 ± .0013 | 0.0190 | 0.0302 | – | 0.0343 | 0.9999 | 0.557 ± .023 |
| gru64 | 0.0177 ± .0059 | 0.0149 | 0.0245 | – | 0.0261 | 1.000 | 0.815 ± .201 |
| **gru128** | **0.0123 ± .0011** | 0.0099 | 0.0175 | – | **0.0190** | 1.000 | **0.995 ± .004** |
| lstm64 | 0.0163 ± .0025 | 0.0131 | 0.0218 | – | 0.0246 | 1.000 | 0.569 ± .026 |

Reading (corrected in round 2): on the strict trained-range slice
(bouts ≤16) the order is
**corridor < corridordyn < constvel < openloop < gru128 < kalman <
lstm64 < gru64 < ssm** — gru128 *does* beat kalman here (earlier text
reversed this) but not the clip/dynamics baselines.  On the aggregate
the same ordering holds except constvel/openloop/kalman fall below the
top learned models (long-bout steps dominate).  **gru128 beats every
heuristic except the two corridor variants on aggregate, and beats none
of them on the trained range.**  Existence while hidden is trivial for
everyone on this distribution (nothing ever disappears) — except the
MLP, which scores 0: it has no memory and cannot represent "still
exists but unobserved".  Identity is solved only by gru128 (0.995);
gru64 is bimodal across seeds (0.57–0.99).

## Criterion 1 — beats heuristic + MLP baselines: **PARTIAL → leaning FAIL**

- vs MLP: clear pass (0.012–0.023 vs 0.051; MLP also fails existence).
- vs prior/lastobs/constvel/openloop/kalman on the aggregate: pass.
- vs corridor: **fail** (0.0109 < 0.0123; paired episode bootstrap on
  `id`: Δ = +0.0027, 95% CI [+0.0022, +0.0031], P(model wins) = 0.000).
- vs corridordyn (openloop dynamics + bounds clip, the strongest
  baseline): **fail everywhere** — aggregate 0.0107 vs 0.0123,
  trained-range 0.0076 vs 0.0099, and on every OOD preset below.
- Honest residual advantage: gru128 does beat constvel/openloop/kalman
  on the *long-bout tail* and under motion shifts (below).  The
  learned model is competitive with the best hand-crafted tracker
  without being handed the dynamics — but it does not beat it.

## Criterion 2 — occlusion-length transfer: **PARTIAL**

Force `occl_len ≡ 24/32/48` (`plen*`, pure: no gone episodes):

| predictor | plen24 (realized ~33) | plen32 (~44) | plen48 (~54, tail 92) |
|---|---|---|---|
| constvel | 0.0888 | 0.1087 | 0.1207 |
| corridor | 0.0368 | 0.0513 | 0.0749 |
| openloop | 0.0478 | 0.0717 | 0.0837 |
| **corridordyn** | **0.0334** | **0.0496** | **0.0675** |
| kalman | 0.0900 | 0.1282 | 0.1735 |
| gru128 | 0.0439 | 0.0716 | 0.1012 |
| gru64 | 0.0518 | 0.0807 | 0.1126 |
| lstm64 | 0.0554 | 0.0892 | 0.1291 |
| ssm | 0.0665 | 0.0984 | 0.1290 |
| mlp | 0.100 | 0.130 | 0.159 |

(see `err_vs_occstep_occ48.png`: error grows roughly linearly with
occlusion step for all predictors; gru128 stays under 0.04 through
k≈40.)

- vs constvel: gru128 wins the *step-weighted* metric (0.101 vs 0.121)
  but **loses episode-weighted** (0.095 vs 0.090; paired bootstrap
  Δ = +0.006, CI [−0.003, +0.015], P(win) = 0.09): the win is carried
  by the long-bout tail, not the typical episode.
- vs corridor/corridordyn: **loses at every length** (plen48: 0.075 and
  0.068 vs 0.101).  Degradation remains graceful (0.0123→0.101 over
  ~4× the nominal trained range, never catastrophic), but the claim
  "learned model beats heuristics at long occlusion" is false once the
  strongest baselines are included.  kalman, notably, collapses
  (0.174) — its diagonal-Q approximation mishandles long horizons.
- vs v1: no change in conclusion; realized-bout disclosure added above.

## Criterion 3 — hidden-state causal tests: **PASS**

`reset@mid_occl` recomputed on **5-seed means, post-anchor window only**
(hidden-alive steps after the mid-occlusion anchor — the fair
denominator; earlier table mixed seed-0 values):

| predictor | post-window MAE, none | post-window MAE, reset | degradation |
|---|---|---|---|
| prior | 0.0585 | 0.1352 | 2.3× |
| constvel | 0.0342 | 0.4983 | 14.6× |
| corridor | 0.0156 | 0.1025 | 6.6× |
| openloop | 0.0276 | 0.4983 | 18.1× |
| corridordyn | 0.0160 | 0.1025 | 6.4× |
| kalman | 0.0358 | 0.4983 | 13.9× |
| lastobs | 0.1664 | 0.4983 | 3.0× |
| mlp | 0.0564 | 0.0564 | 1.0× |
| ssm | 0.0235 | 0.1281 | **5.4×** |
| gru64 | 0.0204 | 0.1604 | **7.8×** |
| gru128 | 0.0152 | 0.1626 | **10.7×** |
| lstm64 | 0.0188 | 0.1056 | **5.6×** |

(True degradation range ≈ 5.4–10.7× for recurrent models — earlier
"7–12×" overstated lstm/ssm.  High heuristic ratios are trivial:
resetting `last_x,last_v` destroys the whole tracker.)

- Post-reset, gru128's hidden-step predictions revert to its
  pre-appearance prior: pos 0.31 vs 0.27 pre-appearance, exist prob
  **0.0008 vs 0.0009** (earlier "0.07" misreported a different mask),
  vel 0.057 vs 0.046; predicted existence after reset is 0.00 —
  the model behaves as if it never saw the target.
- `reset@mid_vis` control (5-seed hidden-window MAE vs none):
  mlp 1.0×, ssm 1.5×, lstm 1.9×, gru64 2.5×, **gru128 3.5×**
  (0.0435 vs 0.0123).  **Not near-free** — the residual damage mixes
  genuine visible-phase warm-up with post-reset tracking loss for
  episodes still occluded at the anchor; it weakens the contrast with
  mid_occl (10.7×) but the direction is unambiguous.
- `hidden_noise`: see `causal_pos_mae.png`; monotone degradation with σ.
- **Linear probe (new, round 2)** — `extras.py::hidden_probe`,
  `probe_pos_r2.png`: ridge regression on the recurrent state decodes
  true position at occlusion depth k with held-out-episode R²:
  gru128 1.00 (k1) → 0.89 (k16); gru64 1.00→0.91; lstm64 0.99→0.95;
  ssm 0.98→0.83; mlp ≈ 0 (no state, control).  Velocity similarly
  (0.99→0.70–0.93).  This is direct, non-causal confirmation that the
  state *is* a readable object-position estimate, decaying with depth —
  matching the linear error growth.  On `gone20`, the same probe cannot
  decode the dead/alive distinction (R² ≤ 0 at every depth with dead
  steps): existence is carried in the *output head's* duration decay,
  not linearly in the state.

## Criterion 4 — distractor tracking: **PARTIAL (revised)**

Fixed-slot distractors were nearly free (all predictors ≥0.97 identity
on `id`).  The takeover condition is the real test (62% of decoytk
episodes affected; mean 5.5 decoy steps, p90 = 8, max = 75):

- Positional pull: every predictor is pulled toward the decoy.
  Decoy-step pos error: **recurrent learned 0.112–0.119**
  (lstm .112, gru128 .117, gru64 .117, ssm .119) vs kalman .121,
  mlp .143, constvel/corridor/corridordyn .142 — recurrent models
  resist better than kinematic baselines, which have no
  appearance/mismatch signal.
- Decoy identity rejection (`id_acc_decoy`, lower = more fooled):
  corridordyn/constvel-family 0.585–0.699 (pos+appearance rule partially
  detects mismatches), **gru128 0.563** best learned; gru64 0.465,
  lstm 0.501, ssm 0.407, mlp 0.284.
- Overall decoytk: corridor-family 0.0070–0.0078 < gru128 0.0112 <
  lstm 0.0159 < gru64 0.0172 < ssm 0.0228; gru128 overall id_acc 0.822
  vs heuristics 0.666.  The learned tracker is meaningfully more
  robust to channel-takeover than kinematic baselines on identity, but
  is still fooled >40% of decoy steps — a real, non-catastrophic
  failure mode.

## Criterion 5 — seed replication: **PASS**

Position MAE seed-σ ≤ 0.006 for every learned model; per-episode 95% CI
on `id` ≤ ±0.0013 (gru128 ±0.0005).  Identity: gru128 0.995 ± .004
stable; **gru64 is bimodal** (0.57–0.99) — disclosed, not hidden by the
mean.

## OOD suite (`none` intervention, 5-seed means, `pos_mae_occluded`)

(`occ24/32/48` = fixed long occlusion + narrow 2–4-step visible window,
slow narrow velocity band, restricted spawn — a harder composite
condition than `plen*`'s pure length shift.)

| preset | constvel | corridor | corridordyn | kalman | mlp | ssm | gru64 | **gru128** | lstm64 |
|---|---|---|---|---|---|---|---|---|---|
| id | .0198 | **.0109** | **.0107** | .0199 | .0510 | .0226 | .0177 | .0123 | .0163 |
| plen48 | .1207 | .0749 | **.0675** | .1735 | .159 | .129 | .113 | .101 | .129 |
| occ48 | .1478 | .0943 | **.0853** | .2612 | .160 | .153 | .139 | .130 | .162 |
| gone20 | .0863 | .0637 | **.0484** | .1037 | .123 | .099 | .089 | .076 | .099 |
| fast_v | .0107 | .0095 | **.0092** | .0124 | .051 | .042 | .028 | .019 | .037 |
| slow_v | .0423 | .0159 | **.0152** | .0556 | .087 | .027 | .021 | .0150 | .020 |
| drag_x3 | .0528 | .0212 | **.0162** | .0358 | .093 | .026 | .022 | .0173 | .020 |
| v_flip | .0390 | .0352 | **.0338** | .0385 | .115 | .041 | .038 | .0350 | .037 |
| distractors4 | .0164 | .0097 | **.0092** | .0188 | .051 | .025 | .020 | .0134 | .018 |
| ambush | .0200 | **.0105** | .0116 | .0218 | .050 | .024 | .020 | .0137 | .0175 |
| decoytk | .0076 | **.0070** | .0075 | .0102 | .047 | .023 | .017 | .0112 | .016 |
| app_jitter | .0249 | **.0109** | .0111 | .0243 | .050 | .023 | .018 | .0127 | .016 |
| swap_half | .0282 | **.0117** | .0116 | .0316 | .049 | .023 | .018 | .0127 | .017 |

Bold = best in row (learned bold only where a learned model is best;
corridor-family shown separately since corridordyn is the strongest
predictor overall).

**Corrected OOD claims (M-a).**  Earlier text claimed gru128 wins on
slow_v/drag_x3/v_flip/app_jitter/swap_half.  Seed-resolved truth:

- vs **corridor**: gru128 wins cleanly (all 5 seeds) only on
  **drag_x3** (.0164–.0193 < .0212); slow_v is a thin mean win whose
  worst seed loses (s1 .0165 > .0159); v_flip is a tie
  (.0350 vs .0352); app_jitter (.0127 vs .0109) and swap_half
  (.0127 vs .0117) are losses on every seed.
- vs **corridordyn**: gru128 has **no clean win anywhere** — it loses
  or ties on every preset including drag_x3 (.0162 < .0173), which
  eliminates "learned adaptive dynamics" as a selling point on this
  task: handing the tracker true drag is strictly better.
- vs **constvel/openloop/kalman**: gru128 wins clearly on
  slow_v (Δ −0.0055, CI excludes 0, P=1.00), v_flip (P=1.00),
  drag_x3 (P=1.00), swap_half (P=0.75), app_jitter (P=0.63), and the
  long-bout step-weighted metrics; **loses on fast_v** (0.019 vs
  0.009–0.012 — under-extrapolation at |v|=0.25 persists).
- Paired episode-resampled bootstrap CIs (2000 resamples, shared eval
  stream) in `reports/extras.json::paired_bootstrap`: no comparison
  against corridor reaches P(win) ≥ 0.10 on any preset.

**Existence generalization (`gone20`, never trained)** — corrected (M-b).
`exist_acc_absorbed` is measured over *all* hidden steps of absorbed
episodes (alive fraction = 0.544 — an always-exists predictor scores
exactly that):

| predictor | exist_acc_hidden | exist_acc_absorbed | dead steps detected | false-gone on alive |
|---|---|---|---|---|
| constvel | 0.912 | 0.926 | exit rule — true detector | rare |
| corridordyn | 0.921 | 0.871 | exit rule (clipped) | rare |
| kalman | 0.865 | 0.786 | exit rule on noisy estimate | rare |
| ssm | 0.815 | **0.818** | **88%** | **22%** |
| gru64/gru128/lstm64 | ≈0.81 | **0.569–0.571** | **~10%** | ~0% |
| mlp | 0.314 | 0.518 | — (broken: also fails alive-hidden) | — |

- GRU/LSTM are near always-exists: gru128 detects only ~10% of
  dead-hidden steps (mean exist output 0.12–0.18 at deep k — suppressed,
  mostly above the 0.5 threshold).  They learned permanence *too* well.
- **SSM partially detects death but via the wrong mechanism**: its
  exist output decays with occlusion depth on edge-touching occluders
  (alive-hidden exist_pred 0.98 at k=4 → 0.83 at k=24 → 0.54 at k=40),
  i.e. it learned "long occlusion ⇒ probably gone" from the training
  bout-length distribution.  On gone20 that scores 0.82 — but it
  false-declares gone on 22% of alive-hidden steps in absorbed episodes
  (rising toward ~46% at k≈40).  The linear probe confirms the state
  does not encode the dead/alive distinction (R² ≤ 0).
- No learned model approaches the explicit exit rule (0.79–0.93).
  Existence prediction under true disappearance is the weakest axis of
  the learned models.

## What the experiment does and does not show

**Established.** (a) A memoryless model cannot do the task (MLP:
0.051, exist-acc 0) — memory is causally required.  (b) Trained
recurrent models maintain a linearly-decodable object state (pos R²
≈0.9–1.0 through k=16) that is causally necessary: zeroing it mid-
occlusion destroys tracking (5–11× post-window degradation) and snaps
predictions back to the pre-appearance prior.  (c) gru128
re-identifies the target with 0.995 accuracy and is the most
pull-resistant predictor under channel takeover.  (d) Degradation with
occlusion length is graceful, not catastrophic, to ~4× the nominal
trained range.

**Not established.** (a) Any position-tracking advantage over the best
hand-crafted baseline: `corridordyn` dominates everywhere, `corridor`
loses only on drag_x3/long-bout-tail step-weighted metrics.  On this
nearly-linear dynamics, "know the bounds + know the dynamics" is the
optimal policy and the learned model only approximates it.
(b) Death/exit detection: no learned model implements it (SSM
approximates a duration prior with ~30% false-gone at deep k).
(c) Strict 4–16 training (realized bouts reach ~89; see disclosure).

## Caveats / remaining shortcuts

- All models were trained with occlusion ~4–16 sampled; realized bouts
  exceed that for ~16.5% of episodes (max ~89).  Length-transfer is
  "beyond the bulk", not strict.
- Heuristics are given true world parameters (bounds, drag, noise);
  the learned models must infer them — the comparison is informative
  but not fair in the reverse direction either.
- The reset intervention is not strictly equivalent across predictors
  (model h‖c vs heuristic scalar state).
- `pos_mae_visible` under decoytk includes takeover steps (~0.12 error
  each); `pos_mae_visible_clean` (new metric, excludes decoy steps) is
  ~30% lower — e.g. gru128 0.0215→0.0154.
- pos_mae_occluded is step-weighted; episode-weighted ranking on long
  bouts can flip (plen48: constvel wins episode-weighted).
- kalman's Q is a diagonal approximation and its accel model is a
  random walk (true accel is per-episode constant); a filter matched
  exactly to the generative model would do better.
- Eval used a shared fixed stream (512 episodes/condition,
  seed 910000): identical episodes across predictors, enabling the
  paired bootstrap; condition estimates have ~±0.001–0.003 CI95.

## Reproducibility

```
python -m experiments.o0.sweep --config experiments/o0/configs/full.yaml \
    --seeds 0 1 2 3 4 --models mlp gru64 gru128 lstm64 ssm
python -m experiments.o0.analysis.extras --config experiments/o0/configs/full.yaml
```

Raw rows: `reports/raw/all_results.json` (682 + corridordyn rows).
Extras: `reports/extras.json`.  Plots: `causal_pos_mae.png`,
`err_vs_boutlen.png`, `err_vs_occstep_{id,occ48}.png`, `ood_pos_mae.png`,
`probe_pos_r2.png`.  Checkpoints `artifacts/runs/`; training curves in
`metrics.jsonl` per run.
