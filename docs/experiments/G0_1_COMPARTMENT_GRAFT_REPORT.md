# G0.1 — Compartment-Aware Graft on a Real Anatomical Circuit

Date: 2026-09-16 · Store: `store_v2` (schema v2)
Runner: `experiments/mioba/scripts/g01_compartment_graft.py`
Raw report: `/tmp/g01.json` (regenerate via the runner)

## 1. Setup (§26)

Host circuit: **240 canonical anatomical entities** (32 afferent
drivers + 32 descending + 176 strongest 1-hop partners), 546 intra-
circuit edges, expanded to **858 runtime compartment nodes**
(237 entities multi-compartment). Same seed (20260916), same drive
(afferent SOMA nodes @ 30 Hz), 300 ms, 4 paired replicates.

Graft: `graft:g001` — 10 artificial neurons, recurrent p=0.15, LIF.
All host↔graft synapses are recorded `ARTIFICIAL_GRAFT` provenance;
host anatomy is read-only (§24/§25). Weights are MODEL_INFERENCE demo
values (in-link 30, out-link 80, p=0.5) — chosen so events clear the
7 mV threshold gap after synaptic filtering; not physiological claims.

## 2. Conditions A / B / C (§26)

| Cond | Wiring | Spikes per lane | Graft spikes |
|---|---|---|---|
| A — host only | — | 303, 273, 289, 305 | 0 |
| B — neuron graft | graft→host onto entity **SOMA** nodes (31 targets) | 376, 333, 359, 379 | 66, 58, 68, 68 |
| C — compartment graft | graft→host onto **DENDRITE_DIST** compartments (30 targets) | 396, 357, 378, 392 | 66, 58, 68, 68 |

- host→graft propagation: graft fires ~19–22 Hz/cell (identical in B
  and C — same in-link, as expected).
- graft→host propagation: B adds ~+7 host spikes/lane; C adds ~+27 —
  the compartment landing site measurably changes the causal effect
  (dendritic delivery spreads charge through the compartment chain).
- **Determinism: PASS** (identical per-lane counts on re-run).

## 3. Causal ablation of condition C (§27)

Δ spikes vs full, per lane:

| Ablation | Δ (full − ablated) | Interpretation |
|---|---|---|
| graft disabled | 93, 84, 89, 87 | removes graft spikes (66) + graft→host effect (~27) |
| afferent disabled | 93, 84, 89, 87 | host→graft cut → graft silent → whole contribution gone |
| efferent disabled | 27, 26, 21, 19 | graft still fires (66) but graft→host effect isolated |

The three ablations separate host→graft, graft-internal and
graft→host contributions cleanly — the acceptance goal of §27.

## 4. Schema / targeting (§21–§23)

- `HostSelector.compartment_type` narrows candidates to entities that
  *provably* carry that compartment on the link-relevant side of the
  split edgelist — data-grounded, never assumed.
- Strict mode (default) rejects anatomically invalid requests —
  e.g. an `in`-link (host as source) targeting DENDRITE /
  PRIMARY_NEURITE raises; permissive mode warns and records.
- Each attachment records `connection_provenance: ARTIFICIAL_GRAFT`
  plus the resolved selector/compartment in `resolution`.

## 5. Terminology & limits (§30)

- Graft edges are *artificial graft synapses attached to real
  anatomical host entities/compartments* — not "real graft edges".
- Compartment labels derive from upstream split SWCs
  (RAW_EM_DERIVED); the reduction and all coupling/weight parameters
  are MODEL_INFERENCE.
- Entities without labeled skeletons expose only SOMA and are
  unreachable for compartment selectors — counted, not silently
  remapped.
- No claim of physiological efficacy: the demo weights are tuned
  model parameters.
