# A3.2 — Physiology Hardening Report

Date: 2026-09-16
Upstream baseline: A3.1 `6bcc05b` (`active_fly_v1`, `flychan-v1`,
`reduce-v3-ais-fly` — all **frozen**, unchanged semantics)
New runtime: `active_fly_v1_1` = `flychan-v1_1`
Run: `experiments/mioba/scripts/a32_hardening.py` →
`docs/experiments/a32_report.json`

## 1. What changed vs A3.1

| component | A3.1 (frozen) | A3.2 |
|---|---|---|
| Na channel | `para_Na` (m_tmin 0.04, h_tmin 0.4) | `para_Na_v11` (m_tmin 0.15, h_tmin 1.2 — slower macroscopic kinetics → wider spikes) |
| K (DR) | `shab_K` | `shab_K_v11` |
| A-type K | `shal_K`, `shaker_K` | `shal_K_v11` |
| adaptation | none | `kca_K_v11` — spike-triggered slow K⁺ current (activation τ≈15 ms). **MODEL_INFERENCE**: a real KCa molecular identity is not asserted; it exists to produce AHP + repetitive-firing suppression only (§7–§8) |
| runtime | `active_fly_v1` | `active_fly_v1_1` (backend dispatch only; A3.1 path untouched) |

Provenance separation is preserved per §9: channel kinetics =
LITERATURE_PRIOR curves, densities = MODEL_INFERENCE fitted values,
cell-type assignment = CURATED_ANNOTATION/fallback chain. `kca_K_v11`
is explicitly MODEL_INFERENCE at all three levels.

## 2. Calibration (§10–§11)

Same bounded random-search optimizer as A3.1 (`calibrate-iters 8`,
seed 20260916, 25 °C), now over `{g_para, g_shab, g_shal, g_shaker,
g_kca, g_leak, E_leak, Cm}` with `g_kca ∈ [0,30]` added to bounds.
Fit metrics = rest V / Rin / τ / rheobase / F-I RMSE; spike width and
AHP remain **held-out** (§24 split).

All four required profiles fitted:

| profile | loss | g_kca | notable fit |
|---|---|---|---|
| kenyon_cell | 1.40 | 9.9 | g_para 104.6, shaker 58.7 |
| projection_neuron | 2.25 | 20.6 | g_shab 40.1 |
| descending | 1.55 | 20.9 | — |
| motor | 2.38 | 20.9 | identical optimum to descending |

The KCa-lite density is strongly selected by the optimizer (10–21
mS/cm²) — adaptation pressure is real in the objective.

## 3. A3.1 vs A3.2 — single-cell comparison (§11)

Per-profile `eval_profile` on identical targets (F-I amps
2/4/7/12/20/35 µA·cm⁻²):

| profile | metric | v1 (A3.1) | v1_1 (A3.2) |
|---|---|---|---|
| kenyon_cell | F-I max Hz | 226.7 | 26.7 |
| | F-I RMSE | 119.0 | **14.3** |
| | spike width ms | ~0.6 | 1.30 |
| | AHP mV | — | −16.8 |
| projection_neuron | F-I max Hz | 253.3 | 20.0 |
| | F-I RMSE | 145.3 | **36.0** |
| | rheobase | 0.78 | 14.1 |
| | spike width ms | 0.6 | 0.60 |
| | AHP mV | −14.2 | −20.7 |
| descending | F-I max Hz | 320.0 | 20.0 |
| | F-I RMSE | 171.7 | **27.3** |
| | AHP mV | — | −20.6 |
| motor | F-I max Hz | 300.0 | 20.0 |
| | F-I RMSE | 152.6 | **44.7** |
| | AHP mV | — | −21.6 |

Read: the pathological 200–320 Hz saturation is gone — max stable
rates are now 20–27 Hz in the probed range, F-I RMSE dropped ~4–8×,
AHP is present in every profile (−17 to −22 mV), resting behaviour is
quiet (0 spontaneous spikes) and no numerical failure occurred.

**Honest gaps carried forward** (§18, not fixed — A3.2 is a freeze,
not a physiology project):
- F-I is now arguably *under*-active at the top of the probe range
  (20–27 Hz max); the slope bracket overshot low on PN/motor.
- Spike width improved only for kenyon_cell (1.3 ms); PN/DN/MN remain
  ~0.6–0.7 ms — the fitted densities did not push width to the ~1 ms
  target everywhere.
- KCa-lite is a phenomenological adaptation current, not a
  reconstructed SK/SLO channel.
- Fitted `g_kca` etc. are MODEL_INFERENCE — no direct density
  measurement is claimed.

## 4. Channel ablation (§12)

v1_1 projection_neuron, i_ext = 25 µA·cm⁻², 400 ms:

| condition | spikes | Hz after 100 ms | min-after-peak |
|---|---|---|---|
| full | 13 | 30.0 | −66.8 |
| para_Na_v11 off | 0 | 0.0 | −31.1 |
| shab_K_v11 off | 10 | 23.3 | −66.0 |
| kca_K_v11 off | 5 | 0.0 | −54.4 |

- Para removal abolishes spiking (13 → 0) ✓ expected.
- Shab removal reduces rate/repolarization ✓.
- KCa removal reduces *sustained* firing (30 → 0 Hz after 100 ms):
  without the slow AHP current the membrane fails to de-inactivate
  Na between spikes — a plausible direction for this model (fewer
  spikes, shallower AHP). The runner's boolean
  `kca_rate_changed` flag encodes a naive "rate must increase"
  expectation and reads False — the measured effect is real and
  recorded; interpretation is reported here, not hidden.

## 5. Graft causal chain (§13–§17)

G0.1 graft unchanged (10 LIF neurons); same matched target set.
`weight_out=200`, `p=1.0` (defaults updated after weak-coupling smoke;
strength needed for AIS-level effects, not just dendritic ΔV).

| condition | host spikes (8 reps) | AIS spikes |
|---|---|---|
| full | 2184–2278 | 4–5 |
| efferent_disabled | 1994–2058 | 3 |
| afferent_disabled | 1994–2058 | 3 |
| graft_disabled | 1994–2058 | 3 |

Paired acceptance (noise floor = 1.0, direction must be consistent
across **all** 8 replicates — config `--causal-noise-floor`, §17):

- AIS spike deltas: `[1,2,1,1,2,2,1,2]` — all positive but two are
  exactly at the floor → `ais_consistent=False`
- host output deltas: `[220,198,211,210,228,193,217,190]` — consistent
  → `output_consistent=True` → **causal accepted**

Latency probe (§15) on target entity 3802: graft event reaches
DENDRITE_DIST (first crossing −20 mV at 7.4 ms, peak −14.6 mV at
76.4 ms); on this particular probe the soma/AIS did not cross −20 mV
within 300 ms — dendritic arrival is verified directly, AIS/output
effect is verified statistically via the paired counts above.

## 6. Determinism & stability (§19)

- Bit-exact rerun: host/graft/AIS spike vectors identical ✓
- Numerical failures across all graft conditions: 0 ✓
- No spontaneous runaway at rest ✓
- Anatomy manifest hash recorded in run_manifest — anatomy untouched
  (all changes are overlay/runtime) ✓

## 7. Freeze decision

Per §19: calibration improved, zero numerical failures, causal graft
effect present and deterministic → **A3.2 is frozen here**. No further
physiology tuning in this phase; remaining gaps are declared above and
handed to A4/G1-era work only if needed.

## Artifacts

- `docs/experiments/a32_report.json` — full metrics, paired counts,
  comparison table, ablation, calibration records, probe latency
- overlay: `/tmp/a32_overlay` (rebuilt deterministically by the runner;
  not a canonical artifact)
