"""MIE collector plugin registry.

Config example — external sensors (temperature/humidity/light) can be
added without code changes:

    mie:
      collectors:
        - gpu
        - cpu
        - {name: bme280, module: "my_pkg.sensors:BME280Collector",
           params: {i2c_addr: "0x76"}}

Each entry is either a builtin name string or a mapping with ``name``
and optional ``module`` ("pkg.mod:Class") plus ``params`` passed to the
constructor. ``module`` is required for non-builtin collectors.
"""
from __future__ import annotations

import importlib
from typing import Any

from .collectors import BUILTIN, Collector, Normalizer


def load_collectors(config: dict) -> list[Collector]:
    mie_cfg = (config or {}).get("mie", {})
    entries = mie_cfg.get("collectors", ["gpu", "cpu", "ram"])
    normalizer = Normalizer()
    collectors: list[Collector] = []
    for entry in entries:
        if isinstance(entry, str):
            cls = BUILTIN.get(entry)
            if cls is None:
                raise ValueError(f"unknown builtin collector {entry!r}; "
                                 "use {name, module, params} for externals")
            collectors.append(normalizer.wrap(cls()))
            continue
        name = entry.get("name")
        params = entry.get("params") or {}
        if name in BUILTIN and "module" not in entry:
            collectors.append(normalizer.wrap(BUILTIN[name](**params)))
            continue
        module = entry.get("module")
        if not module:
            raise ValueError(f"collector {name!r} needs a 'module' entry")
        mod_name, _, cls_name = module.partition(":")
        cls = getattr(importlib.import_module(mod_name), cls_name)
        collectors.append(normalizer.wrap(cls(**params)))
    return collectors
