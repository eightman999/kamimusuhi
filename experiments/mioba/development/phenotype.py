"""Development: map a Genome onto a concrete Phenotype description.

The phenotype is a plain dict (serialisable) consumed by FBA backends.
``ancestry_fraction`` is what the GUI renders as "ancestry %".
"""
from __future__ import annotations

from ..fba.fba0 import FBA0_REFERENCE, fba0_neuron_count
from ..fba.params import resolve_params
from ..genome.schema import Genome
from ..genome.structure import analyse


def develop(genome: Genome, base_neurons: int | None = None) -> dict:
    """`base_neurons`: caller-provided FBA0 population size override (e.g.
    the synthetic-N count when the backend runs in synthetic mode);
    defaults to the real FlyWire v783 count when the data is present.

    Disabled organs (DISABLE_ORGAN, M1 §3.1) and the attachments that
    touch them are not built: a disabled organ stays in the genome for the
    lineage record but costs nothing to simulate. Attachments naming an
    organ that no longer exists are dropped the same way, so a prune can
    never hand the backend a dangling endpoint.
    """
    organs = [o for o in genome.artificial_organs if getattr(o, "enabled",
                                                             True)]
    live = {o.organ_id for o in organs}

    def wired(a) -> bool:
        if not getattr(a, "enabled", True):
            return False
        return all(e.startswith("fba0") or e in live
                   for e in (a.source, a.target))

    attachments = [a for a in genome.attachments if wired(a)]
    n_extra = sum(o.size for o in organs)
    # region/organ-scoped mutations are left to the backend
    params = resolve_params(genome.parameter_mutations)
    base_n = base_neurons if base_neurons is not None else fba0_neuron_count()
    denom = (base_n or 0) + n_extra
    ancestry_fraction = (base_n / denom) if base_n and denom else (1.0 if not n_extra else None)
    return {
        "base": dict(FBA0_REFERENCE),
        "species_base": genome.species_base,
        "genome_id": genome.genome_id,
        "n_extra_neurons": n_extra,
        "artificial_organs": [
            {"organ_id": o.organ_id, "kind": o.kind, "size": o.size,
             "params": o.params}
            for o in organs
        ],
        "attachments": [
            {"attachment_id": a.attachment_id, "source": a.source,
             "target": a.target, "direction": a.direction,
             "weight_scale": a.weight_scale}
            for a in attachments
        ],
        # where each organ sits in the graph (M1 section 4): functional /
        # neutral_structure / invalid_structure / disabled
        "structure": analyse(genome).to_dict(),
        "params": params,
        "ancestry_fraction": ancestry_fraction,
        # part of the individual's own resource cost (M1 §8): a genome
        # that carries more structure costs more to store and ship
        "genome_bytes": len(genome.to_json().encode("utf-8")),
    }


Phenotype = dict
