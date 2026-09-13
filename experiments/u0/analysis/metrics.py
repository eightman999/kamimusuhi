"""Metric definitions and aggregation for U0.

Primary metrics (per episode, averaged over the eval corpus) — the
crisis-window metrics are the headline numbers; error_full is kept as a
secondary context metric:

    need_resolution       fraction of *planned* needs resolved by ACTing
                          at a matching site before episode end. A need
                          the agent died before reaching counts as
                          unresolved — early death cannot dodge it.
    crisis_error_auc      per-need homeostatic error integrated from
                          onset to resolution / death / episode end,
                          averaged over planned needs. Steps the agent
                          did not live are forfeited at the error
                          measured at death (same convention as
                          error_full), so dying cannot shrink the
                          integral. This is the U0 primary loss.
    time_to_resolution    per-need steps from onset to resolution,
                          censored at episode end for unresolved needs
    survival_after_need   fraction of planned needs the agent survived
                          (resolved, or still alive at episode end)
    important_retention   fraction of *fired* needs whose matching site
                          was present in memory at the need's onset
    store_precision       fraction of STORE actions written on events
                          whose function matched a planned need
    error_full            [secondary] whole-episode homeostatic error;
                          post-death steps count at the death error

Gate metrics derived in report.py:
    normalized_regret     (learned_auc - oracle_auc) /
                          (no_memory_auc - oracle_auc) on
                          crisis_error_auc. 0.0 = oracle, 1.0 = no_memory.
    U-H3 auc ratio        auc_causal / auc_clean (targeted_erase or
                          donor_shuffle)
    U-H4 mean_abs_delta   mean |P(store|func, adverse) - P(store|func, safe)|
"""

from __future__ import annotations

NUMERIC_METRICS = [
    "error_full", "mean_error", "survival_fraction", "stable_fraction",
    "need_resolution", "crisis_error_auc", "time_to_resolution",
    "survival_after_need", "important_retention", "store_precision",
    "stores", "stores_functional", "stores_useful", "stores_junk",
    "recalls", "moves", "acts", "memory_occupied", "needs_planned",
    "needs_fired", "needs_resolved", "died",
]

GATE_THRESHOLDS = {
    # ---- P0 protocol sanity gates (protocol_check.py) ----
    "P0_oracle_resolution": 0.90,        # oracle must resolve >= 90%
    "P0_no_memory_resolution": 0.40,     # no_memory must stay BELOW 40%
    "P0_oracle_erase_drop": 0.30,        # erased oracle: resolution drop
    # ---- main gates ----
    "U-H1_need_resolution": 0.80,
    "U-H1_normalized_regret": 0.50,
    "U-H2_store_precision": 0.75,
    "U-H2_important_retention": 0.75,
    "U-H3_auc_ratio": 1.30,              # erase OR shuffle
    "U-H3_resolution_drop": 0.20,        # or a clear resolution drop
    "U-H4_mean_abs_delta": 0.20,
    "U-H4b_mediation_gap": 0.50,         # mediation closes >= half the
                                         # clean->no_memory auc gap
    "U-H5_min_seeds": 3,
    # ---- strong-PASS extensions ----
    "strong_delay128_regret": 0.70,
    "strong_distractor4x_precision": 0.70,
    # spec: "adapts to unseen need-resource association". Under a full
    # function->variable rotation the learned association is wrong, so
    # adaptation means still resolving >= half of crises via the
    # trial-and-error fallback plus whatever memory remains useful.
    "strong_need_mapping_shift_resolution": 0.50,
    "strong_capacity2_regret": 0.50,     # as close to oracle as the main
                                         # regret bar, under 2 slots
}
