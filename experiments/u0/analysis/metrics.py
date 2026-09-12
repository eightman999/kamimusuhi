"""Metric definitions and aggregation for U0.

Primary metrics (per episode, averaged over the eval corpus):
    error_full            homeostatic error integrated over the whole
                          episode; post-death steps count at the error
                          measured at death, so dying early cannot look
                          successful
    mean_error            error over lived steps only
    survival_fraction     steps lived / episode length
    stable_fraction       fraction of lived steps with all variables in
                          their preferred ranges
    need_resolution       fraction of fired needs resolved by ACTing at
                          a matching site before episode end
    important_retention   fraction of fired needs whose matching site
                          was present in memory at the need's onset
    store_precision       fraction of STORE actions written on events
                          whose function matched a need that fired
    stores / recalls / moves / acts / stores_junk / memory_occupied

Gate metrics derived in report.py:
    U-H1 improvement      1 - err_learned / err_best_baseline
    U-H3 delta_err        (err_causal - err_clean) / err_clean
    U-H4 mean_abs_delta   mean |P(store|func, adverse) - P(store|func, safe)|
    oracle_gap            err_learned - err_oracle
"""

from __future__ import annotations

import numpy as np

NUMERIC_METRICS = [
    "error_full", "mean_error", "survival_fraction", "stable_fraction",
    "need_resolution", "important_retention", "store_precision",
    "stores", "stores_functional", "stores_useful", "stores_junk",
    "recalls", "moves", "acts", "memory_occupied", "needs_fired",
    "needs_resolved", "died",
]

GATE_THRESHOLDS = {
    "U-H1_improvement": 0.20,     # vs best(random, fifo)
    "U-H2_store_precision": 0.75,
    "U-H2_important_retention": 0.75,
    "U-H3_delta_err": 0.30,       # erase OR shuffle
    "U-H4_mean_abs_delta": 0.20,
    "U-H5_min_seeds": 3,
    "strong_oracle_gap": 0.15,
    "strong_delay128_degradation": 0.20,   # err ratio vs clean
    "strong_distractor4x_precision": 0.70,
}
