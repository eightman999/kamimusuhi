# CX0 Synthetic Cortex — Overnight Iteration Report

Session: autonomous a→b→c improvement + train/eval loop, local PC,
2026-09-14 23:35 → 2026-09-15 ~05:00 JST (with a ~02:53–03:10 pause before
review fixes resumed the loop).
Branch: `exp/cx0-synthetic-cortex`. Data: `experiments/cx0/runs_v2/`
(2,600 eval rows + field-lesion pass) and `experiments/cx0/runs_v3/`
(v4 protocol: redundant-recall cost), report: `reports/CX0_RESULTS.md`.

## Final verdict: PARTIAL

Gates: **C-G1 PASS** (best-arm mean clean 0.754) · **C-G2 FAIL** (generic
GRU still best — no structured-cortex advantage) · **C-G3 PASS** (organ
causality proven — shuffle drop 0.165, lesion drops up to 0.9) ·
**C-G4 PASS** (reset_hidden drop 0.239) · **C-G5 PASS** · **C-G6 PASS**
(context decode 0.59) · null check OK (cortex_off → ~0, drop 0.739).

## What the loop changed (v1 → v3)

| # | Change | Effect |
|---|---|---|
| a | `_memory_proof`: success on ctx1/2/4 requires the last env-visible recall to carry the needed etype | C-G3 became measurable; r0 went from decorative to load-bearing |
| a | R0 signal += direction pointer (shortest arc to recalled site); PPO nav shaping toward it | "go where memory says" is a readable organ output |
| b | CTX-2 phantoms persist 2 steps + dense during announce window | confirm-then-store learnable; ctx2 clean 0.81–0.91 across c0/c1/c2 (was ~0) |
| c | C3 tuning: slow τ64→32, cross_p 0.08→0.12, predict reads assoc+feedback (fast path) | did NOT rescue c3 on nav tasks (see findings) |
| + | Diverse episode seeds per BC round (was same 48 seeds recycled) | reduced overfitting to fixed layouts |
| + | Field-lesion conditions (`lesion_{h0,s0,t0,r0}`) added to eval battery | revealed dependence donor-shuffle can't see |
| + | Fixed `last_recall` 3-tuple in `get_state`/`set_state` (test regression) | suite green (19 pass, 1 skip) |

## Results — clean success, mean±sem (10 seeds)

| task | c0 flat | c1 GRU | c2 layered | c3 sparse |
|---|---|---|---|---|
| ctx1 | 0.654±.035 | **0.708**±.027 | 0.631±.034 | 0.023±.014 |
| ctx2 | 0.812±.014 | 0.898±.023 | **0.906**±.019 | 0.000±.000 |
| ctx3 | 0.856±.046 | **0.919**±.077 | 0.908±.054 | 0.829±.086 |
| ctx4 | 0.752±.025 | **0.756**±.023 | 0.592±.038 | 0.035±.016 |
| ctx5 | 0.510±.052 | 0.490±.046 | **0.529**±.036 | 0.519±.062 |
| **mean** | 0.717 | **0.754** | 0.713 | 0.281 |

Paired diffs: c2−c1 = −0.165±.039 (ctx4), −0.077±.040 (ctx1), ~0 else.
c0−c1 = −0.085±.031 on ctx2 only — recurrence pays exactly where context
must be held (waiting for the go window).

## Organ causality — the headline result

Donor-shuffle is blind to stereotyped signals, so field **lesions** were
added. Pooled lesion drops (clean − lesion):

| task | h0 | s0 | t0 | r0 |
|---|---|---|---|---|
| ctx1 | .37 | .09 | .10 | **.57** |
| ctx2 | .47 | .20 | **.52** | **.75** |
| ctx3 | .50 | .28 | **.63** | **.84** |
| ctx4 | .33 | .09 | **.58** | **.58** |
| ctx5 | .05 | .00 | −.01 | .00 |

Every organ field is load-bearing somewhere — the bundle is genuinely
integrative. Per-arm texture: **c0-flat leans hardest on organ
affordances** (lesion ≈ its whole clean score); c1/c2 partially
compensate in hidden state; **c3 routes attribution most strongly of any
arm** (ctx3 lesion_s0 +0.78) — the modular design does specialize, it
just can't execute.

## Findings

1. **Organ use is task-driven.** r0 only became causal once success
   required a real retrieval (proof gate). Same pattern would apply to
   s0/h0 if their shortcuts were closed.
2. **Generic GRU ≈ structured cortex at this scale.** C2 never wins
   significantly and loses ctx4 badly. C-G2 fails → **no cortex winner**
   → MIOBA substrate/genome integration stays gated out.
   *Diagnosed cause of the ctx4 gap:* c2 **recall-dithers** — in failed
   episodes it re-issues RECALL ~8× consecutively, each retrieval
   refreshing the direction pointer, and never commits to INTERACT
   (rollout trace: FWD×62, RECALL×8, INTERACT×0). The layered
   context pop appears to keep the "must recall" state alive. A small
   penalty on redundant recall (when a matching payload is already held)
   or a recall cooldown would likely close much of the −0.165 gap —
   queued as a candidate fix, not applied inside this window.
3. **C3's leaky sparse pops can't run multi-step programs.** BC fits
   (CE≈1.0) but execution collapses: ungated leak decays credit ~0.9^step
   over 80-step horizons. Four rescues failed (connectivity 0.3/0.5,
   dense assoc, predict fast-path, head access to assoc — best 0.125).
   Fine on short-horizon tasks (ctx3 0.83, ctx5 0.52).
4. **ctx5 is non-discriminative** (~0.5 all arms, zero lesion
   sensitivity) — probe-agreement metric too noisy.
5. Method note: c3 seeds 0–4 ran pre-rewire predict wiring, 5–9
   post-rewire; failure is consistent under both.

## Next candidates (unscheduled)

- s0: make last-write-wins lose (phantom stores corrupt matching slots,
  or attribution-gated store required) → attribution load-bearing for
  GRU arms too.
- c3: gated sparse populations / GRU-per-pop hybrid, or retire it as a
  documented negative result.
- ctx5: continuous agreement metric or denser probes.
- Decide whether "no cortex advantage at this scale" is the final C-G2
  answer or whether a harder task family is warranted before closing.

## v4 — recall-dithering candidate fix: TESTED, REJECTED

The queued fix from finding 2 was applied and measured (10-seed matrix,
`runs_v3/`, 3,400 rows): a 0.02 cost on RECALLs that re-fetch an
identical already-held payload (`RECALL_REDUNDANT_COST`, `ctx_world.py`).

| task/arm | v3 | v4 | Δ paired |
|---|---|---|---|
| ctx4/c2 | 0.592 | 0.690 | **+0.098 ± .052** — dithering reduced (s0 fail→pass; s1 3-recall fail → 1-recall) |
| ctx3/c2 | 0.908 | 0.794 | **−0.114** — pre-window recall block suppressed |
| ctx1/c2 | 0.631 | 0.583 | −0.048 ± .057 |
| ctx4/c1 | 0.756 | 0.760 | +0.004 ± .040 |
| ctx2 (all arms) | — | — | −0.03…−0.08 |
| **c2 mean** | 0.713 | 0.702 | **−0.011** |

Verdict on the fix: **rejected by the data**. It confirmed the dithering
mechanism (+0.098 on the diagnosed task) but regressed ctx3's legitimate
pre-window recall pattern (−0.114) and ctx2 broadly — net c2 −0.011, and
C-G2 still fails (c1 0.746 > c2 0.702). `RECALL_REDUNDANT_COST` is set
to 0.0 in `env/ctx_world.py`: mechanism kept, documented, inert —
current code ≡ v3 semantics, so `CX0_RESULTS.md` continues to report the
runs_v2 dataset. The v4 data stays in `runs_v3/` as the rejection
evidence. A targeted variant (penalty only on nav tasks, or a recall
cooldown instead of a cost) remains a candidate if the ctx4 gap is
revisited.

## Post-session note (kernel fix)

A post-window review found that `_perceived_event()` rolled fresh RNG
per call while being consumed by `step()`, `_obs()`, recall queries,
cause labels, and the oracle — obs and stored events could disagree
(~23% of steps). Perception is now rolled once per step and cached;
STORE on empty perception is a no-op. All data above was generated
under the pre-fix kernel (history.md §11); regenerate before citing
exact numbers.

A second pass then found `_check_success()` ran after `self.t += 1`,
judging the ctx2/ctx3 windows one step late (pre-window actions scored;
the last in-window step failed) and letting ctx1/ctx4 serve one step
before the crisis was observable. Success is now judged at action time;
`ctx_labels`, the validation-seed stride, and slot-memory clock/rng
were aligned in the same pass (history.md §12). Same data caveat.
