# A2.1 — Split-Synapse Runtime & Compartment Fidelity

Date: 2026-09-16 · Store: `store_v2` (schema v2) · Algorithm:
`reduce-v1-split-synapse` (v0 preserved)
Runner: `experiments/mioba/scripts/a2_compartment_compare.py`
Raw report: `/tmp/a2_compare.json`

## 1. What changed vs A2

| Issue | Fix |
|---|---|
| PRIMARY_NEURITE silently folded into dendrites | declared in `manifest.compartment_reduction` (`runtime_mapping: DENDRITE_PROX_OR_DIST`, MODEL_INFERENCE) — never reported as a runtime compartment (§1) |
| split synapses collapsed onto dominant compartment | `compile_reduced_graph_v2` emits **one runtime edge per (pre,post,pre_comp,post_comp) row** of `connections_split` (§2–§4) |
| UNKNOWN → SOMA fallback invisible | every fallback edge carries `fallback_used` + `fallback_reason` + `mapping_provenance: MODEL_INFERENCE`; manifest reports the §10 counters (§9) |
| `soma_axon_w` unused parameter | removed (§32, option B) |
| soma path-distance only walked children | distance is now computed on the **undirected** tree from the soma — non-root somas no longer drop parent-side branches into dist=0 (§27–28) |
| UNKNOWN-labelled SWC nodes silently mapped PROX | counted as `unknown_swc_nodes` / `unknown_nodes_fallback_mapped` per entity (§30–31) |

## 2. Circuit comparison (160 entities, seed 20260916, 200 ms, 2 repl.)

| Mode | Runtime edges | Spikes | Wall |
|---|---|---|---|
| point (aggregated pairs) | 235 | 197, 175 | 0.13 s |
| reduced-v0 (dominant collapse) | 235 | 205, 175 | 0.16 s |
| reduced-v1-split-synapse | **272** | 205, 175 | 0.16 s |

- split edges total: **272** (vs 235 aggregated pairs — 37 extra
  compartment placements preserved instead of collapsed)
- edges by anatomical post_compartment: DENDRITE 187, AXON 79,
  PRIMARY_NEURITE 1, **UNKNOWN 5**
- UNKNOWN fallback: **5/272 → SOMA** (permissive), each flagged in the
  edge audit; `fallback_fraction` 0.018
- strict mode: UNKNOWN post-compartment edges are skipped and counted
  (`split_edges_skipped_strict`) — unit-tested (§11)
- determinism: v0 PASS, v1 PASS

## 3. Weight mapping (§6)

`weight = anatomical_count / 32` applied **per split edge**
(count_scaled_v0, MODEL_INFERENCE) — never summed-then-redistributed.
Each audit row keeps the full §5 record: pre/post entity, anatomical
pre/post compartment, anatomical_count, runtime_weight,
weight_provenance, fallback flags.

## 4. Presynaptic compartment (§7)

Anatomical `pre_compartment` is preserved in the edge audit, while the
runtime emitter remains SOMA — `manifest.presynaptic_note` states this
explicitly. Active axonal conduction stays out of scope until A3 (§8);
the schema keeps AXON as a first-class compartment so the extension
SOMA→AXON→terminal remains possible.

## 5. Soma-distance verification (§27–29)

New fixture `root → branch → soma → {d1, d2}` (soma not the root):
after the undirected-walk fix every reachable node receives a
distance; the parent-side dendrite 60 nm above the soma classifies
DENDRITE_DIST (it silently became PROX before). Covered by
`test_soma_not_root_gets_distances_everywhere`.

## 6. Remaining honest limits

- UNKNOWN compartments are not invented; permissive fallback routes
  them to SOMA *with an audit trail* — strict mode drops them.
- The 5 UNKNOWN-post edges in this circuit are upstream label gaps,
  not model output.
- Coupling/weights remain MODEL_INFERENCE; no measured conductances.
