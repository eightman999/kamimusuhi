"""On-disk persistence artifact for the C0 process boundary.

Schema (JSON):

    {"format": "c0-state-v1",
     "agent": "gru64",                  # model name (arch tag)
     "condition": "hidden",            # cold|hidden|memory|compressed|full
     "budget_bytes": 64,               # compressed only, else null
     "config_fingerprint": "...",      # hash of env config (not episode)
     "items": [{"payload_b64": "...", "nbytes": N}, ...]}

Shortcut guarantees enforced here:

    * no episode id / seed is stored -- items are positionally matched to
      the episode list supplied by the orchestrator (matching is harness
      bookkeeping; the wrong-state causal test shuffles it)
    * no answer, key->value binding label, or context token is stored --
      payloads are opaque state bytes and the only allowed top-level keys
      are ARTIFACT_KEYS
"""
from __future__ import annotations

import base64
import hashlib
import json
from pathlib import Path

import numpy as np

ARTIFACT_FORMAT = "c0-state-v1"
ARTIFACT_KEYS = {"format", "agent", "condition", "budget_bytes",
                 "config_fingerprint", "items"}
ITEM_KEYS = {"payload_b64", "nbytes"}
CONDITIONS = ("cold", "hidden", "memory", "compressed", "full")


def config_fingerprint(env_cfg_dict: dict) -> str:
    blob = json.dumps(env_cfg_dict, sort_keys=True, default=str).encode()
    return hashlib.sha256(blob).hexdigest()[:16]


def write_artifact(path, *, agent: str, condition: str,
                   payloads: list[bytes], budget: int | None = None,
                   env_cfg_dict: dict | None = None) -> dict:
    assert condition in CONDITIONS
    items = [{"payload_b64": base64.b64encode(p).decode(),
              "nbytes": len(p)} for p in payloads]
    doc = {"format": ARTIFACT_FORMAT, "agent": agent,
           "condition": condition,
           "budget_bytes": budget if condition == "compressed" else None,
           "config_fingerprint":
               config_fingerprint(env_cfg_dict) if env_cfg_dict else None,
           "items": items}
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(doc))
    tmp.replace(path)
    return doc


def read_artifact(path) -> tuple[dict, list[bytes]]:
    doc = json.loads(Path(path).read_text())
    assert set(doc.keys()) <= ARTIFACT_KEYS, f"bad keys {set(doc.keys())}"
    assert doc["format"] == ARTIFACT_FORMAT
    payloads = []
    for it in doc["items"]:
        assert set(it.keys()) <= ITEM_KEYS
        payloads.append(base64.b64decode(it["payload_b64"]))
    return doc, payloads
