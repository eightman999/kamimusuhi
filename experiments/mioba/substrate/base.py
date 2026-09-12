"""The substrate contract.

A substrate is the inherited tissue an organism is built on: it has
identity (``substrate_id`` + ``kind``), attachable ports (regions or
named endpoints), a population size, its own parameter defaults, and a
lesion interface the functional-departure evaluator uses.

This is deliberately a *small* contract. The substrate adapter is not
the simulator: ``reset``/``snapshot``/``restore`` cover adapter-level
description state (lesion masks, port availability), while the voltage
and spike state of a running network stay behind the backend's own
``checkpoint``/``restore``. FBA backends keep their own interface
(``fba/backend.py``) because a backend simulates the whole phenotype —
substrate *and* organs — in one population.

Interface vocabulary stays substrate-neutral: ``ports()``,
``regions()``, ``lesion()``, ``resource_summary()`` — never
``flywire_region()`` or ``fba_neuron()``.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from typing import Protocol, runtime_checkable

#: Signal disciplines a port may carry. M1 attachments are all
#: ``event`` ports (neural spike events); ``continuous`` and the generic
#: ``signal`` exist so transducers/effectors can be typed without a
#: type-system redesign.
PORT_SIGNALS = ("event", "continuous", "signal")


@dataclass(frozen=True)
class PortSpec:
    """One attachable port on a substrate (or organ)."""
    name: str
    kind: str = "region"            # region | input | output | exchange
    signal: str = "event"           # see PORT_SIGNALS
    endpoint: str = ""              # stored spelling, e.g. "fba0:medulla"

    def to_dict(self) -> dict:
        return {"name": self.name, "kind": self.kind,
                "signal": self.signal, "endpoint": self.endpoint}


@dataclass
class LesionSpec:
    """A deterministic substrate lesion: ``neuron_ids`` are indices into
    the substrate's population that lose the ability to emit spikes.

    ``severity`` is the fraction of the substrate population lesioned;
    ``sham`` marks the control lesion that applies the selection
    procedure but silences nothing. ``region`` scopes the lesion when
    the substrate supports regional targeting.
    """
    substrate_id: str
    severity: float
    neuron_ids: list[int] = field(default_factory=list)
    region: str | None = None
    sham: bool = False

    def to_dict(self) -> dict:
        return {"substrate_id": self.substrate_id,
                "severity": self.severity,
                "n_lesioned": len(self.neuron_ids),
                "region": self.region, "sham": self.sham}


@runtime_checkable
class SubstrateProtocol(Protocol):
    """What development, mutation and the departure evaluator may ask
    of a substrate implementation."""

    substrate_id: str
    kind: str

    def describe(self) -> dict:
        """Serialisable identity + capability description, embedded in
        the phenotype's ``substrates`` list."""
        ...

    def reference(self) -> dict:
        """The organism's ``base`` record (phenotype backward compat)."""
        ...

    def ports(self) -> list[PortSpec]:
        """Every port an attachment may name."""
        ...

    def regions(self) -> list[str]:
        """Port names in canonical order (the order mutation draws
        endpoints in — it is part of the deterministic protocol)."""
        ...

    def port_groups(self) -> dict:
        """Named port subsets mutation policy uses (e.g. upstream /
        downstream). Unknown for substrates that do not define them."""
        ...

    def neuron_count(self) -> int | None:
        """Population size, or None when unknown until the backend loads
        the dataset."""
        ...

    def default_params(self) -> dict:
        """This substrate's simulator parameter defaults."""
        ...

    def resolve_params(self, mutations) -> dict:
        """Parameter mutations resolved over this substrate's defaults."""
        ...

    def lesion(self, severity: float, n_neurons: int,
               seed: int) -> LesionSpec:
        """Choose the neurons a ``severity`` lesion silences, from a
        dedicated seeded stream (never a shared RNG)."""
        ...

    # -- adapter-level state (NOT simulator state) --
    def reset(self, seed: int) -> None: ...
    def snapshot(self) -> dict: ...
    def restore(self, snapshot: dict) -> None: ...


def check_substrate(obj) -> list[str]:
    """Duck-type conformance check used by tests: the members of
    SubstrateProtocol an adapter must provide."""
    missing = []
    for name in ("substrate_id", "kind", "describe", "reference",
                 "ports", "regions", "port_groups", "neuron_count",
                 "default_params", "resolve_params", "lesion",
                 "reset", "snapshot", "restore"):
        if not hasattr(obj, name):
            missing.append(name)
    return missing
