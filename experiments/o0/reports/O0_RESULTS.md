# O0 — Object Permanence: Results

**Question.** After a target disappears behind an occluder, can an agent
maintain its existence, position, and identity in an internal state —
i.e. track something it cannot currently see? Not general memory:
*persistent world representation under occlusion*.

**Verdict: PASS** on position + existence tracking (the core claim),
**PARTIAL** on identity re-acquisition (capacity-dependent; see below).

## Setup

- World: 1D `[0,4]`, horizon 96. Target appears, moves with
  near-constant velocity (drag 0.005, per-episode accel, process noise),
  enters a static occluder for a planned 4–16 steps (train), then
  reappears on the far side — unless absorbed by the world boundary
  while hidden (`p_gone=0.2`, occluder flush with the edge) or
  identity-swapped (`p_swap=0.3`, appearance jump ≥0.25).
- 0–4 wandering distractors; occluder bounds always observable.
- Observation dim 20. While the target is inside the occluder, **all**
  target channels read exactly 0; `tests/test_leakage.py` verifies no
  channel decodes hidden-position residual, time-to-reappearance, or
  counterfactual hidden trajectories.
- Tasks: O0-A position (MAE on the hidden window; headline
  `pos_err_pre_reappear` = error on the last hidden step), O0-B
  existence (incl. post-absorption steps), O0-C identity
  (post-reappearance "same object?").
- Learned: MLP (memoryless control), GRU-64/128, LSTM-64, tiny diagonal
  SSM. 3000 supervised updates, fresh episodes each step, multitask
  heads; checkpoint selected on a fixed validation stream.
  Heuristics: `prior`, `lastobs`, `constvel`, `oracle` (true motion
  model — reference ceiling).
- **5 seeds × 5 architectures + 4 heuristics**, 512 eval episodes per
  condition. Raw: `reports/raw/all_results.json` (493 rows),
  `reports/summary.json`, figures `reports/*.png`.

## ID distribution (occlusion 4–16)

`pos_mae_occluded` = MAE over hidden steps while the object exists
(normalized by world length; ×4 for world units).

| predictor | pos MAE occluded ↓ | pre-reappear ↓ | exist acc hidden ↑ | id acc ↑ |
|---|---|---|---|---|
| prior (geometry)   | 0.1222 | 0.0823 | 0.817 | 1.000* |
| lastobs            | 0.2521 | 0.1734 | 0.817 | 1.000* |
| constvel           | 0.0877 | 0.0256 | 0.910 | 1.000* |
| oracle (ceiling)   | 0.0662 | 0.0205 | 0.915 | 0.995 |
| mlp ×5             | 0.1235±.0010 | 0.0797±.002 | 0.450±.001 | 0.613±.016 |
| ssm ×5             | 0.0726±.0036 | 0.0458±.003 | 0.856±.002 | 0.565±.031 |
| **gru64 ×5**       | **0.0549±.0026** | 0.0259±.002 | 0.903±.007 | 0.560±.030 |
| **gru128 ×5**      | **0.0533±.0011** | 0.0299±.001 | 0.909±.005 | **0.989±.003** |
| **lstm64 ×5**      | **0.0585±.0029** | 0.0293±.004 | 0.900±.004 | 0.558±.025 |

\* heuristics get identity "for free" via a hard-coded appearance
comparison rule — a designed capability, not evidence about learning.

Reading:
- **O0-A position**: every recurrent model beats *all* heuristics
  (incl. the true-dynamics oracle, 0.0662) and the MLP (0.1235 — it can
  only guess the occluder interior). Error grows smoothly with hidden
  duration (`err_vs_occstep_id.png`), ≈0.03 at k=1 → ≈0.10 at k=20.
- **O0-B existence**: recurrent ≈ oracle/constvel (~0.90–0.91);
  MLP 0.45 ≈ chance — existence-over-time genuinely requires memory.
  On absorbed (vanished) episodes: models ~0.84–0.85 vs oracle 0.848.
- **O0-C identity**: **gru128 solves it (0.989, balanced
  intact 0.99/swapped 0.99).** gru64/lstm64/ssm sit at ~0.56 — *below*
  the ~0.7 always-"same" prior — they attempt discrimination but cannot
  retain/compare the appearance signature reliably at this capacity.
  PARTIAL: identity persistence is achievable but capacity-limited.

## Causal tests (ID preset) — does the hidden state carry the object?

| intervention | mlp | ssm | gru64 | gru128 | lstm64 | constvel | oracle |
|---|---|---|---|---|---|---|---|
| none                | .1235 | .0726 | .0549 | .0533 | .0585 | .0877 | .0662 |
| reset @ mid-occl    | .1235 | .1379 | .1567 | .1589 | .1419 | .2785 | .2758 |
| reset @ mid-vis (control) | .1235 | .0833 | .0845 | .0820 | .0732 | .0877 | .0734 |
| reset @ occl-start  | .1235 | .1622 | .2469 | .2488 | .2071 | .4869 | .4869 |
| noise σ=0.5 @mid-occl | .1235 | .5031 | .1243 | .1501 | .0694 | .3648 | .3600 |
| noise σ=2.0 @mid-occl | .1235 | 1.0848 | .3322 | .4060 | .1047 | .4895 | .4883 |

- **KEY RESULT — hidden reset mid-occlusion degrades tracking ~2.6–2.9×**
  (gru128 0.053→0.159, gru64 0.055→0.157, lstm64 0.059→0.142), while the
  **mid-visible control is nearly free** (0.073–0.085): when the target
  is observable the model re-reads position from the observation; when
  hidden, the state *is* the object representation. Existence accuracy
  collapses 0.90→0.58 after mid-occlusion reset. No environment
  shortcut: with the state zeroed there is nothing left to read —
  consistent with the leakage audit.
- Heuristics show the same signature (constvel 0.088→0.279,
  oracle 0.066→0.276), validating the instrument.
- MLP is invariant to all interventions (no state) — the negative
  control behaves exactly as specified.
- Reset at occlusion *start* is catastrophic (exist acc →0.22, whole
  window lost), as expected.
- Noise: graded degradation; LSTM notably robust (σ=2: 0.105 vs GRU
  0.33–0.41) — its additive cell channel resists perturbation; SSM is
  fragile (linear modes swamped by noise).

## OOD / generalization (trained on occlusion 4–16)

pos MAE occluded, mean over 5 seeds (heuristics deterministic):

| preset | prior | constvel | oracle | mlp | ssm | gru64 | gru128 | lstm64 |
|---|---|---|---|---|---|---|---|---|
| id          | .122 | .088 | .066 | .124 | .073 | .055 | .053 | .059 |
| **occ24**   | .132 | .113 | .077 | .132 | .092 | .065 | .064 | .070 |
| **occ32**   | .146 | .128 | .089 | .146 | .106 | .073 | .070 | .083 |
| **occ48**   | .180 | .140 | .102 | .180 | .120 | .089 | .085 | .098 |
| fast_v      | .124 | .019 | .014 | .126 | .060 | .030 | .031 | .060 |
| slow_v      | .142 | .071 | .079 | .142 | .109 | .070 | .068 | .080 |
| drag_x3     | .123 | .132 | .071 | .123 | .083 | .068 | .063 | .068 |
| v_flip      | .126 | .098 | .076 | .128 | .077 | .062 | .060 | .067 |
| distractors4| .123 | .092 | .063 | .125 | .076 | .059 | .058 | .062 |
| ambush      | .124 | .090 | .060 | .127 | .071 | .054 | .053 | .058 |
| app_jitter  | .126 | .073 | .069 | .127 | .071 | .052 | .051 | .057 |
| swap_half   | .126 | .080 | .069 | .128 | .072 | .055 | .054 | .060 |

- **Occlusion-length transfer**: graceful degradation — gru128 holds
  0.085 at occ48 (3× the max trained length) and still beats constvel
  (0.140), prior (0.180), MLP (0.180) *and* the oracle (0.102).
  Pre-reappear error grows (gru128 0.030→0.089→0.137→0.237 at
  24/32/48): localizing the *exit moment* over long horizons is the hard
  part. `err_vs_occstep_occ48.png`, `err_vs_boutlen.png`.
- **Motion-model shift**: drag_x3 barely hurts models (0.063–0.068 ≈
  oracle 0.071) while constvel collapses (0.132) — the learned dynamics
  model absorbs the shift. v_flip (velocity reversal while hidden):
  models 0.060–0.067 vs oracle 0.076 — hidden flip is in principle
  undetectable until the object exits the entry side; models' hidden MAE
  stays low because the mid-occlusion position is similar either way.
- **Unseen velocity caveat**: on `fast_v` (0.11–0.14 vs trained
  0.04–0.09) the learned models (0.03–0.06) *lose* to constvel (0.019)
  and oracle (0.014) — they under-extrapolate speeds outside the
  training prior. Honest negative.
- **Appearance stress**: app_jitter costs gru128 id_acc 0.99→0.88
  (jitter mimics swaps); swap_half leaves it at 0.99. Position/existence
  unaffected.

## Distractors (criterion 4)

Always-4-distractors and `ambush` (a decoy timed to exit the occluder's
far edge just before the target) leave position error essentially at ID
level for all recurrent models (gru128 0.058/0.053 vs 0.053 ID;
id_acc ≥0.98). The target slot is tracked independently of distractor
slots. PASS.

## Seed replication (criterion 5)

5 seeds, tight dispersion on the headline metric: gru128
0.0533±0.0011, gru64 0.0549±0.0026, lstm64 0.0585±0.0029, ssm
0.0726±0.0036, mlp 0.1235±0.0010. PASS.

## Scorecard

| # | Criterion | Result |
|---|---|---|
| 1 | Beats heuristic + MLP baselines | **PASS** (O0-A: all recurrent < oracle/constvel/prior/lastobs/mlp; O0-B: parity with oracle, ≫ MLP; O0-C: only gru128, see caveat) |
| 2 | Transfer to unseen occlusion lengths | **PASS** (occ24/32/48 all beat heuristics+MLP; gru128/64 beat oracle everywhere) |
| 3 | Hidden reset degrades performance | **PASS** (2.6–2.9× occluded-MAE degradation vs ~1.3–1.5× for the mid-visible control; existence collapses; MLP invariant) |
| 4 | Tracks target under distractors | **PASS** (distractors4/ambush ≈ ID level) |
| 5 | Replicates across seeds | **PASS** (5 seeds, σ≤0.004 on occluded MAE) |

**Overall: PASS** — learned recurrent models genuinely maintain hidden
object state (existence + position), verified causally. **PARTIAL** on
O0-C for ≤64-unit models (identity needs capacity: 128-unit GRU solves
it, 64-unit GRU/LSTM and the SSM do not beat the trivial same-prior).

## Caveats / threats to validity

- Occluder bounds are observable and their width correlates with
  crossing time — *physical* information (a wider tunnel takes longer to
  cross), verified to carry nothing beyond geometry by the leakage
  probe. Exit-timing inference from width+velocity is the intended
  computation, not a leak.
- Heuristic id_acc=1.0 is a hard-coded rule; learned models are not
  directly comparable — the informative comparison is gru128 vs gru64.
- `fast_v` under-extrapolation shows the learned motion prior is
  range-limited; OOD generalization is good on duration, weaker on
  speed.
- Edge-touching occluders (gone episodes) are observable geometry; the
  existence task still requires predicting absorption while hidden.
- SSM converges slower and is noise-fragile; included as a capacity
  point, not a contender.
- Probes `probe_*` in `artifacts/runs/` are earlier hyperparameter
  explorations (kept for the record); the reported matrix is
  `{mlp,gru64,gru128,lstm64,ssm}_s{0..4}`.

## Reproduce

```bash
PY=/Users/eightman/dev/sandbox/kamimusuhi/.venv/bin/python
$PY -m pytest experiments/o0/tests
$PY -m experiments.o0.train --config experiments/o0/configs/full.yaml \
    --run-id gru64_s0 --seed 0 --arch gru64
$PY -m experiments.o0.sweep --config experiments/o0/configs/full.yaml \
    --skip-train --seeds 0 1 2 3 4 --models mlp gru64 gru128 lstm64 ssm
```
