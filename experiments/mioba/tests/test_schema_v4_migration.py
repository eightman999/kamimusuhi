"""Schema v4: substrate genes become explicit, legacy documents migrate,
and historical identities are never recomputed.

The migration contract:

- a v1-v3 document (no ``substrates`` field) loads with the implicit
  ancestral FBA0 substrate spelled out, ``source_schema_version``
  recording what it was authored as, and its stored ``genome_id``
  preserved verbatim;
- a v4 document round-trips exactly;
- children born under v4 are v4 records — they do not inherit the
  parent's migration provenance;
- an organism's substrate list is a real gene: it is hashed, inherited
  through ``mutate``, and selectable.
"""
from __future__ import annotations

import random

from experiments.mioba.development.phenotype import develop
from experiments.mioba.genome.mutation import mutate
from experiments.mioba.genome.schema import (SCHEMA_VERSION, Genome,
                                             SubstrateGene, fba0_genome)


def _legacy_doc(schema_version: int) -> dict:
    return {
        "parent_ids": [], "generation": 0, "birth_index": 0,
        "random_seed": 11, "genome_id": "b2b:historical-founder",
        "schema_version": schema_version, "species_base": "fba0",
        "ancestral_base": "flywire-v783-shiu-lif",
        "artificial_organs": [], "attachments": [],
        "parameter_mutations": [], "development_rules": [],
        "plasticity_rules": [], "created_at": "2026-01-01T00:00:00+00:00",
    }


def test_schema_version_is_four():
    assert SCHEMA_VERSION == 4


def test_v1_v3_load_gains_the_implicit_fba0_substrate():
    for version in (1, 2, 3):
        g = Genome.from_dict(_legacy_doc(version))
        assert [s.substrate_id for s in g.substrates] == ["fba0"]
        assert g.substrates[0].kind == "flywire-v783-shiu-lif"
        assert g.substrates[0].enabled
        assert g.source_schema_version == version
        assert g.schema_version == 4
        # historical identity is never recomputed on migration
        assert g.genome_id == "b2b:historical-founder"


def test_v4_document_round_trips_without_remigration():
    g = fba0_genome(seed=3)
    assert g.schema_version == 4
    assert [s.substrate_id for s in g.substrates] == ["fba0"]
    assert g.source_schema_version is None
    again = Genome.from_json(g.to_json())
    assert again.to_dict() == g.to_dict()
    assert again.source_schema_version is None


def test_migrated_genome_stays_migrated_on_round_trip():
    g = Genome.from_dict(_legacy_doc(3))
    again = Genome.from_dict(g.to_dict())
    assert again.source_schema_version == 3
    assert again.to_dict() == g.to_dict()


def test_child_of_a_migrated_parent_is_a_native_v4_record():
    parent = Genome.from_dict(_legacy_doc(3))
    child, _records = mutate(parent, random.Random(1), birth_index=0,
                             generation=1)
    assert child.schema_version == 4
    # the child was authored now; it does not pretend to be a migration
    assert child.source_schema_version is None
    assert [s.substrate_id for s in child.substrates] == ["fba0"]


def test_substrate_genes_are_inherited_through_mutation():
    g = fba0_genome(seed=2)
    g.substrates.append(SubstrateGene(substrate_id="proto0",
                                      kind="synthetic-test"))
    g = g.finalize()
    child, _ = mutate(g, random.Random(5), birth_index=0, generation=1)
    assert [s.substrate_id for s in child.substrates] == ["fba0", "proto0"]


def test_development_sees_explicit_and_implicit_substrates_alike():
    """A migrated legacy genome and a native v4 founder develop into the
    same substrate description."""
    legacy = Genome.from_dict(_legacy_doc(2))
    native = fba0_genome(seed=11)
    pl = develop(legacy, base_neurons=1000)
    pn = develop(native, base_neurons=1000)
    assert [s["substrate_id"] for s in pl["substrates"]] == ["fba0"]
    assert pl["substrate_neurons"] == pn["substrate_neurons"] == \
        {"fba0": 1000}
    assert pl["base"] == pn["base"]


def test_disabled_substrate_gene_is_not_developed():
    g = fba0_genome(seed=1)
    g.substrates.append(SubstrateGene(substrate_id="proto0",
                                      kind="synthetic-test",
                                      enabled=False))
    phen = develop(g.finalize(), base_neurons=500)
    assert [s["substrate_id"] for s in phen["substrates"]] == ["fba0"]
