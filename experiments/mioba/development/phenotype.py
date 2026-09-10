"""Development: map a Genome onto a concrete Phenotype description.

The phenotype is a plain dict (serialisable) consumed by FBA backends.
``ancestry_fraction`` is what the GUI renders as "ancestry %".
"""
from __future__ import annotations

from ..fba.fba0 import FBA0_REFERENCE, fba0_neuron_count
from ..genome.schema import Genome


def develop(genome: Genome) -> dict:
    n_extra = sum(o.size for o in genome.artificial_organs)
    overrides: dict[str, float] = {}
    for m in genome.parameter_mutations:
        if m.scope != "global":
            continue  # region/organ-scoped overrides applied by the backend
        cur = overrides.get(m.path)
        if m.op == "scale":
            overrides[m.path] = (cur if cur is not None else 1.0) * m.value
        elif m.op == "add":
            overrides[m.path] = (cur or 0.0) + m.value
        else:
            overrides[m.path] = m.value
    base_n = fba0_neuron_count()
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
            for o in genome.artificial_organs
        ],
        "attachments": [
            {"attachment_id": a.attachment_id, "source": a.source,
             "target": a.target, "weight_scale": a.weight_scale}
            for a in genome.attachments
        ],
        "param_overrides": overrides,
        "ancestry_fraction": ancestry_fraction,
    }


Phenotype = dict
