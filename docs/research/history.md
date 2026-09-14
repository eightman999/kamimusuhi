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
