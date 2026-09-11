"""Topology identity and residency (M1 §2.1).

M0 rebuilt the whole network for every evaluation: 14M FBA0 edges were
re-sampled, coalesced and converted to CSR per genome, then thrown away.
Two things make that unnecessary:

1. **The FBA0 base is a reference, not a trait.** In M0 the synthetic
   base was sampled from the *genome's* ``random_seed``, so every
   individual lived in a different pseudo-connectome. M1 samples it from
   ``fba.base_seed`` — one immutable base shared by the whole run, which
   is what ``FBA0_BASE_NAME`` claimed all along. The base seed is part of
   the dataset identity (``v1-n…-e…-s<seed>``) and therefore of the
   scientific record.

2. **Only the artificial part is per-individual.** Organ-internal and
   attachment edges touch at most ``artificial_neurons`` rows/columns,
   which M1 caps at 512. They are built per genome and kept in a separate
   small sparse matrix; the base stays resident on the GPU.

``base_topology_key`` identifies the resident base, ``organ_topology_hash``
identifies the per-individual artificial part so that repeated
structural genomes (siblings sharing a structural mutation, re-evaluated
champions, ablations) reuse a built graph instead of rebuilding it.

Nothing here changes the simulated equations; it changes *when* the graph
is built. The resulting propagation is a sum of two sparse products
instead of one, so floating-point summation order differs from a single
coalesced matrix — the same class of difference as CUDA sparse atomics,
and M1 is a new ``scientific_config_hash`` regardless.
"""
from __future__ import annotations

import hashlib
import json
import threading
from collections import OrderedDict


def _h(obj) -> str:
    return hashlib.blake2b(
        json.dumps(obj, sort_keys=True, separators=(",", ":")).encode(),
        digest_size=16).hexdigest()


def base_topology_key(dataset_identity: dict, n_base: int, base_seed: int,
                      connectivity: float | None,
                      synthetic_edges: int | None, device: str) -> str:
    """Identity of the resident FBA0 base graph on one device."""
    return _h({
        "dataset": {k: dataset_identity.get(k)
                    for k in ("dataset_id", "version", "manifest_hash")},
        "n_base": int(n_base),
        "base_seed": int(base_seed),
        "connectivity": (None if connectivity is None else float(connectivity)),
        "synthetic_edges": (None if synthetic_edges is None
                            else int(synthetic_edges)),
        "device": str(device),
    })


def organ_topology_hash(phenotype: dict, seed: int) -> str:
    """Identity of the per-individual artificial graph (organs +
    attachments + the seed their wiring is drawn from).

    Parameter-only mutations do not change it — that is the point: a
    child that only rescales ``vThr`` reuses its parent's built graph.
    Weight *scales* are excluded because they are applied as multipliers
    at build time on the same sampled index set, and included in
    ``organ_value_key`` instead.
    """
    organs = [{"organ_id": o.get("organ_id"), "size": int(o.get("size") or 0),
               "kind": o.get("kind")}
              for o in (phenotype.get("artificial_organs") or [])]
    atts = [{"attachment_id": a.get("attachment_id"), "source": a.get("source"),
             "target": a.get("target"),
             "direction": a.get("direction", "forward")}
            for a in (phenotype.get("attachments") or [])]
    return _h({"organs": organs, "attachments": atts, "seed": int(seed)})


class TopologyCache:
    """Small LRU of built graphs, keyed by topology hash.

    Process-level: a worker keeps the base resident for its whole life and
    a handful of recent artificial graphs. Entries hold device tensors, so
    ``maxsize`` is a VRAM decision, not a speed one.
    """

    def __init__(self, maxsize: int = 4):
        self.maxsize = int(maxsize)
        self._items: OrderedDict[str, object] = OrderedDict()
        # concurrent evaluation slots (M1 §2.2) share one process cache
        self._lock = threading.Lock()
        self.hits = 0
        self.misses = 0

    def get(self, key: str):
        with self._lock:
            if key in self._items:
                self._items.move_to_end(key)
                self.hits += 1
                return self._items[key]
            self.misses += 1
            return None

    def put(self, key: str, value) -> None:
        with self._lock:
            self._items[key] = value
            self._items.move_to_end(key)
            while len(self._items) > self.maxsize:
                self._items.popitem(last=False)

    def clear(self) -> None:
        with self._lock:
            self._items.clear()

    def stats(self) -> dict:
        total = self.hits + self.misses
        return {"entries": len(self._items), "maxsize": self.maxsize,
                "hits": self.hits, "misses": self.misses,
                "hit_rate": (round(self.hits / total, 4) if total else None)}

    def __len__(self) -> int:
        return len(self._items)


# One cache per worker process. The base graph lives here between jobs.
BASE_CACHE = TopologyCache(maxsize=2)
ORGAN_CACHE = TopologyCache(maxsize=8)
