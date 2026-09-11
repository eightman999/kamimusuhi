"""Evaluation environments: map environment_id -> InputDrive.

The E-adapter can register richer environments later via
``register_environment``.
"""
from __future__ import annotations

_ENVS: dict[str, callable] = {}


def register_environment(env_id: str, factory):
    _ENVS[env_id] = factory


def make_drive(env_id: str, n_neurons: int, config: dict) -> dict:
    if env_id not in _ENVS:
        raise KeyError(f"unknown environment {env_id!r}")
    return _ENVS[env_id](n_neurons, config)


def _synthetic_quiet(n_neurons: int, config: dict) -> dict:
    env_cfg = (config or {}).get("env", {})
    frac = float(env_cfg.get("stim_fraction", 0.01))
    rate = float(env_cfg.get("stim_rate_hz", 50.0))
    n_stim = max(1, int(round(n_neurons * frac)))
    return {"rates_hz": {f"slice:0-{n_stim}": rate}, "silence": []}


register_environment("synthetic-quiet-v0", _synthetic_quiet)


def known_environments() -> list[str]:
    return sorted(_ENVS)
