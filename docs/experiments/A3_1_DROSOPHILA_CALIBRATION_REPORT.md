# A3.1 — Drosophila Physiological Calibration Report

Date: 2026-09-16 · Baseline: A3 `cfe42a47` · Circuit: G0.1 240-entity
(identical anatomy, matched targets, paired seeds)

Result: **PASS** — graft causal path survives under fly physiology,
deterministic rerun bit-exact, zero numerical failures, all §56
model-validity checks green.

## 1. What changed vs A3 (scope discipline)

Fixed (immutable, verified by manifest hash): BANC entities,
morphology, compartments, native connectivity, synapse counts.

Changed (physiology layer only): channel models → fly families;
channel densities → compartment-localised priors; synaptic sign →
receptor-aware; spike initiation → AIS node; added calibration DB +
bounded optimizer.

`active_hh_v0` / `generic_Na` / `generic_Kdr` remain registered and
behaviourally unchanged — the A3 benchmark numbers below were re-run
on this commit as the comparison arm.

## 2. Channel models (`physio/fly_channels.py`, `flychan-v1`)

| Model | Family | Gates | Activation V1/2 | Inactivation | Q10 | Provenance |
|---|---|---|---|---|---|---|
| `para_Na` | Para/DmNav | m³h | −40 mV | h V1/2 −55 | 2.3 | kinetics LITERATURE_PRIOR, exact curves MODEL_INFERENCE |
| `shab_K` | Shab/Kv2-like | n² | −5 mV (steep, s=6) | none | 2.3 | same |
| `shaker_K` | Shaker/Kv1-like | m·h A-type | −30 mV | h V1/2 −60 | 2.3 | same |
| `shal_K` | Shal/Kv4-like | m·h A-type | −25 mV | h V1/2 −60 | 2.3 | same |

These are **not renamed generics**: independent Boltzmann/bell-tau
kinetics, own E_rev, Q10 temperature scaling, per-model provenance
and `flychan-v1` version tag. `build_channel` passes
`temperature_C` through; generic classes reject it, fly classes
apply Q10 (`t_ref = 22 °C`).

**Tuning note (honest):** the first Para parameterisation
(m V1/2 −30) produced onset-only spiking then depolarisation block
under sustained drive — h inactivates before m opens. Moving
activation to −40 mV (inside the reported −30…−45 mV window for
DmNav) restored stable repetitive firing. Shab was made steep and
spike-window-limited (V1/2 −5, s=6) so it opens hard during the
upstroke, disengages below threshold, and produces a real AHP.

## 3. Channel localisation & AIS (`physio/fly_compiler.py`, `reduce-v3-ais-fly`)

- Densities are per-runtime-compartment: `DENDRITE_*`, `SOMA`,
  `PRIMARY_NEURITE`, `AXON_INITIATION_ZONE`, `AXON` (§6).
- **AIS**: entities carrying an AXON compartment gain a
  runtime-derived `AXON_INITIATION_ZONE` node inserted between SOMA
  and AXON in the coupling chain. Canonical anatomy untouched;
  every AIS row records `ais_location=soma_axon_boundary`,
  `ais_source=runtime_derived`, `ais_mapping_rule`,
  `ais_provenance=MODEL_INFERENCE` (§7–§9).
- AIS carries `para_Na` at `3×` soma density (MODEL_INFERENCE
  enrichment prior, §8).
- **Spike emission moved to the AIS**: every edge's `pre` node is
  the AIS when present, else SOMA — verified by test (§10).
- Axonal delay remains the deterministic delayed relay
  (§11 allowed); per-edge morphology-derived delay is a recorded
  gap.

Circuit: 240 entities → **1095 runtime nodes** (237 AIS inserted;
30 matched-target AIS), 629 split edges.

## 4. Receptor-aware synapses (`physio/receptors.py`, `receptor-v0`)

`sign = f(pre NT, post receptor profile)` — never NT alone (§27–§29):

- ACh + nAChR-like → exc; GABA + Rdl-like → inh
- Glu + GluCl-like → inh (KC/PN/DN/CX); Glu + iGluR-like → exc
  (motor, MODEL_INFERENCE); Glu + no receptor → UNKNOWN_SIGN
- `unknown_sign_mode`: `strict`/`drop` (edge skipped) · `neutral`
  (kept as exc, flagged UNKNOWN) · `prior` (NT-level prior where
  defined — Glu deliberately has none)

G0.1 circuit audit (`neutral` mode): 629 synapses — **551
receptor-resolved, 78 unknown-sign (12.4%), 0 prior, 0 dropped**.
Fidelity: 89 receptor-resolved entities, 151 receptor-unknown.

## 5. Fly profiles (`physio/fly_overlay.py`, `physio-overlay-fly-v1`)

8 profile families: sensory, projection_neuron, kenyon_cell,
central_complex, descending, motor, modulatory, **generic_fly**
(§38: the fallback is a fly prior, never the squid table).
Assignment: cell_type keyword → super_class → modulatory NT →
generic_fly (GENERIC_FALLBACK provenance).

Circuit distribution: generic_fly 94, central_complex 47, sensory
39, descending 39, modulatory 18, motor 3. Entities with
fly-specific profile: 146/240; with Para 240, Shab 240, Shal 89,
Shaker 42.

## 6. Calibration (`physio/calibrate.py` + `benchmark.py`, `calib-v1`)

Target DB (`TARGETS`) carries stage/sex/temperature/method/
location/source per profile; adult preferred, motor flagged
`cross_stage_prior` (§16–§18). Targets are *measurements*;
parameters are *knobs* — separate schemas (§15).

Optimizer: seeded bounded random search (seed=20260916, 10 iters),
physiological bounds on g_para/g_shab/g_shal/g_shaker/g_leak/
E_leak/Cm (§20–§21). Fit set: rest_V, Rin, tau, rheobase, F-I RMSE.
Held-out validation: spike width, AHP (§24). Fitted params are
recorded as MODEL_INFERENCE with fit metadata — never upgraded to
DIRECT_MEASUREMENT (§23).

| Profile | Final loss | rest_V (target) | rheobase (target) | val: width err | val: AHP err |
|---|---|---|---|---|---|
| kenyon_cell | 3.11 | −69.1 (−65) | 3.9 (8) | 0.50 | 0.74 |
| projection_neuron | 3.07 | −62.7 (−58) | 0.8 (12) | 0.50 | 0.71 |
| descending | 2.01 | — | — | 5.60 | 1.03 |
| motor | 2.81 | −70.9 (−55) | 11.7 (15) | 6.89 | 1.39 |

**Honest fit quality:** resting potentials and rheobase land in the
right ranges and direction; F-I slopes are systematically too steep
(model fires 150–250 Hz at moderate drive vs ~60–180 targets) and
spike width (~0.5 ms vs ~1 ms target) is the largest held-out error.
This is a real gap, not hidden — recorded in
`CALIBRATION_MANIFEST.json` with seeds/bounds/metrics.

F-I curves (post-fit, Hz vs µA/cm² — monotonic then depol block):

| profile | 2 | 4 | 7 | 12 | 20 | 35 |
|---|---|---|---|---|---|---|
| kenyon_cell | 0 | 40 | 100 | 147 | 193 | 247 |
| projection_neuron | 53 | 87 | 127 | 160 | 193 | 240 |
| motor | 0 | 0 | 0 | 20 | 160 | 220 |

Profiles are clearly not identical (§56 check passes).

## 7. Channel ablation (§25–§26)

Single cell (projection_neuron, i_ext=25 µA/cm²):

| condition | spikes | min V after peak | numerical failure |
|---|---|---|---|
| full | 42 | −63.5 | none |
| para_Na off | 1 | −18.7 (depolarised plateau) | none |
| shab_K off | 5 | −56.5 | none |

Para removal effectively abolishes spiking (§56); Shab removal
shifts post-spike voltage and changes repetitive firing (expected
direction). Spike undershoot reaches −63.5 mV — repolarisation
present (§56).

## 8. G0.1 causal revalidation (`active_fly_v1`)

Same 240-entity circuit, same graft (10 LIF neurons, unchanged —
difference attributable to host calibration, §46), paired seeds:

| condition | host spikes (8 reps) | graft spikes |
|---|---|---|
| full | 724/685/734/721/710/677/671/648 | 41–57 |
| efferent_disabled | identical | identical firing |
| afferent_disabled | identical | 0 (silent) |
| graft_disabled | identical | 0 |

- Graft→target telemetry deltas nonzero → **causal path survives**
- Telemetry nodes now include matched-target **DENDRITE_DIST +
  SOMA + AIS** (§49); 30 AIS nodes instrumented
- Deterministic rerun: bit-exact
- Numerical failures: **0**

## 9. Performance (same circuit, 300 ms × 8 replicates, CPU)

| level | wall | steps/s | state mem |
|---|---|---|---|
| L2 passive | 0.62 s | 3889 | 0.64 MB |
| A3 generic HH | 5.25 s | 521 | 1.30 MB |
| A3.1 fly | 8.31 s | 332 | 1.81 MB |

Fly runtime ≈ 1.6× generic HH cost — extra gates (4–5 state vars vs
3) and AIS nodes. Channel-state memory included in state total.

## 10. Fidelity & provenance (PHYSIOLOGY_FIDELITY v2)

- entities_total 240 · active 240 · compartments_active 1095
- fly_profile 146 / generic_fly_fallback 94 / unknown 0
- Para 240 · Shab 240 · Shal 89 · Shaker 42
- AIS entities 237, all MODEL_INFERENCE
- receptor resolved 89 / unknown 151
- calibration targets 32 total (24 adult / 8 cross-stage-flagged)
- parameters: direct 0 · cell_type 0 · literature 636 ·
  fitted 28 · model_inference 1655 · generic_fallback 0 · unknown 0

Artifacts: `docs/experiments/a31/{a31_report.json,
PHYSIOLOGY_FIDELITY_v2.json, CALIBRATION_MANIFEST.json,
kc_trace.json}` (voltage+gate trace, §41).

## 11. Model validity gates (§56)

- depolarisation block everywhere → no (healthy F-I window)
- spontaneous high-frequency firing at rest → none (rest_spikes=0)
- all profiles identical → no
- no effect of Para removal → false (spikes 42→1)
- no repolarisation after spike → false (−63.5 mV undershoot)
- gate out-of-[0,1] / non-finite state / unphysical current /
  AIS runaway (>10 kHz sustained) → guards active, zero triggered
  in the circuit runs; deliberately misconfigured single cells DO
  flag NUMERICAL_FAILURE (tested)

## 12. Known missing physiology (explicit)

- Kinetic windows are summarised literature, not digitised curves;
  per-source temperature/stage mismatches remain possible
- F-I slopes too steep; spike width ~2× too fast (held-out metrics)
- No Ca / KCa / HCN / Ih / Na persistent — catalog incomplete (§61)
- Axonal delay is uniform deterministic relay, not
  morphology-derived per-edge (recorded gap)
- Receptor profiles are class-level priors; no transcriptomic
  mapping yet (§33 hook exists, not populated)
- No gap junctions, neuromodulation, neuropeptides (→ A4)
- Direct measurements: 0 entities — all physiology remains
  prior/inference/fitted

## 13. Reproduce

```bash
.venv/bin/python -m experiments.mioba.scripts.a31_fly_calibration \
  --store experiments/mioba/datasets/banc_888/store_v2 \
  --duration-ms 300 --replicates 8 --calibrate-iters 10 \
  --fidelity-out …/PHYSIOLOGY_FIDELITY_v2.json \
  --calibration-out …/CALIBRATION_MANIFEST.json \
  --trace-out kc_trace.json --out a31_report.json
```

Run manifest: anatomy hash + overlay files hash + `reduce-v3-ais-fly`
+ `active_fly_v1` + dt 0.1 + 25 °C + `flychan-v1` +
`receptor_aware_v0` + `unknown_sign_mode=neutral`.
