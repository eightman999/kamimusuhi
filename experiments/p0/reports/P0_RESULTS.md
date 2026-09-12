# P0 — Predictive Surprise: results (v3, second post-review revision)

**RQ.** Can an agent learn to selectively attend to channels it cannot
predict, instead of observing everything at high cost?

**Verdict: FAIL for the intended mechanism — documented negative result,
now stated more precisely.** PPO learns *channel selection* (watch only D)
but not gated *temporal* attention of any kind — surprise-gated or
belief-reactive. The env demonstrably admits a **belief-reactive frontier**
far above the pure-clock frontier (`vtrigger`, `cwatch` below), so the
failure is attributable to the learner, not the task. But — correcting the
v2 report — **no current env feature makes calibrated prediction
load-bearing in principle**: the same performance is reachable from raw
belief features (`v_D` threshold, C-jump detection). The strong claim
("surprise features necessary") is therefore untestable in this env, not
merely false for this learner.

All findings replicate across 5 seeds (605 eval rows).

---

## What this revision fixes (2nd adversarial review)

- **C-1a — new non-predictive baselines** `vtrigger` (burst D when the last
  read `v_D ≥ τ`; no predictor) and `cwatch` (poll C, burst D on detected
  C jumps; no predictor), run ID + causal subset + all 5 OOD configs.
- **C-1b — reframing.** v2 claimed "the only way past clock is prediction".
  False: RAMP mean (≈0.32) vs QUIET (≈0.15) is detectable from the raw
  observed value — `e_D` degenerates to a lagged deviation detector — and
  C is a leading indicator (≈54% of D ramps start during a C-jump boost,
  measured over 40 episodes), not a pure distractor. Docstrings/README
  corrected.
- **C-1c — vacuous fresh_pred fixed.** The v2 `fresh_pred` rows on
  `trigger`/`fixed` were self-evidently unchanged: baselines never had a
  trained predictor, so `id` already used a fresh one. Now a real
  provenance battery: `trained_pred` (predictor from the learned ckpt,
  keeps learning) vs `trained_frozen` vs `fresh_frozen`. Also noted: the
  predictor keeps training *during* eval episodes (training-time
  semantics), so eval episodes are not strictly iid and ablations couple
  back into the predictor via the policy's observation choices.
- **C-2a — stochastic evals of final ckpts.** `learned_final` /
  `bonly_final` re-evaluated with sampled actions.
- **C-2b — collapse mechanism corrected.** "Predictor starvation →
  absorbing IGNORE state" was wrong: `bonly` collapses identically with no
  predictor inputs at all, and the sampled final policies retain large
  OBS_D mass (below). Correct mechanism: late-training advantage
  flattening → IGNORE probability mass grows → the *argmax* flips to
  IGNORE while the sampled policy still watches D. An **argmax artifact**,
  not a dead policy.
- **M-1 — ckpt_best update numbers now reported** (they are mostly early:
  4/5 learned seeds = update 5).
- **M-2 — invariance claim now backed by runs** (`runs/probe_gru_s*`,
  `runs/probe_ent05_s*`, 300k steps each).
- **M-3 — s3 interpretation softened** + `pe_shuffle_D` (a permutation that
  must move D's e,d; `p1=[1,0,2,3]` left D untouched).
- **M-4 — mask_belief description fixed** (it kills only D-belief-dependent
  policies; always-D policies are unaffected).
- **Minor** — FixedAgent docstring updated to v2 semantics; s2 collapse
  timing corrected (first excursion ~u80, sustained ~u135); reward-model
  constant corrected (≈6.6p−8.4/ep, not 5.76p−8); event counts corrected
  (~8.4 decided events/ep, 94–106 per seed); eval non-iid noted.

## In-distribution results (mean ± std over 5 seeds, 12 ep each)

| agent | detection | cost/step | info-eff | reward/ep | attention |
|---|---|---|---|---|---|
| never | 0.000 | 0.0000 | 0.0 | −8.37 | IGN 1.00 |
| random | 0.324 ± .050 | 0.0234 | 13.9 | −15.12 | uniform |
| roundrobin | 0.248 ± .040 | 0.0200 | 11.9 | −14.61 | A–D .25 |
| always | 1.000 | 0.0600 | 16.7 | −22.07 | ALL 1.00 |
| clock1 | 1.000 | 0.0200 | 50.0 | −1.57 | D 1.00 |
| clock2 | 0.481 ± .044 | 0.0100 | 48.5 | −5.47 | D .50 |
| clock3 | 0.391 ± .023 | 0.0067 | 58.9 | −5.25 | D .33 |
| clock4 | 0.233 ± .024 | 0.0050 | 48.3 | −7.21 | D .25 |
| clock5 | 0.188 ± .009 | 0.0040 | 45.7 | −7.49 | D .20 |
| fixed | 0.406 ± .033 | 0.0072 | 57.0 | −5.25 | IGN .64/D .36 |
| pe_heuristic | 0.404 ± .083 | 0.0200 | 20.4 | −11.87 | D .39 |
| pe_thresh (θ=.35) | 0.081 ± .072 | 0.0024 | 5.8 | −8.15 | IGN .88 |
| trigger (e_D-gated) | 0.499 ± .051 | 0.0064 | 78.7 | −3.27 | IGN .68/D .32 |
| **vtrigger** (v_D≥0.30) | **0.586 ± .042** | **0.0038** | **153.0** | **−0.56** | IGN .81/D .19 |
| **vtrigger22** (v_D≥0.22) | **0.732 ± .052** | **0.0050** | **147.7** | **+1.46** | IGN .75/D .25 |
| **cwatch** (C-jump→D burst) | **0.807 ± .015** | **0.0113** | **70.1** | **−0.43** | IGN .43/D .38/C .18 |
| bonly (no surprise) | 0.872 ± .196 | 0.0161 | 66.4 | −1.66 | D .80 |
| learned (PPO+e,d) | 0.973 ± .061 | 0.0186 | 53.4 | −1.39 | D .93 |
| learned_final (argmax) | 0.000 | 0.0000 | 0.0 | −8.37 | IGN 1.00 |
| **learned_final_stoch** (sampled) | **0.835 ± .075** | 0.0160 | 52.2 | −2.21 | D .55 |
| **bonly_final_stoch** (sampled) | **0.826 ± .058** | 0.0170 | 48.6 | −3.24 | D .68 |

### Three frontiers

1. **Pure clock** (`clockK`, no feedback): flat eff ≈ 46–59 for
   det 0.19–1.00. Structurally dominated — the v1 flaw stays fixed.
2. **Reactive frontier** (belief- or surprise-gated): `vtrigger` eff
   **153** at det 0.59, `vtrigger22` eff **148** at det 0.73, `trigger`
   eff 79 at det 0.50, `cwatch` det 0.81 at cost 0.011. All dominate the
   clock frontier *at their detection level*.
3. **Learned**: sits ON the clock frontier (det 0.97 @ cost 0.019 ≈ clock1
   1.00 @ 0.020, eff ~53) — it found which channel, not when to look.

**The env is not prediction-required.** `vtrigger22` beats `trigger` on
detection AND efficiency using only `v_D`: a raw-value trigger suffices
because RAMP's mean elevation is directly visible in the observed value.
`cwatch` shows the C→D coupling is independently exploitable (det 0.81 —
higher than `trigger`'s 0.50 — by watching C and bursting D on jumps).
The e,d pathway is one sufficient trigger among several.

## Criterion 1 — info-efficiency vs random attention: **PASS (hollow)**

Learned eff 53.4 vs random 13.9 (3.8×). Achieved by a degenerate always-D
policy identical to `clock1` — and strictly dominated by `vtrigger22`
(det .73 @ eff 148) and `cwatch` (det .81 @ eff 70, at 0.6× the cost).

## Criterion 2 — cost < always-observe at comparable detection: **PARTIAL**

Literally met (0.019 vs 0.060 at det .97 vs 1.00) but learned merely ties
the best pure clock. The reactive frontier it *should* have found
(vtrigger-class, eff 148–153 at det 0.6–0.7; cwatch det 0.81 @ 0.011) is
2–3× better.

## Criterion 3 — degrades under PE intervention: **FAIL** (with nuance)

Best checkpoint (`learned`), pooled det over seeds:

| ablation | det | note |
|---|---|---|
| id | 0.973 ± .061 | — |
| pe_shuffle (×3 perms) | 0.968 ± .065 | no effect |
| pe_shuffle_D (must move D) | 0.841 ± .355 | only s3 drops (→0.21) |
| pe_mask (all ch) | 0.859 ± .315 | only s3 drops (→0.30) |
| pe_mask_D (D only) | 0.800 ± .447 | only s3 drops (→0.00) |
| ch_permute (×3 perms) | 0.991 ± .034 | no effect |
| mask_belief(D) | 0.800 ± .447 | only s3 drops (→0.00) |
| fresh_pred | 0.991 ± .020 | no effect |
| trained_frozen | 0.973 ± .061 | = id; eval-time learning irrelevant |
| fresh_frozen | 1.000 ± .000 | fresh frozen pred → always-D (s3 loses selectivity) |

The ckpt_best policy is **invariant to every surprise intervention on 4/5
seeds** — it ignores e,d entirely. s3 is the exception and its semantics
are now precisely characterized: pe_mask_D → 0.00 but pe_shuffle_D → 0.21
and fresh_pred → 0.96. So s3 needs e_D *nonzero*, not e_D *calibrated*:
a fresh predictor (large uncalibrated errors) preserves or improves det,
while e_D ≡ 0 acts as an OFF-switch quirk. **It depends on the e/d feature
pathway, not on calibrated predictions.** Ablation machinery validated by
controls: trigger collapses under pe_mask (0.50→0.14); bonly unaffected by
all pe_* (correct no-ops); `mask_belief(D)` zeroes D's belief features and
kills D-belief-dependent policies (vtrigger→0.00, cwatch→0.35, s3→0.00)
while always-D policies (learned 4/5, bonly s1/s3/s4) are unaffected —
exactly the expected semantics.

## Predictor-provenance battery (fixing the vacuous fresh_pred test)

| agent | id | trained_pred | trained_frozen | fresh_frozen |
|---|---|---|---|---|
| trigger | .499/.0064/78.7 | .524/.0064/84.1 | .450/.0062/73.4 | **.761/.0086/78.1** |
| fixed | .406/.0072/57.0 | .438/.0072/61.4 | .385/.0072/53.3 | .428/.0080/54.8 |
| learned | .973/.0186/53.4 | — | .973 (=id) | 1.000/.0200/50.0 |

(det / cost / eff; `—` = trained_pred ≡ id for learned since the ckpt
predictor is already the trained one.)

**A fresh predictor serves trigger at least as well as the trained one**
(0.761 ≥ 0.524): e_D is a lagged deviation detector — an untrained x̂
makes |x̂−v| large exactly when v is large, which is all the trigger
needs. Calibrated prediction contributes nothing even to the policy
designed to exploit it. For the learned policy, `trained_frozen` = id
shows eval-time predictor learning is irrelevant; `fresh_frozen` → 1.000
shows a saturated fresh e pushes the (invariant) policy to always-D.

## Criterion 4 — attention adapts under OOD: **FAIL**

Learned OBS_D share flat ≈0.91–0.99 under all five shifts; cost scales
passively (cost↑: 0.019→0.037, eff 50→26.5). The controls show what real
adaptation looks like: `trigger` OBS_D 0.27→0.56 under noise↑; `vtrigger`
maintains eff 76–164 across all OOD (degrading gracefully under noise↑
and cost↑ rather than collapsing); `cwatch` holds det 0.71–0.90 under
every shift. Learned does none of this.

## Criterion 5 — replication across seeds: **PASS** (of a degenerate outcome)

All 5 learned seeds: always-D within ~25 updates, hold det=1.0 for
100–200 updates, then argmax collapses to IGNORE (sustained onset
u135–225; s2 also had an early transient excursion at ~u80). bonly seeds
collapse similarly (3/5). Probe runs reproduce the same failure: GRU 2/3
collapse (u160, u175), ent_coef=0.05 3/3 (u105–180).

## The collapse, corrected (C-2)

**v2 story (wrong):** "policy tips toward IGNORE → predictor starves →
e,d carry no D signal → absorbing state." Counterevidence: `bonly` has NO
predictor inputs and collapses identically; and the sampled final policy
still detects events.

**Supported mechanism:** argmax-vs-sampling artifact.

| ckpt_final | argmax det | sampled det | sampled OBS_D share |
|---|---|---|---|
| learned s0 | 0.000 | 0.87 | .60 |
| learned s1 | 0.000 | 0.90 | .60 |
| learned s2 | 0.000 | 0.79 | .52 |
| learned s3 | 0.000 | 0.72 | .45 |
| learned s4 | 0.000 | 0.90 | .60 |
| bonly s0–s4 | 0.000 | 0.77–0.91 | .60–.87 |

Late in training the advantage signal flattens (always-D reward
≈ 6.6p−8.4/ep is near its p=1 optimum; the −1 miss penalty arrives
unconditioned), so IGNORE probability mass grows until the *argmax* flips.
The sampled policy retains substantial OBS_D mass — and, notably, is
**partially surprise-reactive**: pe_mask on sampled finals drops learned
det 0.835→0.555 (OBS_D share .55→.34), while bonly is correctly unchanged
(0.826→0.826). So the final learned policy does carry some e,d-dependence
that argmax never expresses; the best-checkpoint policy (the one that
matters for the RQ) remains e,d-invariant.

## M1 — ckpt_best selection, stated honestly

| run | best @ update | final @ update |
|---|---|---|
| learned s0–s4 | **5, 5, 5, 160, 5** | 292 all |
| bonly s0–s4 | 25, 5, 135, 5, 10 | 292 all |

4/5 learned ckpt_best are update 5 (step 5120) — a nearly-untrained
always-D policy chosen because nothing later beat its eval reward. bonly
s0's selective point (u25) is a transient lottery pick. s3's u160 best is
the only genuinely learned selective checkpoint. The "learned" row in
this report is therefore mostly "untrained always-D" — which is exactly
the point: PPO never found anything better than its first attractor.

## M2 — invariance probes (now with runs)

`runs/probe_{gru,ent05}_s{0,1,2}` (300k steps each): GRU collapses 2/3
(u160, u175; s2 stays always-D to the end), ent_coef=0.05 collapses 3/3
(u105–180). Same failure mode across architecture and entropy —
supporting, not proving, robustness of the collapse. The earlier v2 claim
of invariance was asserted without runs; it is now backed.

## OOD summary (mean det / cost / OBS_D share)

| agent | freq↑ | freq↓ | noise↑ | dynamics | cost↑ |
|---|---|---|---|---|---|
| learned | .97/.019/.95 | .98/.018/.92 | 1.00/.020/.99 | .95/.018/.91 | .97/.037/.93 |
| bonly | .87/.016/.80 | .85/.016/.78 | .69/.013/.66 | .83/.016/.78 | .83/.031/.78 |
| trigger | .52/.007/.34 | .58/.005/.27 | .71/.011/.56 | .39/.006/.29 | .50/.012/.30 |
| vtrigger | .56/.004/.20 | .58/.003/.17 | .49/.005/.24 | .39/.004/.18 | .58/.007/.18 |
| vtrigger22 | .72/.005/.26 | .71/.004/.22 | .61/.006/.31 | .52/.005/.24 | .73/.009/.23 |
| cwatch | .78/.011/.37 | .83/.011/.36 | .79/.014/.58 | .78/.012/.45 | .81/.022/.37 |
| fixed | .42/.007/.36 | .44/.007/.35 | .40/.008/.39 | .31/.007/.35 | .41/.014/.35 |
| clock2 | .51/.010/.50 | .51/.010/.50 | .54/.010/.50 | .48/.010/.50 | .50/.020/.50 |

## Caveats

- Eval episodes are not strictly iid: the predictor keeps training from
  the policy's observations across the 12-episode eval (EMA state resets,
  weights persist). Frozen-pred variants (`*_frozen`) isolate this; the
  effect on the learned agent is nil (trained_frozen = id).
- Under ablations the policy chooses different channels → the eval-time
  predictor trains on different data — ablation effects on e,d consumers
  are therefore slightly confounded by induced predictor drift (bounded
  by the trained_frozen/fresh_frozen comparisons).
- ckpt_best = best periodic-eval reward; on most seeds that is the early
  always-D point (update 5). Reported per-seed above.
- pe_thresh is weak (θ=0.35 starves it); kept as honest evidence that raw
  surprise-thresholding is not enough.
- Eval: 12 episodes/seed/condition, ~94–106 decided events per seed
  (~8.4/ep).
- Probe runs are 3 seeds each — enough to show the collapse recurs, not
  to bound its rate.
- vtrigger/cwatch are hand-coded operating points (τ, poll rates), tuned
  lightly; they are existence proofs for the belief-reactive frontier,
  not learned policies.
- v1 artifacts superseded; remain in git history.

## Per-criterion summary

| # | criterion | verdict |
|---|---|---|
| 1 | info-efficiency > random | PASS (hollow: clock-equivalent; dominated by belief-reactive baselines) |
| 2 | cost < always-observe at comparable det | PARTIAL (ties best clock; far below reactive frontier) |
| 3 | degrades under PE intervention | FAIL (best-ckpt invariant on 4/5 seeds; s3 needs e_D-nonzero, not calibrated prediction) |
| 4 | attention adapts under OOD | FAIL (flat allocation under all shifts) |
| 5 | replicates across seeds | PASS (same always-D → argmax-collapse dynamics ×5) |

**Overall: FAIL on the intended claim — documented and sharpened.** The
environment is fixed (pure-clock dominated; reactive frontier exists and
is reachable two different non-predictive ways), the evaluation machinery
is validated (controls behave correctly under every ablation), and the
learner still does not learn gated temporal attention — it collapses to
always-D at the checkpoint worth keeping and to IGNORE under argmax at
the end. The v2 claim that this env makes prediction *necessary* is
withdrawn: it does not.
