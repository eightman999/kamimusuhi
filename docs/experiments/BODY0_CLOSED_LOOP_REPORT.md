# Body0 — Minimal Closed Sensorimotor Loop

Date: 2026-09-16
Upstream: A3.2 `active_fly_v1_1` (calibrated flychan-v1_1), G0.1 graft
Run: `experiments/mioba/scripts/body0_orient.py` →
`docs/experiments/body0/` (body0_report.json, port_manifest.json,
BODY_FIDELITY.json, episode_traces/*.jsonl)

## 1. Architecture

```
env (2D target, relocates at t=2 s)
 → encode: bearing → bilateral Poisson rates (MODEL_INFERENCE)
 → BANC sensory port (29 L + 32 R entities; super_class ∈
   {sensory, visual_projection, sensory_ascending}, side-split;
   identity CURATED_ANNOTATION, SOMA conductance drive)
 → host CNS: 240 entities / 1095 runtime nodes, active_fly_v1_1
   with A3.2-fitted densities
 → motor port: 8 + 8 descending/motor entities, AIS spike counts,
   side-split (identity CURATED_ANNOTATION, grouping
   MODEL_INFERENCE)
 → actuator: v_ang = k(dL−dR), v_lin = k·(dL+dR)/2, baseline-
   subtracted (MODEL_INFERENCE)
 → 2D unicycle body (x, y, heading) → env
```

Control tick = 20 ms (200 sim steps of 0.1 ms); motor decode is a
trailing 100 ms window (per-tick counts are ~1 spike and too coarse).

## 2. Circuit selection — the honest hard part

`_pick_circuit` (random afferents + descending + 1-hop partners)
produced **zero** sensory→motor paths in the runtime graph — measured,
not assumed. Body0 therefore uses `pick_sensorimotor_circuit`:
descending/motor entities whose shortest-path distance from the
**left** sensory field is smaller than from the right (dL<dR) form the
"left-turn" pool (and mirrored), then shortest BFS paths back to
same-side sensory sources are kept (85 path entities), filled to 240
with strongest 1-hop partners. Entity *identity* stays
CURATED_ANNOTATION; the L/R *grouping* is declared MODEL_INFERENCE.

Runtime-graph audit (synapses + compartment coupling):
sensL→motL = 8/8, sensR→motR = 8/8, cross terms nonzero — real
paths, verified in `port_manifest.json`.

## 3. Hidden-bypass forensics (§45)

Two fake-effect mechanisms were found and fixed:

- **RNG-stream leak**: zeroing a drive removed the node from the
  Poisson draw set, perturbing the *whole* RNG stream → ablations
  "changed" behavior without any anatomical path. `set_inputs` now
  keeps explicitly-listed zero-rate nodes in the draw set, so
  sensory_ablation differs only by missing conductance pulses.
- **Intrinsic dominance**: uncalibrated v1_1 defaults fired motor
  pops at ~70–80 Hz with or without synapses (verified by zeroing
  all weights) — spike output was independent of input. The A3.2
  *fitted* densities quiet the network; sensory conduction then
  required `weight_to_g=5.0` (MODEL_INFERENCE free parameter) to
  produce measurable modulation (R drive → motR +13 Hz,
  L drive → motL +3 Hz, correct directions).

## 4. Results — 4 s episodes, seeds {20260916, 20260917}

mean |heading error| over tail half (rad):

| condition | s…916 | s…917 |
|---|---|---|
| oracle (proportional) | 0.284 | — |
| host_only | 0.337 | 0.372 |
| host+graft | 0.484 | 0.492 |
| graft efferent cut | 0.337 | 0.372 |
| graft afferent cut | 0.337 | 0.372 |
| sensory ablation | 0.391 | 0.387 |
| motor ablation | 0.371 (no motion) | 0.371 (no motion) |

- **Loop closed**: motion at 20–380 ms; motor spikes at 20 ms;
  sensor→motor latency ≈ 20 ms (one control tick).
- **Sensory ablation changes behavior**: Δ = [−0.094, −0.105] rad vs
  host_graft, consistent sign across seeds, above the 0.01 floor.
- **Motor ablation removes motion**: `first_motion_latency_s = None`
  in both seeds.
- **Determinism**: paired rerun of host_graft seed 20260916 produced
  identical metrics.
- Graft currently *degrades* orienting (0.48 vs 0.34) — expected and
  allowed (§53); the point is comparability, and graft ablations
  reproduce host_only exactly.

## 5. Fidelity classification (BODY_FIDELITY.json)

| component | provenance |
|---|---|
| host anatomy/connectivity | DIRECT_MEASUREMENT (BANC, immutable) |
| host physiology | flychan-v1_1: kinetics LITERATURE_PRIOR, fitted densities MODEL_INFERENCE |
| sensory/motor entity identity | CURATED_ANNOTATION |
| L/R population grouping | MODEL_INFERENCE |
| sensory encoding, motor decode, actuator, body, environment | MODEL_INFERENCE |
| graft | ARTIFICIAL_GRAFT |

## 6. Known simplifications

- No biomechanics; heading is driven directly by decoded asymmetry.
- Sensory drive is population-wide Poisson at SOMA — no receptor
  field structure.
- Motor decode uses population rate only; temporal codes unexplored.
- `weight_to_g=5.0` is a free conduction-gain parameter — the
  synapse-count→conductance mapping remains weakly constrained.
- The graft has no task-relevant function yet (G1 question).
- GUI/Observatory replay not wired (optional per §48).

## 7. Exit status

All §52 criteria met: loop closes through real BANC anatomy,
ablations are causal (not noise), paired seeds deterministic, host
anatomy unchanged, no direct sensor→body or graft→body path.
**Body0 complete → G1 (100-neuron artificial cortex) unblocked.**
