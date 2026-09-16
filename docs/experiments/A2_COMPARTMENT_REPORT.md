# A2 — Morphology Ingestion & Point-vs-Reduced Compartment Runtime

Date: 2026-09-16 · Store: `experiments/mioba/datasets/banc_888/store_v2` (schema v2)
Runner: `experiments/mioba/scripts/a2_compartment_compare.py`
Raw report: `/tmp/a2_compare.json` (regenerate via the runner)

## 1. Morphology ingestion (§11–§13)

Two SWC sources were used:

| Source | Count | Compartment labels | Role |
|---|---|---|---|
| `neuron_skeletons.zip` (bulk archive) | 86,482 files | SWC `type` column present but **0 everywhere** (unlabeled) | full-skeleton canonical store (Layer A) |
| `banc_banc_space_split_swc/` per-file | fetched per circuit (160/240 files typical) | **labeled** — 1 soma / 2 axon / 3 dendrite / 4 primary neurite / 7 soma-like | compartment reduction input |

Bulk ingestion result (`anatomy/morphology.py::ingest_skeletons`):

- files ingested: **86,482**
- SWC nodes parsed: **9,389,225**
- validated: **86,482 / valid: 86,482** (duplicate ids, broken parents,
  multiple roots, reachability, NaN/impossible coordinates — §13)
- invalid files are recorded, never repaired silently
  (`morphology_validation_status`).

Entities with any morphology: 86,482 of 204,257 canonical entities
(~42%). Absence is recorded as `morphology_status: UNKNOWN` — never
fabricated (§12).

## 2. Compartment schema (§14–§16)

Runtime compartment types: `SOMA`, `DENDRITE_PROX`, `DENDRITE_DIST`,
`AXON`, `PRIMARY_NEURITE` (mapped onto the required SOMA/DENDRITE/AXON/
PRIMARY_NEURITE/UNKNOWN taxonomy). Full SWC is retained; the runtime
graph is a **compiled reduction** (`reduce-v0`), never a replacement
(§16).

Split-synapse compartment labels (Layer A `connections_split`, BANC
`edgelist_split_v3`: per-pair axon/dendrite/primary-neurite fractions)
drive post-synaptic routing: an edge labeled DENDRITE lands on the
target's distal-dendrite compartment; UNKNOWN → SOMA, recorded
honestly (§15, `mapping_provenance: MODEL_INFERENCE` where inferred).

## 3. Reduced runtime semantics (§17)

- Nodes = `(entity, compartment)` pairs; entities without a labeled
  skeleton keep a single SOMA node (point fallback).
- Intra-entity coupling: passive voltage diffusion along the chain
  AXON↔SOMA↔DENDRITE_PROX↔DENDRITE_DIST
  (`tauCoup`, `coupling_c` — **MODEL_INFERENCE**, no measured
  conductances exist in BANC).
- Spike semantics: **soma is the only emitter** — the LIF reset is
  instantaneous, so passive diffusion cannot relay a spike down an
  axon; all out-edges emit from SOMA. AXON exists as an input-side
  compartment (axo-axonic inputs) and morphology anchor.
- Dendrites never spike in this model: they filter and relay charge to
  the soma. This is deliberate — dendritic spikes require A3
  biophysics.

## 4. Point vs reduced comparison (§19)

Same circuit, same seed (20260916), same drive (32 afferents @ 30 Hz,
300 ms, 2 replicates):

| Mode | Runtime nodes | Spikes (lanes) | Propagated edge events | Wall |
|---|---|---|---|---|
| point | 160 | 197, 175 | 185 | 0.13 s |
| reduced | 552 | 205, 175 | 166 | 0.16 s |

- Circuit: 160 entities (32 afferent drivers + 32 descending + 96
  strongest 1-hop partners), 235 intra-circuit edges.
- 157/160 entities had labeled split SWCs → 237→552 compartment nodes
  (≈3.5 nodes/entity).
- **Determinism: PASS** (identical per-lane spike counts on re-run).
- Reduced mode produces *different* spike counts — anatomy affects
  dynamics (dendritic filtering delays/attenuates delivery). The goal
  was not high-precision agreement (§19) but a non-degenerate,
  structure-sensitive compartment runtime: PASS.
- Reduced overhead at this scale: ~1.2× wall time; coupling matvec is
  one extra sparse op per step.

## 5. Reduction manifest (§18)

`compile_reduced_graph` emits per-entity rows:
`original_swc_nodes`, `runtime_compartments`, `reduction_ratio`,
`mapping_hash`, plus `algorithm: reduce-v0`, coupling parameters and
their provenance. See `reduction_manifest` in the raw report.

## 6. Known missing physiology (honest limits)

- No measured membrane conductances — all coupling/LIF parameters are
  MODEL_INFERENCE.
- No dendritic spikes, no active channels (A3).
- Bulk skeleton `type` column is unlabeled (0); compartment labels come
  from the split SWC set only → entities without split files run as
  point fallback, counted in the manifest.
- Gap junctions remain UNKNOWN.

## 7. Terminology (§30)

Counts throughout use *canonical anatomical entities* (204,257), of
which 128,153 are classified biological neurons; 13.6M refers to
*aggregated directed connection pairs* representing 42.3M chemical
synapse counts — not "13.6M synapses".
