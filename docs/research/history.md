# Kamimusuhi / MIO — experiment and PR history

Status: **research record (non-normative)**
Baseline: `origin/master` @ `a79cd64`, branch `exp/cx0-synthetic-cortex`
Compiled: 2026-09-14, for the Mio Synthetic Cortex (CX0) integration phase.

This file consolidates the experiment series and merged PRs that the Synthetic
Cortex plan builds on. It exists so that the cortex design does not re-derive
verdicts from memory. Sources of truth remain the per-experiment reports and
the git history; this document is a navigable summary, not new evidence.

## 1. Research phases

```text
Phase A   Persistent AI / Persona architecture
              (architecture.md, spec.md, crates/)

Phase B   K0 non-linguistic Core experiments
              (K0-E, K0-E2 — branches exp/k0-*, pre-PR-era)

Phase C   Decomposed cognitive-organ experiments
              H0 homeostasis · R0 memory gate · S0 self/world
              T0 temporal sense · O0 object permanence
              P0 predictive surprise · G0 emergent grounding

Phase D   MIOBA — artificial-organism substrate
              (M-series infra, substrate abstraction, reflex0)

Phase E   validated cognition → runtime organ (PR #39, OPEN)

Phase F   Mio Synthetic Cortex ← this line of work
```

Phase F is an **integration phase**, not a new research series: it tests
whether individually validated organs can be composed by a recurrent
cortical-like layer into longer-timescale, context-dependent behavior.

## 2. Pre-history — K0 non-linguistic Core

| Line | Question | Result |
|---|---|---|
| K0-E | Does a recurrent core (MLP/GRU, 16-dim obs, 6 actions, 48-step episodes) acquire stable cognition via imitation / PPO? | Imitation favored GRU; **PPO degraded performance** — recurrence alone does not yield stable cognition. |
| K0-E2 | Strict-memory + language-return closed loop | GRU64/128 used returned information (`correct−shuffled = 0.4889`); strict memory delay=47 PASS for GRU128; **transfer to unseen delays FAIL**; PPO B1−B0 FAIL; macro success 1.000, action macro-F1 0.873 (GRU128). |

Takeaway recorded at the time: a GRU128-class recurrent core holds state and
exploits returned information, but **temporal generalization and RL-stable
policy improvement are separate problems** — motivating the T0 line.

Evidence: branches `exp/k0-e2-active-info`, `exp/k0-f-machine-interoception`,
`exp/k0-f2-confirmatory`, `exp/k0-parallel-brainstem` (pre-PR workflow; no
merged PR numbers).

## 3. Cognitive-organ experiments (Phase C)

Canonical verdict table. "Evidence" is the human-readable report; machine
artifacts live next to it in the same directory.

| Exp | Title | Verdict | Key measured facts | Evidence |
|---|---|---|---|---|
| **H0** | Artificial Homeostasis | **PASS (5/5)** | GRU64/128 regulate internal vars; energy/fatigue/novelty used causally; certainty/temperature largely ignored; beats heuristic on part of OOD, loses on resource OOD | `experiments/h0/reports/H0_RESULTS.md` |
| **R0** | Learned Memory Gate | **PASS (strong)** | mlp .999 / gru128 .988 clean; erase→.126–.139 (chance); permute is a no-op (content-based readout); delay128 ≈ .99; slots2 .697 vs oracle .760 | `experiments/r0/reports/R0_RESULTS.md` |
| **S0** | Self/World Separation | **PASS (with caveats)** | action-aware self-dim MSE ≈.004 vs blind ≈.026; action shuffle → 9–12× error; counterfactual cos ≈.99; attribution AUC .90–.97; **breaks at action delay=2**; no cross-world transfer | `experiments/s0/reports/S0_RESULTS.md` |
| **T0** | Temporal Sense v2 | **PASS (with strong-gate failures)** | gru64 seen .946 / held-out interp .953 / band .962; elapsed-time probe R²≈.993; hidden reset → collapse (recurrence causally required); **extrapolation .260 FAIL; temporal scaling FAIL** | `experiments/t0/reports/T0_RESULTS.md`, `T0_V1_INVALIDATION.md` |
| **O0** | Object Permanence | **PARTIAL** | hidden state decodes true hidden position R²≈.9–1.0; reset → 5–11× degradation; **but `corridordyn` heuristic dominates overall**; death/exit detection not learned | `experiments/o0/reports/O0_RESULTS.md` |
| **P0** | Predictive Surprise | **FAIL (negative result, kept)** | no surprise-gated or belief-reactive attention; policy fixes on OBSERVE_D; ablations inert; env admits belief-reactive frontier (`vtrigger` −0.56 vs learned −11.87) → failure is the learner's | `experiments/p0/reports/P0_RESULTS.md` |
| **G0** | Emergent Grounding | **FAIL (negative result, kept)** | no reusable context-invariant compositional structure in 11 representation families × 5 seeds; untrained reservoir parity explains above-null performance | `experiments/g0/reports/G0_RESULTS.md` |

### In-flight / revision work (not merged to `master` at baseline)

| Line | Branch / location | Status |
|---|---|---|
| G0-v4 predictive-invariant grounding | `exp/g0-v4-predictive-invariant`; untracked `experiments/g0_v4/` | pilot + bounded grid + stage2 (5 seeds) — verdict **FAIL** (commit `c44ac29`) |
| G0-v5 grounding, obs-only SSL | untracked `experiments/g0_v5/` on `exp/u0-need-guided-memory` | pilot scope only; full 5-seed not started |
| G0-v6 CPC replication | `experiments/g0_v6/` on `exp/u0-need-guided-memory` | revision 2 (`G0-v6.1`) locked; running |
| **U0** need-guided memory (H0×R0 coupling) | `experiments/u0/` on `exp/u0-need-guided-memory` | protocol sanity **PASS** (oracle .920 vs no-memory .172); PPO arm + scaffold arm under comparison; **no final verdict yet** |
| X0 cross-modal binding | `exp/x0-cross-modal-binding` | not merged |
| C0 continuity / FI0–FI3 persona-runtime lines | `exp/c0-continuity`, `exp/fi0-*` | parallel line, unrelated to CX0 naming |

**Naming note:** `C0` already denotes the continuity experiment line
(`exp/c0-continuity`). The cortex series therefore uses **`CX0`** as its
experiment identifier; inside the series the plan's architecture arms keep
the labels C0–C3 exactly as pre-registered.

## 4. MIOBA — substrate work (Phase D)

| PR | Content | Status |
|---|---|---|
| #21 | M-series infrastructure: coordinator, heterogeneous GPU workers, genome, lineage SQLite, checkpoint/resume, replay, FBA backend, mutation, Observatory, MIE telemetry | MERGED |
| #26 | Real-GPU validation follow-up (RTX 3060 + Tesla P100): worker lifecycle, replay preservation, GPU identity, Observatory aggregation. Principle: **hardware speed ≠ scientific condition** | MERGED |
| #36 | M2 substrate abstraction: `SubstrateProtocol`, `SubstrateGene`, typed endpoints, organ IR, deterministic development, mutation staging | MERGED |
| #35 | M3 `reflex0`: independent 48-neuron sensor→integrator→motor substrate through the generic genome→develop→evaluate→lesion→mutate→reproduce→replay pipeline | MERGED |

Consequence for CX0: the cortex is **not** special-cased into FBA. It is
designed so a `cortex0` substrate/organ can later pass through the same
generic pipeline (genome → develop → evaluate → lesion → mutate → reproduce
→ replay).

Docs: `docs/experiments/mioba-architecture.md`, `mioba-schema.md`,
`m1a-substrate-validation-2026-09-11.md`, `mioba-gpu-validation-2026-09-11.md`.

## 5. Promotion boundary (Phase E, PR #39 — OPEN)

Branch `feat/integrate-experimental-organs` adds the admission contract
(`kamimusuhi-core::organs`, `kamimusuhi-runtime::ProcessOrgan`) and the
boundary document `docs/implementation/experimental-organ-integration.md`.

Manifest adopted there — **and adopted by reference as the canonical
admission policy for CX0**:

| Experiment | Role | Verdict | Promotion |
|---|---|---|---|
| H0 | Regulation | PASS | **Active** |
| R0 | MemoryGate | PASS (strong) | **Active** |
| S0 | AgencyAttribution | PASS w/ caveats | **Active** |
| T0 | TemporalState | PASS w/ strong-gate failures | **Active** |
| O0 | ObjectState | PARTIAL | **Shadow only** |
| G0 | — | FAIL | **Excluded** |
| P0 | — | FAIL | **Excluded** |

Binding rules carried into CX0:

- organ signals are **derived transient cognition** — not canonical evidence,
  durable memory, canonical self-state, or mutation authority;
- `Active` requires a recorded PASS; O0 stays shadow; P0/G0 are not wired in;
- the cortex itself is under the same boundary — its output is an organ-style
  signal, never a direct write to canonical state;
- U0 is **not** in the manifest: no recorded verdict exists yet at this
  baseline, so it cannot be admitted (it may inform future CTX task design
  once it reaches `result`).

## 6. What the accumulated evidence supports

```text
                    Status
Homeostasis          PASS      → usable drive/interoceptive organ
Memory admission     PASS      → usable memory gate
Self/world           PASS      → usable agency organ (delay caveat)
Temporal state       PASS      → usable internal-time organ (interpolation only)
Object permanence    PARTIAL   → shadow input only
Surprise attention   FAIL      → not a validated signal
Grounding            FAIL      → latent units get no semantic labels
```

The missing piece, and the CX0 hypothesis: **a structure that integrates
these organ states into one longer-timescale context** — not another
general-purpose NN.

## 7. PR ledger (research-structural)

| PR | Title | Status |
|---|---|---|
| #20 | chore: structure Kamimusuhi as an open research repository (RESEARCH.md, papers/, docs/experiments/, CITATION.cff, licensing/provenance boundary) | MERGED |
| #25 | MIOBA Observatory browser smoke skill | MERGED |
| #31 | K-Edge Android/ARM64 profile docs | MERGED |
| #39 | runtime: integrate validated experiments as cognitive organs | **OPEN** |

Experiment PRs #27–#34 are listed in §3. MIOBA PRs in §4.

## 8. CX0 Synthetic Cortex — first matrix (this branch)

The CTX-1..5 world + organ bundle (H0/R0/S0/T0 live; O0 reserved shadow)
+ C0/C1/C2/C3 arms were trained (BC→PPO) and evaluated with the full
intervention battery: 5 tasks × 4 arms × 5 seeds × 13 conditions = 1300
rows in `experiments/cx0/runs/**/results.jsonl`; report:
`experiments/cx0/reports/CX0_RESULTS.md`.

**Verdict: PARTIAL.**

| gate | result |
|---|---|
| C-G1 solvability | PASS — best arm mean clean 0.382; ctx3 0.97–1.0 |
| C-G2 cortex edge | FAIL — generic GRU (c1) is the best arm; no structured-cortex advantage in v1 |
| C-G3 organ causal | FAIL — mean shuffle drop 0.064; organ fields largely bypassed on ctx1/ctx4 (hidden state holds site maps), though t0/r0 shuffles halve ctx3 (0.97→0.35/0.34) |
| C-G4 context state | PASS — reset_hidden drops (ctx5 c3: 0.62→0.07; ctx3 c3: 1.0→0.51) |
| C-G5 no harm | MIXED — ctx5: c1 0.346 < c0 0.458 (cortex hurt), c3 0.621 ≥ c0 |
| C-G6 decode | PASS — context probe 0.627 from recurrent populations |
| null check | OK — cortex_off → ~0 across arms (integration load-bearing) |

Key observations:

- The memory organs are *available* but often *bypassed*: recurrent cores
  learn the site→function map in-context, so `erase_memory`/`shuffle_r0`
  only hurt where the task truly needs the slot readout (ctx3).
- CTX-2 (phantom filtering) is unsolved by all arms (~0.05) — the S0
  attribution signal discriminates (p_self 0.10 vs p_world 0.70 in
  pretraining checks) but the confirm-then-store policy did not emerge
  under this budget.
- C3 (sparse multi-τ) wins on ctx5 (drive-conflict) and is competitive on
  ctx3, but underperforms c1 on ctx1/ctx4 — sparsity cost exceeds benefit
  at this scale.

Gate outcome per plan: **no C0–C3 winner identified** → noise/development
and MIOBA substrate integration remain blocked (step 14-15 not entered).

## 9. CX0 second matrix (v2/v3, 10 seeds) — organ-causal, still PARTIAL

After a→b→c fixes (memory-proof success, phantom persistence+density,
C3 rewire), the matrix was rerun: 5 tasks × 4 arms × 10 seeds × 13
conditions = 2600 rows in `experiments/cx0/runs_v2/**/results.jsonl`;
report `experiments/cx0/reports/CX0_RESULTS.md`.

**Verdict: PARTIAL** (C-G1/G3/G4/G5/G6 pass; C-G2 fails).

### Clean success, mean±sem (10 seeds)

| task | c0 (flat) | c1 (GRU) | c2 (layered) | c3 (sparse) |
|---|---|---|---|---|
| ctx1 | 0.654±.035 | 0.708±.027 | 0.631±.034 | 0.023±.014 |
| ctx2 | 0.812±.014 | 0.898±.023 | 0.906±.019 | 0.000±.000 |
| ctx3 | 0.856±.046 | 0.919±.077 | 0.908±.054 | 0.829±.086 |
| ctx4 | 0.752±.025 | 0.756±.023 | 0.592±.038 | 0.035±.016 |
| ctx5 | 0.510±.052 | 0.490±.046 | 0.529±.036 | 0.519±.062 |

Paired seed diffs: c2−c1 = −0.165±.039 (ctx4), −0.077±.040 (ctx1),
≈0 elsewhere → **structured cortex does NOT beat the parameter-matched
generic GRU at this scale** (C-G2 FAIL). c0−c1 = −0.085±.031 on ctx2
only (waiting for the go window is where recurrence pays). c3 collapses
on the memory-navigation tasks and survives only on short-horizon tasks
(ctx3 0.83, ctx5 0.52) — see findings.

### Organ usage map (mean success drop under field shuffle, 10 seeds)

| task | sensory | h0 | s0 | t0 | r0 |
|---|---|---|---|---|---|
| ctx1 | .03 | .01 | −.02 | .00 | **.37** |
| ctx2 | **.57** | −.01 | .00 | .01 | **.54** |
| ctx3 | .01 | −.01 | .00 | **.60** | **.61** |
| ctx4 | .09 | .00 | .03 | .05 | **.45** |
| ctx5 | .00 | −.01 | −.01 | .00 | .00 |

- **R0 is now load-bearing** on ctx1/2/3/4 (C-G3 PASS: pooled drop 0.165;
  `erase_memory` ≈ shuffle_r0). Achieved by requiring the last env-visible
  recall to carry the needed etype (`_memory_proof`) + an R0 direction
  pointer + navigation shaping.
- **T0 is load-bearing on ctx2/3/4** (lesion drops .4–.75), not just its
  own task — donor-shuffle under-reports t0 use where cue/due schedules
  are stereotyped.
- **Field lesions vs shuffles**: donor-shuffle is a near-no-op for
  stereotyped signals (h0 internals, partially t0/s0). Zeroing the field
  (`lesion_{h0,s0,t0,r0}`) reveals the true dependence — see the
  field-lesion table in the report: **every organ field is load-bearing
  somewhere**. h0 lesions cost .3–.8 on ctx1–4; s0 lesions cost .2–.8
  (strongest for c0 and c3-on-ctx3). C-G3 is thus supported across all
  organs, not just r0.
- `cortex_off` → ~0 on all tasks (drop .44–.85): the integration layer is
  causally required (null check OK).
- `reset_hidden`: ctx2 +.42, ctx3 +.23 — context state is causal where
  waiting/timing is required; c0 flat is unaffected (reactive policy).
- `reset_organs`: ctx4 +.34 (drift state), ctx3 +.29.
- `lesion_context`: ctx3 +.39, others +.15–.18 — context pop specialized.
- `lesion_feedback`: ctx3 +.23, ctx5 +.11. `lesion_slow` ≈ 0 — the slow
  population contributes nothing measurable at 80-step horizons.

### Findings

1. **Organs get used only when the task makes them load-bearing.**
   Memory became causal exactly when success required a real retrieval
   (proof), not before. Timing is causal on all timed tasks. Under field
   lesions every organ (h0, s0, t0, r0, sensory) is load-bearing
   somewhere — the organ bundle is genuinely integrative.
2. **Generic GRU is sufficient at this scale.** C2's layered
   decomposition loses where it hurts (ctx4 −0.17) and never wins
   significantly. The cortex hypothesis is NOT confirmed — answer: no
   cortex advantage in this regime.
3. **C3's sparse multi-τ leaky pops cannot sustain multi-step motor
   programs.** It fits BC (CE≈1.0) but collapses at execution on
   navigation chains; leaky Jacobian ~0.9^step decays credit over long
   horizons vs gated recurrence. Four rescue attempts (connectivity,
   dense assoc, fast-path predict, head access to assoc) all fail ≤0.13.
   It survives on short-horizon tasks (RESP timing, probe answers) and
   routes attribution on ctx3 (lesion_s0 +0.78 — the strongest single
   organ dependence of any arm).
4. **c0 flat relies most on organ affordances** (lesion drops ≈ clean on
   ctx1–4); c1/c2 partly compensate via hidden state. Recurrence's edge
   over flat shows only where context must be maintained (ctx2, ctx3).
5. **ctx5 drive-conflict is near-chance for all arms (~0.5)** and
   insensitive to any single field lesion — the probe metric is noisy;
   not currently discriminative.

### Verdict & gate

PARTIAL. No C0–C3 winner → per plan, **MIOBA substrate/genome
integration remains gated out**; noise/development work stays blocked
until a cortex variant shows an advantage or the architecture question
is closed at this scale.

### Next candidates (unscheduled)

- s0: make last-write-wins lose (e.g., phantom stores also corrupt
  matching payload slots, or require attribution-gated store) so the
  gate becomes load-bearing for GRU arms too.
- c3: gated sparse populations or GRU-per-population hybrid.
- ctx5: denser probes or continuous agreement metric for resolution.
- Caveat on c3 rows: seeds 0–4 were trained under the pre-rewire
  predict input (context+assoc); seeds 5–9 under assoc+feedback.
  Conclusions hold under both, but exact c3 numbers mix wirings.
- Method note: field lesions can include OOD-input effects; the
  consistency across arms/tasks and agreement with shuffle where signals
  vary (r0) supports interpreting them as functional dependence.

## 10. CX0 v4 — recall-dithering fix: tested and rejected

The first queued candidate was run inside the same window: a 0.02 cost on
RECALL actions that re-fetch an identical already-held payload
(`RECALL_REDUNDANT_COST`, `ctx_world.py`; 10-seed full matrix,
`experiments/cx0/runs_v3/`, 3,400 rows).

| cell | v3 | v4 | paired Δ |
|---|---|---|---|
| ctx4/c2 | 0.592 | 0.690 | **+0.098 ± .052** — dithering reduced |
| ctx3/c2 | 0.908 | 0.794 | **−0.114** — pre-window recall suppressed |
| ctx1/c2 | 0.631 | 0.583 | −0.048 ± .057 |
| c2 mean | 0.713 | 0.702 | −0.011 |

Verdict: **rejected** — mechanism confirmed (dithering down, ctx4 +0.098)
but net-negative (ctx3 −0.114, ctx2 −0.03…−0.08 across arms). C-G2 still
fails. `RECALL_REDUNDANT_COST` is held at 0.0 — mechanism documented
inert; shipped code ≡ v3 semantics, so `CX0_RESULTS.md` still reports the
runs_v2 dataset. runs_v3/ is kept as the rejection evidence.

## 11. Post-review kernel hardening

Review after the overnight window found a consistency bug in the CTX
kernel: `_perceived_event()` rolled fresh RNG and decremented the
phantom TTL on *every call*, while being invoked by `step()`, `_obs()`,
recall queries, cause labels, and the oracle. The event the agent saw
in obs therefore differed from what `step()` stored/acted on in ~23% of
steps, STORE could write payloads for unperceived events (~11% of
stores), and the "2-step" phantom persistence collapsed to ~1 obs frame.

Fixed: perception is rolled once per step and cached (t-keyed), so all
consumers see one event stream; STORE on empty perception is a no-op
instead of burning a zero-key slot; probe labels remap −1 ("no
context") to its own class instead of clipping into class 0;
`run_eval` accepts a shared OrganSet instead of reloading checkpoints
per cell. Verified: obs↔step mismatches 0/395 (was ~23%), suite 24 pass
+ 1 skip, matrix smoke OK.

Data caveat: `runs/`, `runs_v2/`, `runs_v3/` were generated under the
pre-fix kernel. Qualitative conclusions are unlikely to move (phantoms
are noise by design and the fix mainly removes bookkeeping noise), but
exact numbers should be regenerated before being cited.
