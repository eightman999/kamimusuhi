# U0 Protocol Sanity Check
- verdict: **PASS**
- episodes per condition: 512 (seed 900001)
- git commit: `f05e712`
- elapsed: 9.9 s

## Baseline matrix (clean)
| baseline | need_resolution | crisis_error_auc | time_to_resolution | survival_after_need | retention |
|---|---|---|---|---|---|
| oracle | 0.998 | 0.151 | 3.8 | 0.998 | 0.998 |
| no_memory | 0.335 | 10.210 | 52.4 | 0.335 | 0.000 |
| random | 0.483 | 7.510 | 41.8 | 0.483 | 0.259 |
| fifo | 0.844 | 2.206 | 15.4 | 0.844 | 0.759 |
| lru | 0.844 | 2.206 | 15.4 | 0.844 | 0.759 |
| store_all | 0.662 | 5.127 | 29.0 | 0.662 | 0.478 |
| heuristic_current_need | 0.340 | 9.610 | 52.2 | 0.340 | 0.043 |
| oracle + erase | 0.325 | 10.409 | 53.6 | 0.325 | 0.000 |

## Gates
| gate | criterion | value | verdict |
|---|---|---|---|
| P0-1 | oracle need_resolution >= 0.9 | 0.998 | PASS |
| P0-2 | no_memory need_resolution < 0.4 | 0.335 | PASS |
| P0-3 | oracle crisis_error_auc < no_memory crisis_error_auc | oracle=0.151, no_memory=10.210 | PASS |
| P0-4 | oracle not worse than fifo/random on primary metrics (resolution AND crisis_error_auc) | oracle_res=0.998, fifo_res=0.844, random_res=0.483, oracle_auc=0.151, fifo_auc=2.206, random_auc=7.510 | PASS |
| P0-5 | erased oracle degrades clearly (resolution drop >= 0.3) | clean_res=0.998, erased_res=0.325, drop=0.673 | PASS |

All gates passed — PPO training is allowed.
