# CX0 Synthetic Cortex — Results

- generated from `experiments/cx0/runs` (1300 rows)
- params: c0=None, c1=None, c3=None, c2=None
- best cortex arm: **c1** (mean clean 0.382)

## Clean + intervention success matrix

| task | arm | clean | shuffle(sensory) | shuffle(h0) | shuffle(s0) | shuffle(t0) | shuffle(r0) | cortex_off | reset_hidden | erase_mem |
|---|---|---|---|---|---|---|---|---|---|---|
| ctx1 | c0 | 0.292 | 0.287 | 0.371 | 0.300 | 0.296 | 0.296 | 0.000 | 0.292 | 0.283 |
| ctx1 | c1 | 0.233 | 0.192 | 0.217 | 0.225 | 0.225 | 0.221 | 0.000 | 0.050 | 0.354 |
| ctx1 | c2 | 0.154 | 0.142 | 0.096 | 0.138 | 0.163 | 0.163 | 0.000 | 0.058 | 0.133 |
| ctx1 | c3 | 0.108 | 0.133 | 0.100 | 0.071 | 0.108 | 0.092 | 0.000 | 0.167 | 0.121 |
| ctx2 | c0 | 0.025 | 0.058 | 0.033 | 0.021 | 0.033 | 0.033 | 0.000 | 0.025 | 0.000 |
| ctx2 | c1 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.000 | 0.037 | 0.000 |
| ctx2 | c2 | 0.050 | 0.042 | 0.054 | 0.037 | 0.033 | 0.046 | 0.042 | 0.050 | 0.000 |
| ctx2 | c3 | 0.075 | 0.054 | 0.079 | 0.075 | 0.075 | 0.075 | 0.021 | 0.000 | 0.075 |
| ctx3 | c0 | 0.833 | 0.783 | 0.833 | 0.825 | 0.342 | 0.362 | 0.000 | 0.833 | 0.454 |
| ctx3 | c1 | 0.967 | 0.954 | 0.971 | 0.967 | 0.350 | 0.342 | 0.000 | 0.754 | 0.646 |
| ctx3 | c2 | 0.808 | 0.796 | 0.808 | 0.804 | 0.246 | 0.388 | 0.050 | 0.567 | 0.533 |
| ctx3 | c3 | 0.996 | 0.992 | 0.992 | 0.996 | 0.283 | 0.250 | 0.000 | 0.508 | 0.579 |
| ctx4 | c0 | 0.100 | 0.117 | 0.071 | 0.083 | 0.154 | 0.100 | 0.000 | 0.100 | 0.133 |
| ctx4 | c1 | 0.362 | 0.271 | 0.412 | 0.333 | 0.308 | 0.317 | 0.000 | 0.138 | 0.212 |
| ctx4 | c2 | 0.087 | 0.083 | 0.067 | 0.117 | 0.183 | 0.062 | 0.000 | 0.050 | 0.150 |
| ctx4 | c3 | 0.062 | 0.050 | 0.104 | 0.096 | 0.092 | 0.083 | 0.000 | 0.138 | 0.058 |
| ctx5 | c0 | 0.458 | 0.450 | 0.433 | 0.475 | 0.458 | 0.467 | 0.000 | 0.458 | 0.458 |
| ctx5 | c1 | 0.346 | 0.346 | 0.308 | 0.325 | 0.350 | 0.296 | 0.000 | 0.312 | 0.325 |
| ctx5 | c2 | 0.617 | 0.600 | 0.621 | 0.612 | 0.617 | 0.621 | 0.025 | 0.475 | 0.617 |
| ctx5 | c3 | 0.621 | 0.596 | 0.596 | 0.596 | 0.625 | 0.617 | 0.000 | 0.071 | 0.608 |

## Gates

| gate | criterion | value | verdict |
|---|---|---|---|
| C-G1 solvability | best-arm mean clean ≥ 0.30 | 0.382 | PASS |
| C-G2 cortex edge | c1 > c1+0.05 | 0.382 vs 0.382 | FAIL |
| C-G3 organ causal | mean shuffle drop > 0.10 | 0.064 | FAIL |
| C-G4 context state | reset drop > 0.05 | 0.123 | PASS |
| C-G5 no harm | ctx5 c1 ≥ c0−0.05 | - | FAIL |
| C-G6 decode | context probe > 0.45 | - | PASS (acc 0.627) |
| null check | cortex_off ≈ clean (bad) | drop 0.382 | OK |

## Verdict

**PARTIAL** (best arm c1; see gates above)

### Interpretation notes

- Clean success is a *behavioral* metric; the causal story is in the intervention columns.
- `probe_*` columns in results.jsonl give per-population context decode accuracy (C-G6).