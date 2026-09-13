# U0 Protocol Sanity Check
- verdict: **PASS**
- episodes per condition: 512 (seed 900001)
- git commit: `26d7fdc`
- elapsed: 13.3 s

## Baseline matrix (clean)
| baseline | need_resolution | crisis_error_auc | time_to_resolution | survival_after_need | retention |
|---|---|---|---|---|---|
| oracle | 0.920 | 0.827 | 8.6 | 0.920 | 1.000 |
| no_memory | 0.172 | 10.310 | 62.5 | 0.172 | 0.000 |
| random | 0.261 | 8.612 | 57.7 | 0.261 | 0.276 |
| fifo | 0.388 | 7.301 | 49.0 | 0.388 | 0.552 |
| lru | 0.388 | 7.301 | 49.0 | 0.388 | 0.552 |
| store_all | 0.310 | 8.302 | 53.9 | 0.310 | 0.379 |
| heuristic_current_need | 0.158 | 9.847 | 65.4 | 0.158 | 0.071 |
| oracle + erase | 0.133 | 10.459 | 67.6 | 0.133 | 0.000 |

## Gates
| gate | criterion | value | verdict |
|---|---|---|---|
| P0-1 | oracle need_resolution >= 0.9 | 0.920 | PASS |
| P0-2 | no_memory need_resolution < 0.4 | 0.172 | PASS |
| P0-3 | oracle crisis_error_auc < no_memory crisis_error_auc | oracle=0.827, no_memory=10.310 | PASS |
| P0-4 | oracle not worse than fifo/random on primary metrics (resolution AND crisis_error_auc) | oracle_res=0.920, fifo_res=0.388, random_res=0.261, oracle_auc=0.827, fifo_auc=7.301, random_auc=8.612 | PASS |
| P0-5 | erased oracle degrades clearly (resolution drop >= 0.3) | clean_res=0.920, erased_res=0.133, drop=0.787 | PASS |

All gates passed — PPO training is allowed.
