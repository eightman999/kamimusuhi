# A3 — Membrane Biophysics & Active Ion Channels

Date: 2026-09-16 · baseline `3a3e7416` + `6192fc6` · runtime fidelity **L3**

> Anatomically grounded BANC compartments now support explicit
> membrane and active-channel dynamics under provenance-tracked model
> parameters. (This is the strongest claim A3 makes — §36.)

## Design invariants

- BANC anatomy (entities, morphology, connectivity, compartment
  annotation, synapse counts) is untouched — read-only canonical
  store; physiology lives in a **separate overlay**
  (`datasets/banc_888/physiology/overlays/banc888/`,
  gitignored + regenerable).
- `runtime_mode: passive_lif` is preserved verbatim — A2.1/G0.1
  results reproduce exactly (frozen semantics; active code paths are
  gated on `runtime_mode == "active_hh_v0"`).
- `reduce-v1-split-synapse` is untouched; the new compiler is a
  separate function `compile_reduced_graph_v3`
  (`reduce-v2-biophysical`).

## Parameter sources (§3–§7, §37)

No fly membrane measurements exist in the store, so v0 ships **coarse
profile classes only** (§39): `generic_excitatory /
generic_inhibitory / generic_modulatory / sensory / descending /
motor`, assigned per entity by `nt_top`/`super_class`
(MODEL_INFERENCE assignment provenance; GENERIC_FALLBACK when no NT
label exists).

Membrane table per profile: `Cm=1.0 µF/cm²`, `g_leak=0.3 mS/cm²`,
`Ra=100 Ω·cm` (LITERATURE_PRIOR, generic neuron priors);
`E_leak=V_rest=-60 mV`, `V_threshold=-45`, `V_reset=-60`
(MODEL_INFERENCE operating point). Channels: `leak` + `generic_Na`
(HH m³h kinetics, E=+50) + `generic_Kdr` (HH n⁴, E=-77) — squid-axon
kinetics reused as **generic models, explicitly not Drosophila
channel reconstructions** (§10). Resolution order is wired for
neuron→cell-type→profile (§6); v0 has no direct rows, so every value
is honestly LITERATURE_PRIOR or MODEL_INFERENCE — `PHYSIOLOGY_
FIDELITY.json` shows 0 DIRECT_MEASUREMENT and that is the truth.

## Active channel implementation (§8–§12, §22)

`physio/channels.py`: pluggable `ChannelModel` —
`current(V,state)`, `conductance(V,state)`, `advance(dt,V,state)`,
`initial_state(V)`. Integration is **exponential-Euler in V with
cnexp gates**: gates update exactly at frozen V; V then solves the
linearised equation exactly, so V is bounded by the
reversal-potential convex hull and *cannot diverge* — forward Euler
blew up immediately (guard tripped at t=1.6 ms under strong drive)
and was replaced before any results were taken. Recorded as
`exponential_euler_V+cnexp_gates` in run manifests (§22, §43).

Spikes are **rising-edge threshold events** at `v_spike=-20 mV`
across the active state — not an instantaneous LIF reset (§12).
Active nodes repolarise through their own channels; LIF reset still
applies to passive nodes only.

## Single-cell validation (§29)

| test | result |
|---|---|
| A resting | settles at −65.1 mV (net outward K⁺ vs E_leak=−60 — the model's honest equilibrium), 0 spikes |
| B subthreshold (0.05 mS/cm² pulses @50 Hz) | V peaks −56 mV, **0 spikes** |
| C suprathreshold | spikes, repolarises, no NaN |
| D determinism | bit-identical spike counts and final V on reset+rerun |

## Conductance synapses (§19–§21)

Edges carry a per-edge reversal class `esyn` (mV) from the
**presynaptic** entity's `nt_top`: ACh→excitatory (E=0),
GABA/Glu→inhibitory (E=−80; GluCl is inhibitory in flies).
`NT_CLASS` is MODEL_INFERENCE and the audit records it per edge.
Unknown NT follows `unknown_nt_mode` — `generic_prior` (exc, audited
as `esyn_provenance=UNKNOWN`), `neutral` (same, kept for policy
experiments), `strict` (edge dropped, counted). On the G0.1 circuit:
**56/629 edges (8.9%) had UNKNOWN transmitter** and are visible in
the audit — never silently excitatory.

## Compartment propagation (§16–§18, §32)

All compartment nodes of a resolved entity run the membrane equation;
channel density is zero outside the channel's compartment list, so
dendrites carry leak+synaptic+axial currents only (`active_channels_
allowed` is effectively per-compartment via g_bar placement). Axial
coupling uses SWC-derived geometry when provided
(`physio/geometry.py`: effective length/radius/area per compartment,
`axial_coupling` = πr²/(Ra·L)/A_post — MODEL_INFERENCE), else the
scalar `coupling_c` fallback, recorded in the manifest.

Axonal output stays the **delayed_spike_relay** documented in §18:
soma spike → synaptic delay → presynaptic event; `pre_compartment`
remains an anatomical record. Full active axonal cable is deferred
(A3 §49 non-requirement).

## Graft causal comparison (§33–§35)

G0.1 circuit: 240 entities → 858 runtime nodes, 629 split edges, 30
matched targets (SOMA+DENDRITE_DIST, identical set in every
condition). 10-neuron LIF graft → DENDRITE_DIST, `weight_to_g=0.02`:

| condition | host spikes (4 rep) | graft spikes |
|---|---|---|
| full | 363, 314, 379, 331 | 28, 26, 32, 28 |
| efferent_disabled | 311, 288, 308, 307 | 28, 26, 32, 28 |
| afferent_disabled | 311, 288, 308, 307 | 0,0,0,0 |
| graft_disabled | 311, 288, 308, 307 | — |

- host→graft: graft silent when afferents cut ✓
- graft→target compartment: paired full-vs-efferent telemetry deltas
  non-zero (events +5–9, synaptic input, ΔV_peak) ✓
- graft→host spikes: +20–50 above baseline in full ✓
- determinism: identical counts on rerun ✓

## Numerical stability (§23–§24)

Guards: non-finite V or channel state, and V outside the declared
`[v_min, v_max]` band, set `status: NUMERICAL_FAILURE` with
`reason/at_t_ms` and stop stepping — no silent clipping or reset.
Because the V update is bounded, the guard's job is range/state
validity, not divergence. `numerical_failure_count = 0` on all A3
runs.

## Performance (§46–§47)

G0.1 circuit, 200 ms, 4 replicates, CPU:

| level | wall (s) | steps/s | spikes/s | state+edges mem |
|---|---|---|---|---|
| L1 point | 0.22 | ~9 300 | ~2 200 | ~0.1 MB |
| L2 passive | 0.33 | ~6 150 | ~2 350 | ~0.4 MB |
| L3 active | 2.7 | ~730 | ~440 | ~1.5 MB |

Whole-CNS estimate (linear extrapolation — **estimate only**):
~204k entities ⇒ ~730 k runtime nodes; L3 ≈ 8× L2 step cost;
state memory ≈ 1–2 GB order. Active-channel stepping is the
dominant cost; mixed fidelity (§25) is supported — nodes outside
`physiology.active_idx` stay on the frozen LIF path.

## Fidelity / provenance (§42–§43)

`PHYSIOLOGY_FIDELITY.json` (overlay level): 204,257 entities, all
active-mode capable; profile assignment 154,950 MODEL_INFERENCE +
49,307 GENERIC_FALLBACK; parameters 36 LITERATURE_PRIOR + 24
MODEL_INFERENCE, zero measured — recorded as zero, not smoothed over.
Run manifests carry anatomy hash, overlay file hashes, reduction
algorithm, fidelity level, dt, temperature (25 °C, MODEL_INFERENCE
default), channel-model version `chan-v0`, synapse version
`conductance_v0`.

## Observatory (§44–§45)

`export_anatomy_view.py --physiology <overlay>` adds a physiology
layer: per-entity profile, runtime mode, channel set, and
per-parameter provenance. The Anatomy view gets a *physiology layer*
toggle recolouring somata by provenance class (measured → unknown,
7-class legend). Live membrane voltage is **not** in the static
export — noted in-view and here as a gap.

## Known missing physiology (§49 — all deliberately absent)

- No Drosophila-specific channel families (generic Na/Kdr only).
- No direct or cell-type measurements — everything is prior/fallback.
- No dendritic spikes, no active axonal cable (delayed relay only).
- No gap junctions, no neuromodulation, no transcriptomic mapping
  (hook exists: `expression_to_conductance_mapping` would be a
  versioned MODEL_INFERENCE — §40-41).
- Transcript abundance is never used as conductance density.
