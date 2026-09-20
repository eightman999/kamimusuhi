"""Provenance / source-recovery checks.

* ``pointer_rate``     — fraction of claim-spans carrying at least one
                         derived_from id that resolves to a real event
* ``source_recovery``  — fraction whose resolved source claim
                         canonically supports the context claim
* ``reconstructable``  — fraction of context claims for which the
                         original source *text* is retrievable through
                         the pointer chain (strong-PASS check: source
                         reconstruction is possible)
"""

from __future__ import annotations

from typing import Dict

from ..context_builder.bundle import ContextBundle
from ..store.immutable_store import ImmutableEventStore


def source_recovery(
    bundle: ContextBundle, store: ImmutableEventStore
) -> Dict[str, float]:
    total = ptr_ok = supported = reconstructable = 0
    for span in bundle.claims():
        total += 1
        ids = [
            eid for eid in span.prov.derived_from if store.get(eid)
        ]
        if ids:
            ptr_ok += 1
            if any(
                store.get(eid).claim
                and store.get(eid).claim.canonical()
                == span.claim.canonical()
                for eid in ids
            ):
                supported += 1
            # original text still retrievable through the pointer
            if any(store.get(eid).text for eid in ids):
                reconstructable += 1
    return {
        "pointer_rate": ptr_ok / total if total else 0.0,
        "source_recovery": supported / total if total else 0.0,
        "reconstructable": reconstructable / total if total else 0.0,
    }
