"""FBA0 as a substrate implementation.

FBA0 is the FlyWire v783 connectome under the Shiu et al. (2024) LIF +
alpha-synapse model — the ancestral substrate every M-series lineage
descends from. This adapter wraps the existing ``fba/`` reference data
behind :class:`~..substrate.base.SubstrateProtocol` so that development,
mutation and the departure evaluator never touch the FBA modules
directly.

The region names are the synthetic pseudo-region partition
(``region_mode="synthetic-region-v0"``) the torch backend wires
``fba0:<region>`` attachments to; with the real FlyWire dataset no
region mapping exists and the backend refuses regional attachments
rather than wiring them at random.
"""
from __future__ import annotations

import random

from .base import LesionSpec, PortSpec
from ..fba.fba0 import FBA0_REFERENCE, fba0_neuron_count
from ..fba.params import DEFAULT_PARAMS, resolve_params as _resolve

SUBSTRATE_ID = "fba0"

# Canonical region order — part of the deterministic protocol (the
# mutation endpoint pool iterates it). Same names and order as the
# backend's synthetic partition (fba/torch_backend.SYNTHETIC_REGIONS).
FBA0_REGIONS = ("medulla", "lobula", "lobula_plate", "central_complex",
                "mushroom_body", "optic_lobe", "antennal_lobe")

# M1 §13 mutation policy: new organs are wired on an upstream ->
# downstream path so they sit where they can matter. These groups are a
# property of this substrate's known functional anatomy; a generic
# substrate without groups defaults to all ports.
UPSTREAM_PORTS = ("medulla", "lobula", "lobula_plate", "antennal_lobe",
                  "optic_lobe")
DOWNSTREAM_PORTS = ("central_complex", "mushroom_body", "lobula_plate")


class FBA0Adapter:
    """The SubstrateProtocol implementation for the founder substrate."""

    substrate_id = SUBSTRATE_ID
    kind = "flywire-v783-shiu-lif"

    def __init__(self, gene=None):
        # ``gene`` is the genome-layer SubstrateGene record (duck-typed):
        # params on it override nothing yet, but the slot exists so a
        # future substrate variant can be parameterised per genome.
        self.gene = gene

    # ---------------------------------------------------------- identity
    def describe(self) -> dict:
        return {
            "substrate_id": self.substrate_id,
            "kind": self.kind,
            "n_neurons": self.neuron_count(),
            "regions": list(FBA0_REGIONS),
            "ports": [p.to_dict() for p in self.ports()],
            "port_groups": {"upstream": list(UPSTREAM_PORTS),
                            "downstream": list(DOWNSTREAM_PORTS)},
            "reference": self.reference(),
        }

    def reference(self) -> dict:
        """The phenotype ``base`` record — exactly the historical
        FBA0_REFERENCE so the GUI and stored phenotypes are unchanged."""
        return dict(FBA0_REFERENCE)

    # ------------------------------------------------------------- ports
    def ports(self) -> list[PortSpec]:
        return [PortSpec(name=r, kind="region", signal="event",
                         endpoint=f"{SUBSTRATE_ID}:{r}")
                for r in FBA0_REGIONS]

    def regions(self) -> list[str]:
        return list(FBA0_REGIONS)

    def port_groups(self) -> dict:
        return {"upstream": list(UPSTREAM_PORTS),
                "downstream": list(DOWNSTREAM_PORTS)}

    # ----------------------------------------------------------- params
    def neuron_count(self) -> int | None:
        return fba0_neuron_count()

    def default_params(self) -> dict:
        return dict(DEFAULT_PARAMS)

    def resolve_params(self, mutations) -> dict:
        return _resolve(mutations)

    # ----------------------------------------------------------- lesion
    def lesion(self, severity: float, n_neurons: int,
               seed: int) -> LesionSpec:
        """Uniform random ``severity`` fraction of the substrate's
        population, drawn from a dedicated seeded stream. The neuron ids
        are deterministic given (severity, n_neurons, seed)."""
        n = max(0, int(n_neurons))
        k = min(n, max(0, int(round(n * float(severity)))))
        ids = sorted(random.Random(int(seed)).sample(range(n), k))
        return LesionSpec(substrate_id=self.substrate_id,
                          severity=float(severity), neuron_ids=ids)

    # -------------------------------------- adapter-level state (none)
    def reset(self, seed: int) -> None:
        del seed                      # the reference has no mutable state

    def snapshot(self) -> dict:
        return {"substrate_id": self.substrate_id, "kind": self.kind}

    def restore(self, snapshot: dict) -> None:
        if (snapshot or {}).get("substrate_id") != self.substrate_id:
            raise ValueError("not an fba0 substrate snapshot")
