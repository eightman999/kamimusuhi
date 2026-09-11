"""Deterministic genome content hashing.

Hash algorithm: BLAKE2b (stdlib, deterministic across processes and
platforms) over canonical JSON of the genome without ``genome_id`` and
``created_at``. The ``b2b:`` prefix is a version tag that allows a later
migration to BLAKE3 (``b3:``) without ambiguity.
"""
from __future__ import annotations

import hashlib
import json
from dataclasses import asdict, is_dataclass


def _plain(obj):
    if is_dataclass(obj) and not isinstance(obj, type):
        return {k: _plain(v) for k, v in asdict(obj).items()}
    if isinstance(obj, dict):
        return {k: _plain(v) for k, v in obj.items()}
    if isinstance(obj, (list, tuple)):
        return [_plain(v) for v in obj]
    return obj


def canonical_json(obj) -> str:
    return json.dumps(_plain(obj), sort_keys=True, separators=(",", ":"),
                      ensure_ascii=True)


def genome_hash(genome) -> str:
    data = _plain(genome)
    data.pop("genome_id", None)
    data.pop("created_at", None)
    digest = hashlib.blake2b(canonical_json(data).encode("utf-8"),
                             digest_size=32).hexdigest()
    return "b2b:" + digest


def config_hash(config: dict) -> str:
    return hashlib.sha256(canonical_json(config).encode("utf-8")).hexdigest()


# Config sections whose change alters the research result (same experiment
# id must not silently continue). Everything else is operational: GUI,
# heartbeat/checkpoint intervals, stop timeout, MIE collectors, traces, ...
SCIENTIFIC_CONFIG_KEYS = ("population", "evolution", "evaluation", "fba",
                          "env", "fitness", "environment", "dataset")
# fba.data_dir is a filesystem path, not a research identity
_SCIENTIFIC_EXCLUDE = {("fba", "data_dir")}


def split_config(config: dict) -> tuple[dict, dict]:
    sci, run = {}, {}
    for k, v in config.items():
        if k in SCIENTIFIC_CONFIG_KEYS:
            if isinstance(v, dict):
                v = {kk: vv for kk, vv in v.items()
                     if (k, kk) not in _SCIENTIFIC_EXCLUDE}
            sci[k] = v
        else:
            run[k] = v
    return sci, run


def scientific_config_hash(config: dict) -> str:
    """Hash of the result-affecting config *plus the simulator's own
    semantics*.

    The config alone is not enough to identify a scientific condition:
    two runs with byte-identical YAML produce different trajectories if
    the delay line, the RNG protocol or the propagation path changed
    underneath them. Folding ``fba/semantics.py`` in means such a change
    forces a new experiment id through the existing resume check, instead
    of silently continuing an M0 experiment with M1 equations.
    """
    from ..fba.semantics import semantics

    sci = split_config(config)[0]
    prop = ((config.get("fba") or {}).get("propagation_backend")
            or (config.get("evaluation") or {}).get("propagation_backend"))
    payload = dict(sci)
    payload["_simulator"] = semantics(prop) if prop else semantics()
    return config_hash(payload)


def runtime_config_hash(config: dict) -> str:
    return config_hash(split_config(config)[1])
