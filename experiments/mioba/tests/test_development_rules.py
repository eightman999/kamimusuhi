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


def test_grow_reports_the_applied_delta_not_the_requested_one():
    """A negative delta that hits the size floor records the neuron
    change that actually happened — the same accounting SCALE uses."""
    g = fba0_genome(seed=8)
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_x", kind="lif_cluster", size=8))
    g.development_rules.append(
        {"op": "GROW_ORGAN_AT_BIRTH", "organ_id": "org_x",
         "delta": -100})
    phen = develop(g.finalize(), base_neurons=100)
    rep = phen["development"]
    assert rep["applied"][0]["size"] == 1
    assert rep["added_neurons"] == -7
    assert phen["n_extra_neurons"] == 1


def test_no_rules_means_no_development_record_noise():
    phen = develop(fba0_genome(seed=1), base_neurons=100)
    rep = phen["development"]
    assert rep == {"applied": [], "skipped": [], "deferred": [],
                   "added_organs": 0, "added_neurons": 0}


def test_development_never_writes_back_to_the_genome():
    """GROW/SCALE rules act on the phenotype, not the genome: develop()
    is idempotent, the genome's content still hashes to its stored
    genome_id, and the grown size is not heritable (§14)."""
    from experiments.mioba.genome.hashing import genome_hash
    g = fba0_genome(seed=8)
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_x", kind="lif_cluster", size=16))
    g.development_rules += [
        {"op": "GROW_ORGAN_AT_BIRTH", "organ_id": "org_x", "delta": 8},
        {"op": "SCALE_ORGAN_AT_BIRTH", "organ_id": "org_x",
         "factor": 2.0}]
    g = g.finalize()

    a, b = develop(g, base_neurons=100), develop(g, base_neurons=100)
    assert a["artificial_organs"] == b["artificial_organs"]
    assert a["artificial_organs"][0]["size"] == (16 + 8) * 2
    # the genome object is untouched and still hashes to its identity
    assert g.artificial_organs[0].size == 16
    assert genome_hash(g) == g.genome_id
    # children are built from the record — they inherit the recipe
    # (size 16), never the developmental product (48)
    from experiments.mioba.genome.schema import Genome
    clone = Genome.from_dict(g.to_dict())
    assert clone.artificial_organs[0].size == 16


def test_structure_report_uses_developed_sizes():
    """The §4 classification counts the body development produced, not
    the literal gene sizes."""
    g = fba0_genome(seed=8)
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_x", kind="lif_cluster", size=16))
    g.attachments += [
        Attachment(attachment_id="in", source="fba0:medulla",
                   target="org_x"),
        Attachment(attachment_id="out", source="org_x",
                   target="fba0:central_complex")]
    g.development_rules.append(
        {"op": "GROW_ORGAN_AT_BIRTH", "organ_id": "org_x", "delta": 8})
    phen = develop(g.finalize(), base_neurons=100)
    assert phen["structure"]["n_artificial_neurons"] == 24
    assert phen["structure"]["n_functional_neurons"] == 24


def test_add_organ_rule_cannot_reuse_a_disabled_organs_id():
    """organ_id is the report key: a rule may not shadow a disabled
    organ's id (the disabled organ stays in the record)."""
    g = fba0_genome(seed=8)
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_d", kind="lif_cluster", size=8,
                        enabled=False))
    g.development_rules.append(
        {"op": "ADD_ORGAN_AT_BIRTH", "organ_id": "org_d", "size": 4})
    phen = develop(g.finalize(), base_neurons=100)
    rep = phen["development"]
    assert rep["skipped"][0]["reason"] == "organ_id_exists"
    assert phen["structure"]["organs"] == {"org_d": "disabled"}


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
