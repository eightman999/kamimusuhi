"""T0-v2 protocol integrity validation.

v2 removes the interpolation holdout bands from the training support, so a
PASS verdict is only meaningful when the train/eval split is provably
clean.  ``validate_split_integrity`` returns a machine-readable verdict;
``require_clean`` turns any overlap into ``ProtocolViolation`` so that
evaluation, consolidation, and report generation fail instead of emitting
a PASS from a contaminated protocol.
"""
import json

import torch

from experiments.t0.env.interval_env import (EXTRAP_DELAYS, INTERP_DELAYS,
                                             INTERP_HOLDOUT, SEEN_DELAYS,
                                             IntervalEnv)

PROTOCOL_VERSION = "T0-v2"
EVAL_SEED = 900001
VALIDATION_SEED = 700001
IMITATION_SEED_BASE = 100000
PPO_SEED_BASE = 200000


class ProtocolViolation(Exception):
    """Raised when a train/eval split violates the T0-v2 protocol."""


def _int_set(values):
    return set(int(v) for v in values)


def training_support(envconfig):
    """The set of delays the env's training-time sampler can produce.

    ``grid`` returns the grid verbatim (an excluded grid entry is a support
    overlap, i.e. INVALID, not a silent removal); continuous distributions
    return the allowed subset of [delay_min, delay_max].
    """
    envconfig = dict(envconfig or {})
    grid = _int_set(envconfig.get("delays", SEEN_DELAYS))
    lo = int(envconfig.get("delay_min", min(grid)))
    hi = int(envconfig.get("delay_max", max(grid)))
    excluded = _int_set(envconfig.get("excluded_training_delays", ()))
    allowed = set(range(lo, hi + 1)) - excluded
    dist = envconfig.get("delay_distribution", "grid")
    if dist == "grid":
        return set(grid)
    if dist in ("uniform", "geometric"):
        return allowed
    if dist == "mixed":
        return set(grid) | allowed
    raise ValueError(f"Unknown delay_distribution: {dist}")


def validation_support(config):
    """Model selection evaluates on ``delay_distribution='grid'``."""
    envconfig = dict((config or {}).get("env", {}))
    return _int_set(envconfig.get("delays", SEEN_DELAYS))


def validate_split_integrity(config):
    """Return the machine-readable protocol verdict for a training config."""
    envconfig = dict((config or {}).get("env", {}))
    train = training_support(envconfig)
    validation = validation_support(config)
    seed = int((config or {}).get("seed", 0))
    seeds = {"imitation_env": IMITATION_SEED_BASE + seed,
             "ppo_env": PPO_SEED_BASE + seed,
             "validation": VALIDATION_SEED, "eval": EVAL_SEED}
    overlaps = {
        "train_interpolation_overlap": sorted(train & _int_set(INTERP_DELAYS)),
        "train_holdout_overlap": sorted(train & _int_set(INTERP_HOLDOUT)),
        "train_extrapolation_overlap": sorted(train & _int_set(EXTRAP_DELAYS)),
        "validation_test_overlap": sorted(
            validation & (_int_set(INTERP_HOLDOUT) | _int_set(EXTRAP_DELAYS)))}
    rng_separated = len(set(seeds.values())) == len(seeds)
    clean = rng_separated and not any(overlaps.values())
    return {"protocol_version": PROTOCOL_VERSION,
            "status": "PASS" if clean else "INVALID_PROTOCOL",
            "training_support": sorted(train),
            "validation_support": sorted(validation),
            "excluded_training_delays": sorted(
                _int_set(envconfig.get("excluded_training_delays", ()))),
            "interpolation_primary": list(INTERP_DELAYS),
            "interpolation_holdout": list(INTERP_HOLDOUT),
            "extrapolation": list(EXTRAP_DELAYS),
            "rng_seeds": seeds,
            "rng_separated": rng_separated,
            **overlaps}


def intervention_target_independence(intervention, delay=32, shift=17,
                                     device="cpu"):
    """True iff an intervention's output is invariant to ``env.target_step``.

    Replays the same env snapshot twice — once with the true target step,
    once with a shifted one — and requires identical intervention outputs.
    An observation intervention that reads ``target_step`` (e.g. releases a
    blank at T*) fails this check.
    """
    env = IntervalEnv(8, device, 31337,
                      {"delays": [delay], "delay_distribution": "grid",
                       "horizon": delay + 24})
    env.reset()
    snap = env.snapshot()

    def run(shift_by):
        env.restore(snap)
        if shift_by:
            env.target_step = env.target_step + shift_by
        observation = env.observation.clone()
        state = torch.zeros(8, 4, device=env.device)
        outs = []
        for t in range(delay + 16):
            state, obs = intervention(t, state, observation, env)
            outs.append(obs.clone())
            observation, _, done, _ = env.step(
                torch.zeros(8, dtype=torch.long, device=env.device))
            if bool(done.all()):
                break
        return torch.stack(outs)

    return bool(torch.equal(run(0), run(shift)))


def protocol_integrity(config, interventions=None, device="cpu"):
    """Split verdict plus target-independence of observation interventions."""
    result = validate_split_integrity(config)
    if interventions:
        independence = {
            name: intervention_target_independence(fn, device=device)
            for name, fn in interventions.items()}
        result["intervention_target_independence"] = independence
        if not all(independence.values()):
            result["status"] = "INVALID_PROTOCOL"
    return result


def require_clean(validation):
    """Raise ``ProtocolViolation`` unless the verdict is PASS."""
    if validation.get("status") != "PASS":
        raise ProtocolViolation(
            "T0-v2 split violation: " + json.dumps(
                {k: v for k, v in validation.items()
                 if k.endswith("overlap") or k == "intervention_target_independence"}))
    return validation
