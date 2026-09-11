"""Causal ablation battery (spec section 13).

Runs each ablation against a trained checkpoint and reports the performance
delta vs the un-ablated baseline:

    C1 shuffle        internal channels replaced by donor-episode states
    C2 mask           each internal channel fixed to 0.5, one at a time
    C3 permute        observation channels permuted
    C4 action_perm    REST<->EXPLORE physical effects swapped (via EnvParams)
    hidden_reset      GRU hidden state zeroed mid-episode
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Dict, Optional

import numpy as np

from ..config import Config
from ..env import dynamics as dyn
from ..evaluate import evaluate


def run_causal_battery(
    cfg: Config,
    policy_factory,
    n_episodes: int = 12,
    seed: int = 0,
) -> Dict:
    """policy_factory() -> fresh policy (needed because ablations may need
    independent hidden state). Returns {ablation_name: aggregate}."""
    results: Dict = {}

    base = evaluate(cfg, policy_factory(), n_episodes, seed=seed)
    results["none"] = base["aggregate"]

    # C1 internal shuffle
    r = evaluate(cfg, policy_factory(), n_episodes, seed=seed,
                 ablation={"type": "shuffle"})
    results["shuffle"] = r["aggregate"]

    # C2 internal mask, per channel
    for i, name in enumerate(dyn.INTERNAL_NAMES):
        r = evaluate(cfg, policy_factory(), n_episodes, seed=seed,
                     ablation={"type": "mask", "index": i, "value": 0.5})
        results[f"mask_{name}"] = r["aggregate"]

    # C3 sensor permutation
    rng = np.random.default_rng(123)
    perm = rng.permutation(dyn.OBS_DIM).tolist()
    r = evaluate(cfg, policy_factory(), n_episodes, seed=seed,
                 ablation={"type": "permute", "permutation": perm})
    results["permute"] = r["aggregate"]

    # C4 action-effect permutation: swap REST and EXPLORE physical effects
    perm_act = list(range(dyn.N_ACTIONS))
    perm_act[dyn.REST], perm_act[dyn.EXPLORE] = dyn.EXPLORE, dyn.REST
    cfg_c4 = Config(**{**vars(cfg)})
    cfg_c4.env = cfg.env.with_overrides(action_permutation=tuple(perm_act))
    r = evaluate(cfg_c4, policy_factory(), n_episodes, seed=seed)
    results["action_perm_rest_explore"] = r["aggregate"]

    # hidden reset mid-episode (only meaningful for recurrent policies)
    r = evaluate(cfg, policy_factory(), n_episodes, seed=seed,
                 ablation={"type": "hidden_reset", "at": 0.5})
    results["hidden_reset"] = r["aggregate"]

    return results


def summarize_causal(results: Dict) -> Dict:
    """Extract the key deltas used for PASS-3 / masking analysis."""
    base_err = results["none"]["homeostatic_error_full"]["mean"]
    base_surv = results["none"]["survival_fraction"]["mean"]
    out = {"baseline": {"error_full": base_err, "survival": base_surv}}
    for k, agg in results.items():
        if k == "none":
            continue
        out[k] = {
            "error_full": agg["homeostatic_error_full"]["mean"],
            "delta_error": agg["homeostatic_error_full"]["mean"] - base_err,
            "survival": agg["survival_fraction"]["mean"],
            "delta_survival": agg["survival_fraction"]["mean"] - base_surv,
        }
    return out
