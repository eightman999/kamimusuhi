# CX0 Synthetic Cortex — Results

- generated from `experiments/cx0/runs_v2` (1300 rows)
- params: c0=None, c1=None, c3=None, c2=None
- best cortex arm: **c1** (mean clean 0.778)

## Clean + intervention success matrix

| task | arm | clean | shuffle(sensory) | shuffle(h0) | shuffle(s0) | shuffle(t0) | shuffle(r0) | cortex_off | reset_hidden | erase_mem |
|---|---|---|---|---|---|---|---|---|---|---|
| ctx1 | c0 | 0.621 | 0.567 | 0.613 | 0.700 | 0.621 | 0.158 | 0.000 | 0.621 | 0.042 |
| ctx1 | c1 | 0.750 | 0.729 | 0.758 | 0.796 | 0.754 | 0.212 | 0.000 | 0.646 | 0.008 |
| ctx1 | c2 | 0.692 | 0.679 | 0.621 | 0.692 | 0.692 | 0.188 | 0.000 | 0.183 | 0.008 |
| ctx1 | c3 | 0.013 | 0.013 | 0.000 | 0.013 | 0.013 | 0.008 | 0.000 | 0.000 | 0.000 |
| ctx2 | c0 | 0.804 | 0.100 | 0.833 | 0.804 | 0.700 | 0.192 | 0.000 | 0.804 | 0.000 |
| ctx2 | c1 | 0.896 | 0.096 | 0.900 | 0.871 | 0.892 | 0.188 | 0.000 | 0.129 | 0.004 |
| ctx2 | c2 | 0.879 | 0.092 | 0.912 | 0.904 | 0.883 | 0.150 | 0.000 | 0.000 | 0.000 |
| ctx2 | c3 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 |
| ctx3 | c0 | 0.846 | 0.863 | 0.854 | 0.850 | 0.354 | 0.312 | 0.000 | 0.846 | 0.446 |
| ctx3 | c1 | 1.000 | 0.996 | 1.000 | 1.000 | 0.346 | 0.229 | 0.104 | 0.696 | 0.575 |
| ctx3 | c2 | 0.946 | 0.933 | 0.946 | 0.946 | 0.263 | 0.346 | 0.058 | 0.646 | 0.562 |
| ctx3 | c3 | 0.804 | 0.808 | 0.812 | 0.804 | 0.242 | 0.204 | 0.000 | 0.408 | 0.475 |
| ctx4 | c0 | 0.792 | 0.692 | 0.762 | 0.738 | 0.721 | 0.108 | 0.000 | 0.792 | 0.000 |
| ctx4 | c1 | 0.754 | 0.654 | 0.721 | 0.754 | 0.671 | 0.079 | 0.000 | 0.754 | 0.000 |
| ctx4 | c2 | 0.600 | 0.487 | 0.608 | 0.525 | 0.558 | 0.142 | 0.000 | 0.533 | 0.000 |
| ctx4 | c3 | 0.013 | 0.013 | 0.021 | 0.008 | 0.021 | 0.013 | 0.000 | 0.000 | 0.000 |
| ctx5 | c0 | 0.479 | 0.554 | 0.537 | 0.504 | 0.483 | 0.492 | 0.171 | 0.479 | 0.517 |
| ctx5 | c1 | 0.492 | 0.521 | 0.533 | 0.508 | 0.496 | 0.496 | 0.025 | 0.446 | 0.475 |
| ctx5 | c2 | 0.529 | 0.558 | 0.554 | 0.525 | 0.529 | 0.517 | 0.000 | 0.525 | 0.529 |
| ctx5 | c3 | 0.392 | 0.338 | 0.375 | 0.388 | 0.392 | 0.379 | 0.000 | 0.013 | 0.400 |

## Gates

| gate | criterion | value | verdict |
|---|---|---|---|
| C-G1 solvability | best-arm mean clean ≥ 0.30 | 0.778 | PASS |
| C-G2 cortex edge | c1 > c1+0.05 | 0.778 vs 0.778 | FAIL |
| C-G3 organ causal | mean shuffle drop > 0.10 | 0.170 | PASS |
| C-G4 context state | reset drop > 0.05 | 0.244 | PASS |
| C-G5 no harm | ctx5 c1 ≥ c0−0.05 | - | PASS |
| C-G6 decode | context probe > 0.45 | - | PASS (acc 0.593) |
| null check | cortex_off ≈ clean (bad) | drop 0.753 | OK |

## Verdict

**PARTIAL** (best arm c1; see gates above)

### Interpretation notes

- Clean success is a *behavioral* metric; the causal story is in the intervention columns.
- `probe_*` columns in results.jsonl give per-population context decode accuracy (C-G6).