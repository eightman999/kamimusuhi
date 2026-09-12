"""Genome -> phenotype: what actually gets simulated (M1 §13, M2).

Development resolves the genome's substrate genes through the substrate
registry — ``development`` does not know what FBA0 is; it knows the
organism's enabled substrates. For M-series genomes that is the single
ancestral FBA0 substrate (declared explicitly in schema v4, implied by
an empty ``substrates`` list in legacy records), so every M1 phenotype
field keeps its meaning:

``base``
    the primary substrate's reference record (``FBA0_REFERENCE`` for
    FBA0 genomes — unchanged).
``substrates``
    every enabled substrate's descriptor (M2; additive).
``artificial_organs`` / ``attachments``
    the organ IR the backends simulate: organs that are enabled, and
    attachments whose endpoints all resolve — a live organ, an enabled
    substrate endpoint, or a habitat interface (``env:/sensor:/
    effector:``; no current backend can wire those, so a genome that
    names them fails loudly at initialise rather than wiring at
    random). Attachments that name something the organism does not have
    are dropped: the genome keeps them as dead code, the phenotype does
    not lie about them. ``structure`` is the section-4/M2
    classification of the *genome's* graph (disabled organs and dangling
    attachments included — they are part of the structural record).
``params``
    the primary substrate's parameter defaults with the genome's
    ``parameter_mutations`` applied — organ- and region-scoped
    mutations are resolved by the backend, which receives the same
    filtered organ list.
``ancestry_fraction``
    primary-substrate neurons / (substrate + artificial) — the share of
    the simulated body that is inherited tissue. ``genome_bytes``
    prices the genome itself (§8): the genome is what reproduction
    copies, so its cost is its length.

M2 adds ``structural_ancestry_fraction`` (same number, explicit name)
and ``fba0_structural_fraction`` (the FBA0 share specifically) so the
structural share is never confused with *functional dependence* — that
is measured by the departure evaluator, not by counting neurons.

``base_neurons``: caller-provided population-size override for the
primary substrate (e.g. the synthetic-N count when the backend runs in
synthetic mode); defaults to the substrate's own ``neuron_count()``
(the real FlyWire v783 count for FBA0 when the data is present).
"""
from __future__ import annotations

from ..genome.organ_ir import attachment_signal, organ_ports
from ..genome.structure import TOPOLOGY_M1_FBA0_LOOP, analyse
from .lifetime import lifetime_rules_of
from .rules import apply_development_rules
from ..substrate.endpoints import parse_endpoint
from ..substrate.registry import (adapter_for, default_registry,
                                  substrate_genes_of)

#: Endpoint kinds that terminate paths at the habitat boundary.
_HABITAT_KINDS = ("env", "sensor", "effector")


def develop(genome, base_neurons: int | None = None,
            topology_mode: str = TOPOLOGY_M1_FBA0_LOOP,
            registry=None) -> dict:
    reg = registry or default_registry()
    genes = substrate_genes_of(genome)
    adapters = [adapter_for(g, reg) for g in genes]
    primary = adapters[0]

    # Disabled organs (DISABLE_ORGAN, M1 §3.1) and the attachments that
    # touch them are not built: a disabled organ stays in the genome for
    # the lineage record but costs nothing to simulate.
    organs = [o for o in genome.artificial_organs
              if getattr(o, "enabled", True)]
    enabled_atts = [a for a in genome.attachments
                    if getattr(a, "enabled", True)]

    # M2 development rules: deterministic, heritable, applied in order.
    # The body the genome describes may grow beyond its literal gene
    # list — genome != mature phenotype.
    dev = apply_development_rules(genome, organs, enabled_atts, adapters)
    organs = dev["organs"]
    enabled_atts = dev["attachments"]
    live = {o.organ_id for o in organs}

    substrate_ids = {a.substrate_id for a in adapters}
    disabled = {g.substrate_id: set(
        (getattr(g, "params", None) or {}).get("disabled_regions") or [])
        for g in genes}

    def wireable(endpoint) -> bool:
        ref = parse_endpoint(endpoint)
        if ref.kind == "organ":
            return ref.id in live
        if ref.is_substrate():
            if ref.id not in substrate_ids:
                return False
            # port existence is the backend's contract (it raises
            # UnsupportedAttachmentRegion rather than wiring at random);
            # a *genome-disabled* region is a lesion and is dropped
            return (ref.port is None
                    or ref.port not in disabled.get(ref.id, ()))
        return ref.kind in _HABITAT_KINDS

    attachments = [a for a in enabled_atts if wireable(a.source)
                   and wireable(a.target)]
    n_extra = sum(int(o.size) for o in organs)
    # region/organ-scoped mutations are left to the backend
    params = primary.resolve_params(genome.parameter_mutations)

    # per-substrate neuron counts; ``base_neurons`` overrides the primary
    substrate_neurons = {}
    for i, (gene, adapter) in enumerate(zip(genes, adapters)):
        n = adapter.neuron_count()
        if i == 0 and base_neurons is not None:
            n = int(base_neurons)
        if n is None:
            n = int((getattr(gene, "params", None) or {}).get(
                "n_neurons", 0))
        substrate_neurons[adapter.substrate_id] = int(n)
    base_n = substrate_neurons.get(primary.substrate_id) or None
    denom = sum(substrate_neurons.values()) + n_extra
    ancestry = (base_n / denom) if base_n and denom else \
        (1.0 if not n_extra else None)
    fba0_n = substrate_neurons.get("fba0")

    return {
        "base": primary.reference(),
        "substrates": [a.describe() for a in adapters],
        "substrate_neurons": substrate_neurons,
        "species_base": genome.species_base,
        "genome_id": genome.genome_id,
        "n_extra_neurons": n_extra,
        "artificial_organs": [
            # the organ IR the backends simulate: lif_cluster is the
            # backward-compatible special case; ports/internal/state
            # carry the M2 IR when declared (schema v4 fields)
            {"organ_id": o.organ_id, "kind": o.kind, "size": o.size,
             "params": o.params, "ports": organ_ports(o),
             "internal": dict(getattr(o, "internal", None) or {}),
             "state": dict(getattr(o, "state", None) or {})}
            for o in organs
        ],
        "attachments": [
            {"attachment_id": a.attachment_id, "source": a.source,
             "target": a.target, "direction": a.direction,
             "weight_scale": a.weight_scale,
             "signal": attachment_signal(a)}
            for a in attachments
        ],
        # where each organ sits in the graph (M1 §4 / M2 generic):
        # functional / neutral_structure / invalid_structure / disabled.
        # The report classifies the *developed* body — rule-grown sizes
        # and rule-added organs are real tissue — while keeping the
        # genome's disabled organs and dangling attachments in the
        # record. (dev["organs"] are copies: the genome itself is never
        # rewritten by development.)
        "structure": analyse(
            genome, topology_mode,
            _developed=([o for o in genome.artificial_organs
                         if not getattr(o, "enabled", True)]
                        + dev["organs"],
                        list(genome.attachments)
                        + dev["added_attachments"])).to_dict(),
        # which development rules ran (M2 §13): the genome's rules are
        # heritable; their effects are this individual's body
        "development": dev["report"],
        # lifetime plasticity contract (M2 §14-15): the *rules* are
        # heritable; the state they produce is per-individual runtime
        # state in LifetimeState — never in the genome, never inherited
        "plasticity": {"rules": lifetime_rules_of(genome)},
        "params": params,
        "ancestry_fraction": ancestry,
        "structural_ancestry_fraction": ancestry,
        "fba0_structural_fraction": ((fba0_n / denom)
                                   if fba0_n is not None and denom
                                   else None),
        "topology_mode": topology_mode,
        # part of the individual's own resource cost (M1 §8): a genome
        # that carries more structure costs more to store and ship
        "genome_bytes": len(genome.to_json().encode("utf-8")),
    }


Phenotype = dict
