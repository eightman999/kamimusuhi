"""Development rules: the genome is a recipe; develop() applies the
birth-stage rules deterministically and the phenotype differs from the
literal gene list."""
from __future__ import annotations

from experiments.mioba.development.phenotype import develop
from experiments.mioba.genome.schema import (ArtificialOrgan, Attachment,
                                             fba0_genome)


def _genome_with_rule(rule: dict):
    g = fba0_genome(seed=8)
    g.development_rules.append(rule)
    return g.finalize()


def test_add_organ_at_birth_grows_the_body():
    g = _genome_with_rule({"op": "ADD_ORGAN_AT_BIRTH", "size": 24})
    phen = develop(g, base_neurons=1000)
    ids = [o["organ_id"] for o in phen["artificial_organs"]]
    assert ids == ["dev_0"]
    assert phen["n_extra_neurons"] == 24
    # default wiring lands on the primary substrate's upstream ->
    # downstream ports (M1 §13 in-and-out rule)
    atts = {a["attachment_id"]: a for a in phen["attachments"]}
    assert atts["att_dev0_in"]["source"] == "fba0:medulla"
    assert atts["att_dev0_out"]["target"] == "fba0:central_complex"
    # and the developed organ is classified functional, not neutral
    assert phen["structure"]["organs"]["dev_0"] == "functional"
    rep = phen["development"]
    assert rep["applied"][0]["organ_id"] == "dev_0"
    assert rep["added_organs"] == 1 and rep["added_neurons"] == 24


def test_development_is_deterministic_and_not_in_the_genome():
    g = _genome_with_rule({"op": "ADD_ORGAN_AT_BIRTH", "size": 24,
                           "organ_id": "org_dev"})
    a, b = develop(g, base_neurons=1000), develop(g, base_neurons=1000)
    assert a["artificial_organs"] == b["artificial_organs"]
    assert a["development"] == b["development"]
    # the genome itself is untouched: the rule is content, the organ is
    # a product — reproduction copies the rule, not the tissue
    assert g.artificial_organs == []


def test_rules_apply_in_declared_order():
    g = fba0_genome(seed=8)
    g.development_rules += [
        {"op": "ADD_ORGAN_AT_BIRTH", "size": 16, "organ_id": "org_x"},
        {"op": "GROW_ORGAN_AT_BIRTH", "organ_id": "org_x", "delta": 8},
        {"op": "SCALE_ORGAN_AT_BIRTH", "organ_id": "org_x",
         "factor": 2.0}]
    phen = develop(g.finalize(), base_neurons=1000)
    organ = phen["artificial_organs"][0]
    assert organ["size"] == (16 + 8) * 2
    assert len(phen["development"]["applied"]) == 3


def test_lifetime_rules_are_deferred_not_dropped():
    g = _genome_with_rule({"op": "GROW_AFTER_N_STEPS", "steps": 100,
                           "delta": 8})
    rep = develop(g, base_neurons=1000)["development"]
    assert rep["applied"] == []
    assert rep["deferred"][0]["op"] == "GROW_AFTER_N_STEPS"


def test_unknown_rules_are_recorded_skipped():
    g = _genome_with_rule({"op": "TELEPORT_ORGAN"})
    rep = develop(g, base_neurons=1000)["development"]
    assert rep["skipped"][0]["reason"] == "unknown_op"


def test_no_rules_means_no_development_record_noise():
    phen = develop(fba0_genome(seed=1), base_neurons=100)
    rep = phen["development"]
    assert rep == {"applied": [], "skipped": [], "deferred": [],
                   "added_organs": 0, "added_neurons": 0}


def test_rule_wired_organ_reaches_the_backend():
    """A rule-grown organ is real tissue: the torch backend builds and
    simulates it."""
    import pytest
    pytest.importorskip("torch")
    from experiments.mioba.fba.torch_backend import TorchBackend
    g = _genome_with_rule({"op": "ADD_ORGAN_AT_BIRTH", "size": 12,
                           "source": "fba0:medulla",
                           "target": "fba0:central_complex"})
    phen = develop(g, base_neurons=100)
    b = TorchBackend(synthetic=True, synthetic_neurons=100)
    b.initialize(phen, batch_size=1, seed=0, device="cpu")
    assert b.n == 112
    rates = b.get_population_activity(["organ:dev_0"])
    assert len(rates["organ:dev_0"]) == 1
