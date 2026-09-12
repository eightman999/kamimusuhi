"""M3 scaffolding: substrate-region operators exist and are applicable
explicitly, but the M2 selection pool never draws them."""
from __future__ import annotations

import random
from collections import Counter

import pytest

from experiments.mioba.genome import mutation as mut
from experiments.mioba.genome.mutation import (SUBSTRATE_OPERATORS,
                                               apply_operator,
                                               merged_config)
from experiments.mioba.genome.schema import (ArtificialOrgan, Attachment,
                                             fba0_genome)
from experiments.mioba.genome.structure import TOPOLOGY_GENERIC_CAUSAL, analyse


def _prov(g):
    return g.provenance(birth_mutation_id="m")


def test_staged_operators_are_never_drawn():
    """The selection pool cannot produce a substrate operator: they are
    not in any category pool, whatever the configured weights."""
    cfg = merged_config({"evolution": {"mutation": {
        "operator_weights": {op: 100.0 for op in SUBSTRATE_OPERATORS}}}})
    rng = random.Random(1)
    drawn = Counter(mut.choose_operator(rng, cfg)[1] for _ in range(4000))
    assert not (set(drawn) & set(SUBSTRATE_OPERATORS))


def test_staged_operators_apply_explicitly():
    g = fba0_genome(seed=4)
    rec = apply_operator(g, random.Random(2), "DISABLE_SUBSTRATE_REGION",
                         merged_config(None), _prov(g))
    assert rec.outcome == "applied"
    sid, region = rec.target.split(":", 1)
    assert sid == "fba0"
    assert region in g.substrates[0].params["disabled_regions"]


def test_disabled_region_removes_the_port_from_development():
    """The staged operator is real: a disabled region's endpoints become
    dangling in generic-mode structure and are dropped by develop."""
    from experiments.mioba.development.phenotype import develop
    from experiments.mioba.genome.schema import ArtificialOrgan, Attachment
    g = fba0_genome(seed=4)
    apply_operator(g, random.Random(0), "DISABLE_SUBSTRATE_REGION",
                   merged_config(None), _prov(g))
    disabled = set(g.substrates[0].params["disabled_regions"])
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_a", kind="lif_cluster", size=8))
    for region in disabled:
        g.attachments += [
            Attachment(attachment_id=f"in_{region}",
                       source=f"fba0:{region}", target="org_a"),
            Attachment(attachment_id=f"out_{region}", source="org_a",
                       target=f"fba0:{region}")]
    g = g.finalize()
    rep = analyse(g, topology_mode=TOPOLOGY_GENERIC_CAUSAL)
    for region in disabled:
        assert f"in_{region}" in rep.dangling_attachments
    phen = develop(g, base_neurons=1000)
    ids = {a["attachment_id"] for a in phen["attachments"]}
    for region in disabled:
        assert f"in_{region}" not in ids


def test_disabled_region_is_a_lesion_in_m1_mode_too():
    """Both topology modes agree that a genome-disabled region's
    endpoints are dangling — develop() drops the attachments either
    way, so the structural record must not call them wired. (No M1
    genome carries disabled_regions; historical results are unchanged.)
    """
    g = fba0_genome(seed=4)
    apply_operator(g, random.Random(0), "DISABLE_SUBSTRATE_REGION",
                   merged_config(None), _prov(g))
    disabled = set(g.substrates[0].params["disabled_regions"])
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_a", kind="lif_cluster", size=8))
    for region in disabled:
        g.attachments += [
            Attachment(attachment_id=f"in_{region}",
                       source=f"fba0:{region}", target="org_a"),
            Attachment(attachment_id=f"out_{region}", source="org_a",
                       target=f"fba0:{region}")]
    rep = analyse(g.finalize())          # default: the M1 mode
    for region in disabled:
        assert f"in_{region}" in rep.dangling_attachments


def test_staged_operators_do_not_change_the_m1_distribution():
    """Scaffolding cannot shift the operator mix (M1 §3 frozen)."""
    cfg = merged_config(None)
    rng = random.Random(11)
    cats = Counter(mut.choose_operator(rng, cfg)[0] for _ in range(6000))
    total = sum(cats.values())
    assert 0.50 <= cats["structural"] / total <= 0.60
    assert set(cats) <= {"parameter", "structural", "prune"}
