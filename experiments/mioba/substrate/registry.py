"""Substrate registry: substrate_id -> adapter.

Development, mutation and the departure evaluator resolve substrate
genes through the registry; the only ``fba`` import in the architecture
lives inside the adapter itself. ``default_registry()`` is a lazy
process-wide registry — resolving adapters is a dict lookup done once
per genome (development / job setup), never in a simulation hot loop.
"""
from __future__ import annotations

from dataclasses import dataclass, field

from .endpoints import FOUNDER_SUBSTRATE


class UnknownSubstrate(KeyError):
    """A genome named a substrate id no registered adapter provides."""


class SubstrateRegistry:
    def __init__(self):
        self._factories: dict[str, object] = {}

    def register(self, substrate_id: str, factory) -> None:
        """``factory`` is ``callable(gene) -> adapter`` (an adapter class
        or plain function)."""
        self._factories[str(substrate_id)] = factory

    def known(self) -> list[str]:
        return sorted(self._factories)

    def get(self, substrate_id: str, gene=None):
        factory = self._factories.get(str(substrate_id))
        if factory is None:
            raise UnknownSubstrate(
                f"substrate {substrate_id!r}: no adapter registered")
        return factory(gene)


def substrate_ids_of(genome) -> list[str]:
    """The enabled substrate ids a genome carries.

    A genome with no ``substrates`` list (a legacy v1-v3 record, or a
    programmatically built Genome) means the ancestral FBA0 substrate —
    "empty" is the M1 condition, spelled out by schema v4 migration for
    stored documents.
    """
    genes = [s for s in (getattr(genome, "substrates", None) or [])
             if getattr(s, "enabled", True)]
    return [s.substrate_id for s in genes] or [FOUNDER_SUBSTRATE]


def substrate_genes_of(genome) -> list:
    """Enabled substrate gene records; the implicit FBA0 gene when the
    genome carries none (see ``substrate_ids_of``)."""
    genes = [s for s in (getattr(genome, "substrates", None) or [])
             if getattr(s, "enabled", True)]
    if genes:
        return genes
    return [_ImplicitSubstrateGene()]


@dataclass
class _ImplicitSubstrateGene:
    """Duck-type stand-in for a genome SubstrateGene when a record
    carries no substrate genes (legacy v1-v3 documents)."""
    substrate_id: str = FOUNDER_SUBSTRATE
    kind: str = "flywire-v783-shiu-lif"
    enabled: bool = True
    params: dict = field(default_factory=dict)


_DEFAULT: SubstrateRegistry | None = None


def default_registry() -> SubstrateRegistry:
    global _DEFAULT
    if _DEFAULT is None:
        from .fba0 import FBA0Adapter
        reg = SubstrateRegistry()
        reg.register(FBA0Adapter.substrate_id, FBA0Adapter)
        _DEFAULT = reg
    return _DEFAULT


def adapter_for(gene, registry: SubstrateRegistry | None = None):
    """Instantiate the adapter for one substrate gene record."""
    return (registry or default_registry()).get(
        getattr(gene, "substrate_id"), gene)


def substrate_disabled_regions(genome) -> dict[str, set[str]]:
    """``{substrate_id: {region names}}`` the genome's own substrate
    genes have disabled (the M3 staged ``DISABLE_SUBSTRATE_REGION``
    operator writes ``gene.params["disabled_regions"]``).

    This is *declared* state, not adapter state: port existence stays the
    backend's contract (an unknown region raises at initialise), while a
    genome-disabled region is a lesion — its endpoints are dangling.
    """
    out: dict[str, set[str]] = {}
    for gene in substrate_genes_of(genome):
        out[gene.substrate_id] = set(
            (getattr(gene, "params", None) or {})
            .get("disabled_regions") or [])
    return out
