"""M2 topology modes: the historical M1 classification is frozen, and
the generic causal mode extends it to any declared substrate.

For every M1 genome the two modes must agree — the fba0 loop is the
generic rule restricted to one substrate.
"""
from __future__ import annotations

from experiments.mioba.development.phenotype import develop
from experiments.mioba.genome.schema import (ArtificialOrgan, Attachment,
                                             Genome, SubstrateGene,
                                             fba0_genome)
from experiments.mioba.genome.structure import (TOPOLOGY_GENERIC_CAUSAL,
                                                analyse)


def _grown(seed: int = 1) -> Genome:
    """One properly wired organ (fba0 -> organ -> fba0)."""
    g = fba0_genome(seed=seed)
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_a", kind="lif_cluster", size=16))
    g.attachments += [
        Attachment(attachment_id="in", source="fba0:medulla",
                   target="org_a"),
        Attachment(attachment_id="out", source="org_a",
                   target="fba0:central_complex")]
    return g.finalize()


def test_generic_mode_agrees_with_m1_on_m1_genomes():
    g = _grown()
    a = analyse(g)                                    # default m1 mode
    b = analyse(g, topology_mode=TOPOLOGY_GENERIC_CAUSAL)
    assert a.organs == b.organs == {"org_a": "functional"}
    assert a.counts == b.counts
    assert a.dangling_attachments == b.dangling_attachments == []


def test_generic_mode_uses_canonical_endpoint_spellings():
    """``substrate:fba0/medulla`` is the same node as ``fba0:medulla``."""
    g = fba0_genome()
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_a", kind="lif_cluster", size=8))
    g.attachments += [
        Attachment(attachment_id="in", source="substrate:fba0/medulla",
                   target="org_a"),
        Attachment(attachment_id="out", source="org_a",
                   target="substrate:fba0/central_complex")]
    rep = analyse(g.finalize(), topology_mode=TOPOLOGY_GENERIC_CAUSAL)
    assert rep.organs == {"org_a": "functional"}
    # the historical mode recognises them too — the parser normalises
    assert analyse(g).organs == {"org_a": "functional"}


def test_endpoint_to_a_substrate_the_genome_lacks_is_dangling():
    g = fba0_genome()
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_a", kind="lif_cluster", size=8))
    g.attachments += [
        Attachment(attachment_id="in", source="substrate:proto0/root",
                   target="org_a"),
        Attachment(attachment_id="out", source="org_a",
                   target="substrate:proto0/sink")]
    rep = analyse(g.finalize(), topology_mode=TOPOLOGY_GENERIC_CAUSAL)
    assert rep.dangling_attachments == ["in", "out"]
    assert rep.organs == {"org_a": "invalid_structure"}


def test_m1_mode_has_no_external_nodes_without_the_fba0_substrate():
    """A genome whose enabled substrate set excludes fba0 has no source
    or sink under the historical mode — its ``fba0:`` endpoints are
    dangling, the same answer generic mode and develop()'s wiring
    filter give."""
    g = fba0_genome()
    g.substrates = [SubstrateGene(substrate_id="proto0",
                                  kind="synthetic-test")]
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_a", kind="lif_cluster", size=8))
    g.attachments += [
        Attachment(attachment_id="in", source="fba0:medulla",
                   target="org_a"),
        Attachment(attachment_id="out", source="org_a",
                   target="fba0:central_complex")]
    rep = analyse(g.finalize())                    # default m1 mode
    assert rep.dangling_attachments == ["in", "out"]
    assert rep.organs == {"org_a": "invalid_structure"}


def test_generic_mode_uses_env_endpoints_as_habitat_boundary():
    g = fba0_genome()
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_a", kind="lif_cluster", size=8))
    g.attachments += [
        Attachment(attachment_id="in", source="env:habitat/forage",
                   target="org_a"),
        Attachment(attachment_id="out", source="org_a",
                   target="env:habitat/forage")]
    rep = analyse(g.finalize(), topology_mode=TOPOLOGY_GENERIC_CAUSAL)
    assert rep.organs == {"org_a": "functional"}


def test_island_stays_neutral_in_generic_mode():
    g = fba0_genome()
    g.artificial_organs += [
        ArtificialOrgan(organ_id="a", kind="lif_cluster", size=8),
        ArtificialOrgan(organ_id="b", kind="lif_cluster", size=8)]
    g.attachments += [
        Attachment(attachment_id="ab", source="a", target="b"),
        Attachment(attachment_id="ba", source="b", target="a")]
    for mode in ("m1_fba0_loop", TOPOLOGY_GENERIC_CAUSAL):
        rep = analyse(g.finalize(), topology_mode=mode)
        assert rep.organs == {"a": "neutral_structure",
                              "b": "neutral_structure"}, mode


def test_development_resolves_substrates_through_the_registry():
    """The phenotype names its substrates via the registry, and M1
    fields are unchanged."""
    phen = develop(_grown(), base_neurons=1000)
    assert phen["base"]["name"] == "flywire-v783-shiu-lif"
    assert phen["substrates"][0]["substrate_id"] == "fba0"
    assert phen["substrate_neurons"] == {"fba0": 1000}
    assert phen["structural_ancestry_fraction"] == \
        phen["ancestry_fraction"] == phen["fba0_structural_fraction"]
    assert phen["topology_mode"] == "m1_fba0_loop"
