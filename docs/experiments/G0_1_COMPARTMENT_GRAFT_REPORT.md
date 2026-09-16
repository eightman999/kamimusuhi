# G0.1 — Compartment-Aware Graft, Causal Validation (A2.1 hardened)

Date: 2026-09-16 · Store: `store_v2` · Graph: `reduce-v1-split-synapse`
(permissive) · Runner:
`experiments/mioba/scripts/g01_compartment_graft.py`
Raw report: `/tmp/g01.json`

## 1. Setup

Host circuit: **240 canonical anatomical entities** (32 afferent
drivers + 32 descending + 176 strongest 1-hop partners), expanded to
**858 runtime compartment nodes**, 629 split edges (0 UNKNOWN fallback
inside this circuit). 300 ms, **8 paired replicates**, seed 20260916.

Graft `graft:g001`: 10 artificial LIF neurons, recurrent p=0.15,
`connection_provenance: ARTIFICIAL_GRAFT` on every host↔graft edge;
host anatomy read-only (§24/§25). Weights (in 30 / out 80, p=0.5) are
MODEL_INFERENCE demo values — they are tuned so single events clear
the ~7 mV threshold gap after synaptic filtering; no physiological
claim.

## 2. Matched target set (§13–§15)

`matched_target_entities`: **30 descending entities** that provably
carry BOTH SOMA and DENDRITE_DIST compartments — identical entity IDs
in B and C, asserted at run time (mismatch ⇒ FAIL). UNKNOWN or
point-fallback entities are never targets (§12). Afferent sources are
the same SOMA nodes in B and C; the only difference between the two
conditions is the efferent landing compartment.

| Cond | Efferent landing | Host spikes (8 lanes) | Graft spikes |
|---|---|---|---|
| A — host only | — | 303 273 289 305 293 282 300 292 | 0 |
| B — graft→SOMA | SOMA | 315 278 299 311 299 285 308 298 | 66 58 68 68 64 61 65 61 |
| C — graft→DENDRITE_DIST | DENDRITE_DIST | 330 299 310 324 321 300 322 313 | 66 58 68 68 64 61 65 61 |

host→graft transmission: graft fires ~19–22 Hz/cell in both B and C
(identical in-links, identical activity — as required by §15).

## 3. Compartment-level causal effect (§18–§20)

Per-target-compartment telemetry (input_event_count,
synaptic_input_sum, membrane_voltage_mean/peak) on all 30
DENDRITE_DIST targets + their 30 somas, paired full vs
efferent_disabled:

- **30/30 target compartments** show non-zero paired deltas:
  Δevents ≈ +5–9 per lane, Δsynaptic_input ≈ +71–128,
  ΔV_mean ≈ +0.45–0.83 mV, ΔV_peak ≈ +1.5–1.8 mV.
- Host spike effect: +19–27 spikes/lane over the host-only baseline —
  recorded, and happens to be positive here, but per §21 it is not a
  pass criterion.

## 4. Ablation on C (§23–§24)

| Condition | Host spikes | Graft spikes | Interpretation |
|---|---|---|---|
| full | 330 299 310 324 321 300 322 313 | 66…61 | — |
| graft_disabled | = A baseline | 0 | whole artificial-organ contribution |
| afferent_disabled | = A baseline | 0 | host→graft input is necessary for graft activity |
| efferent_disabled | = A baseline | 66…61 (unchanged) | graft→host effect isolated at target compartments |
| internal_recurrence_disabled | = full | = full | internal recurrence not required for this feedforward demo |

## 5. Causal verdict (§35/§36)

| Link | Result |
|---|---|
| host → graft transmission | **PASS** |
| graft → target compartment | **PASS** (paired non-zero Δevents/Δinput/ΔV) |
| graft → soma | PASS (not required) |
| graft → host spike output | PASS (not required) |

Determinism: **PASS** — identical per-lane host spikes, graft spikes
and full telemetry on re-run (bit-exact, §26).

## 6. Integrity & limits

- Native host graph read-only; no native synapse rewiring; no host
  morphology mutation (§36).
- All graft synapses ARTIFICIAL_GRAFT; all weights MODEL_INFERENCE.
- Graft internal activity is reported separately from host spikes —
  no all-mean-rate mixing (§22).
- Not required and not claimed: behaviour, learning, active
  dendritic/axonal dynamics, measured conductances (§37).
