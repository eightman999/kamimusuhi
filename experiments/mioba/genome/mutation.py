"""Genome mutation operators.

`mutate` produces a child genome that either applies a small parameter
mutation (scale a physiological parameter by ±5%) or, with probability
0.2, adds a tiny artificial organ plus an attachment from a random FBA0
region.

Every random choice *and every identifier* (mutation, organ, attachment)
is drawn from the caller's ``random.Random`` so that the same parent +
RNG state + generation + birth_index yields a byte-identical child and
genome hash; the coordinator persists that RNG state in checkpoints.
"""
from __future__ import annotations

import random

from .schema import (ArtificialOrgan, Attachment, Genome, OrganProvenance,
                     ParameterMutation)

# Parameters a mutation may scale (Shiu et al. 2024 LIF names).
MUTABLE_PARAMS = ("wScale", "tauMem", "tauSyn", "vThr", "tRefrac")
FBA0_REGIONS = ("medulla", "lobula", "lobula_plate", "central_complex",
                "mushroom_body", "optic_lobe", "antennal_lobe")


def _mid(rng: random.Random) -> str:
    """Deterministic 128-bit identifier derived from the mutation RNG."""
    return f"{rng.getrandbits(128):032x}"[:16]


def mutate(parent: Genome, rng: random.Random, birth_index: int,
           generation: int) -> Genome:
    child = Genome.from_dict(parent.to_dict())
    child.genome_id = ""
    child.created_at = ""
    child.parent_ids = [parent.genome_id] if parent.genome_id else []
    child.generation = generation
    child.birth_index = birth_index
    child.random_seed = rng.randrange(2**31)

    mutation_id = _mid(rng)
    prov = parent.provenance(birth_mutation_id=mutation_id)

    if rng.random() < 0.2:
        organ = ArtificialOrgan(
            organ_id=f"org_{_mid(rng)[:8]}",
            kind="lif_cluster",
            size=rng.randint(8, 64),
            params={},
            provenance=prov,
        )
        child.artificial_organs.append(organ)
        child.attachments.append(Attachment(
            attachment_id=f"att_{_mid(rng)[:8]}",
            source=f"fba0:{rng.choice(FBA0_REGIONS)}",
            target=organ.organ_id,
            direction="forward",
            weight_scale=round(rng.uniform(0.5, 1.5), 4),
            provenance=prov,
        ))
    else:
        path = rng.choice(MUTABLE_PARAMS)
        child.parameter_mutations.append(ParameterMutation(
            mutation_id=mutation_id,
            path=path,
            op="scale",
            value=round(rng.uniform(0.95, 1.05), 4),
            scope="global",
        ))
    return child.finalize()
