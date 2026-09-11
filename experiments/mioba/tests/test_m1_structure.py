"""M1 §3-4: structural evolution, and what counts as a real circuit.

M0's lesson was that mutation which does not move the phenotype makes
selection a no-op. These tests pin the parts of the fix that are easy to
regress: the operators actually change structure, the caps hold, a new
organ is always wired both ways, and an organ that cannot matter is
labelled as such rather than counted as growth.
"""
from __future__ import annotations

import random
from collections import Counter

import pytest

from experiments.mioba.development.phenotype import develop
from experiments.mioba.genome import mutation as mut
from experiments.mioba.genome.mutation import (ALL_OPERATORS,
                                               OUTCOME_APPLIED,
                                               OUTCOME_AT_LIMIT,
                                               OUTCOME_NO_TARGET,
                                               apply_operator, merged_config,
                                               mutate, mutation_count)
from experiments.mioba.genome.schema import (ArtificialOrgan, Attachment,
                                             Genome, fba0_genome)
from experiments.mioba.genome.structure import analyse
from experiments.mioba.storage import models as M


def _prov(genome):
    return genome.provenance(birth_mutation_id="m")


def _grown(seed: int = 1, organs: int = 1) -> Genome:
    """A genome with `organs` properly wired artificial organs."""
    g = fba0_genome()
    rng = random.Random(seed)
    cfg = merged_config(None)
    for _ in range(organs):
        apply_operator(g, rng, "NEW_ORGAN", cfg, _prov(g))
    return g.finalize()


# ------------------------------------------------------------ mutation rate
def test_children_carry_two_to_three_mutations_on_average():
    """§3: 1 + Poisson(1.5) capped at 4. M0's one-per-child was the
    reason nine generations moved nothing."""
    cfg = merged_config(None)
    rng = random.Random(7)
    counts = [mutation_count(rng, cfg) for _ in range(4000)]
    assert min(counts) >= 1 and max(counts) <= 4
    mean = sum(counts) / len(counts)
    assert 2.0 <= mean <= 3.0, mean


def test_mutation_count_respects_config():
    cfg = merged_config({"evolution": {"mutation": {
        "count": {"base": 2, "poisson_lambda": 0.0, "cap": 9}}}})
    rng = random.Random(1)
    assert {mutation_count(rng, cfg) for _ in range(50)} == {2}


def test_operator_mix_is_structural_dominant():
    """§3: structural 50-60%, parameter 30-40%, prune ~10%."""
    cfg = merged_config(None)
    rng = random.Random(11)
    cats = Counter(mut.choose_operator(rng, cfg)[0] for _ in range(6000))
    total = sum(cats.values())
    assert 0.50 <= cats["structural"] / total <= 0.60
    assert 0.30 <= cats["parameter"] / total <= 0.40
    assert 0.05 <= cats["prune"] / total <= 0.15


def test_every_declared_operator_is_implemented():
    for op in ALL_OPERATORS:
        assert op in mut._OPERATORS, op


# -------------------------------------------------------------- operators
def test_new_organ_is_wired_in_and_out():
    """§13: an organ that only receives, or only emits, cannot matter."""
    g = _grown()
    rep = analyse(g)
    assert rep.n_enabled_organs == 1
    assert rep.n_enabled_attachments == 2
    assert set(rep.organs.values()) == {"functional"}


def test_grow_organ_adds_neurons_without_changing_wiring():
    g = _grown()
    before = analyse(g)
    rec = apply_operator(g, random.Random(3), "GROW_ORGAN",
                         merged_config(None), _prov(g))
    after = analyse(g)
    assert rec.outcome == OUTCOME_APPLIED and rec.operator == "GROW_ORGAN"
    assert 4 <= rec.value <= 32
    assert after.n_artificial_neurons == (before.n_artificial_neurons
                                          + int(rec.value))
    assert after.n_enabled_attachments == before.n_enabled_attachments


def test_duplicate_organ_copies_the_wiring_pattern():
    g = _grown()
    rec = apply_operator(g, random.Random(5), "DUPLICATE_ORGAN",
                         merged_config(None), _prov(g))
    rep = analyse(g)
    assert rec.outcome == OUTCOME_APPLIED
    assert rep.n_enabled_organs == 2
    # the copy is wired like the original, so it is functional too
    assert list(rep.counts) == ["functional"] and rep.counts["functional"] == 2
    copy_id = rec.target
    organ = next(o for o in g.artificial_organs if o.organ_id == copy_id)
    assert copy_id in organ.provenance.ancestry[-1:] or \
        organ.provenance.ancestry[-1] == rec.detail["copied_from"]


def test_inter_organ_edge_needs_two_organs():
    one = _grown(organs=1)
    rec = apply_operator(one, random.Random(2), "ADD_INTER_ORGAN_EDGE",
                         merged_config(None), _prov(one))
    assert rec.outcome == OUTCOME_NO_TARGET

    two = _grown(organs=2)
    rec = apply_operator(two, random.Random(2), "ADD_INTER_ORGAN_EDGE",
                         merged_config(None), _prov(two))
    assert rec.outcome == OUTCOME_APPLIED
    ids = {o.organ_id for o in two.artificial_organs}
    assert rec.detail["source"] in ids and rec.detail["target"] in ids


def test_rewire_moves_exactly_one_endpoint():
    g = _grown()
    rec = apply_operator(g, random.Random(4), "REWIRE_ATTACHMENT",
                         merged_config(None), _prov(g))
    assert rec.outcome == OUTCOME_APPLIED
    before, after = rec.detail["before"], rec.detail["after"]
    changed = [k for k in ("source", "target") if before[k] != after[k]]
    assert len(changed) == 1


def test_prune_organ_removes_its_attachments_too():
    g = _grown(organs=2)
    rec = apply_operator(g, random.Random(6), "PRUNE_ORGAN",
                         merged_config(None), _prov(g))
    assert rec.outcome == OUTCOME_APPLIED
    gone = rec.target
    assert all(o.organ_id != gone for o in g.artificial_organs)
    assert all(gone not in (a.source, a.target) for a in g.attachments)
    assert analyse(g).dangling_attachments == []


def test_disable_organ_keeps_it_in_the_genome_but_out_of_the_body():
    g = _grown()
    rec = apply_operator(g, random.Random(8), "DISABLE_ORGAN",
                         merged_config(None), _prov(g))
    assert rec.outcome == OUTCOME_APPLIED
    assert len(g.artificial_organs) == 1          # lineage keeps it
    rep = analyse(g)
    assert rep.organs[rec.target] == "disabled"
    assert rep.n_artificial_neurons == 0
    phen = develop(g.finalize(), base_neurons=1000)
    assert phen["n_extra_neurons"] == 0           # costs nothing to simulate
    assert phen["artificial_organs"] == []
    assert phen["attachments"] == []              # its attachments too


# ----------------------------------------------------------------- limits
def test_caps_are_enforced_and_recorded_not_silently_skipped():
    cfg = merged_config({"evolution": {"mutation": {
        "limits": {"artificial_neurons": 512, "artificial_organs": 2,
                   "attachments": 24}}}})
    g = _grown(organs=2)
    rec = apply_operator(g, random.Random(9), "NEW_ORGAN", cfg, _prov(g))
    assert rec.outcome == OUTCOME_AT_LIMIT
    assert rec.detail["limit"] == "artificial_organs"
    assert analyse(g).n_enabled_organs == 2       # nothing was added


def test_neuron_cap_blocks_growth():
    cfg = merged_config({"evolution": {"mutation": {
        "limits": {"artificial_neurons": 16, "artificial_organs": 8,
                   "attachments": 24},
        "new_organ_size": [8, 8], "grow_organ_delta": [32, 32]}}})
    g = fba0_genome()
    rng = random.Random(1)
    assert apply_operator(g, rng, "NEW_ORGAN", cfg,
                          _prov(g)).outcome == OUTCOME_APPLIED
    rec = apply_operator(g, rng, "GROW_ORGAN", cfg, _prov(g))
    assert rec.outcome == OUTCOME_AT_LIMIT
    assert rec.detail["limit"] == "artificial_neurons"


def test_a_long_lineage_never_exceeds_its_caps():
    cfg = {"evolution": {"mutation": {
        "limits": {"artificial_neurons": 512, "artificial_organs": 8,
                   "attachments": 24}}}}
    rng = random.Random(20260911)
    g = fba0_genome()
    for gen in range(1, 60):
        g, _records = mutate(g, rng, birth_index=0, generation=gen,
                             config=cfg)
        rep = analyse(g)
        assert rep.n_artificial_neurons <= 512
        assert rep.n_enabled_organs <= 8
        assert rep.n_enabled_attachments <= 24


# -------------------------------------------------------------- structure
def test_orphaned_organ_is_invalid_not_growth():
    """A prune that removes an organ's only outgoing edge leaves dead
    tissue. That is a real event: it is labelled, not repaired."""
    g = _grown()
    organ_id = g.artificial_organs[0].organ_id
    g.attachments = [a for a in g.attachments if a.source != organ_id]
    rep = analyse(g)
    assert rep.organs[organ_id] == "invalid_structure"
    assert rep.n_functional_neurons == 0
    assert rep.n_artificial_neurons > 0            # still costs resources


def test_island_of_organs_is_neutral_not_functional():
    """Two organs wired only to each other run, cost memory, and cannot
    affect anything the environment drives or reads."""
    g = fba0_genome()
    a = ArtificialOrgan(organ_id="org_a", kind="lif_cluster", size=16)
    b = ArtificialOrgan(organ_id="org_b", kind="lif_cluster", size=16)
    g.artificial_organs += [a, b]
    g.attachments += [
        Attachment(attachment_id="att_ab", source="org_a", target="org_b"),
        Attachment(attachment_id="att_ba", source="org_b", target="org_a"),
    ]
    rep = analyse(g.finalize())
    assert rep.organs == {"org_a": "neutral_structure",
                          "org_b": "neutral_structure"}
    assert rep.n_functional_neurons == 0
    assert rep.n_artificial_neurons == 32


def test_organ_fed_by_fba0_but_going_nowhere_useful_is_neutral():
    g = fba0_genome()
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_a", kind="lif_cluster", size=8))
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_dead", kind="lif_cluster", size=8))
    g.attachments += [
        Attachment(attachment_id="in", source="fba0:medulla", target="org_a"),
        Attachment(attachment_id="mid", source="org_a", target="org_dead"),
        Attachment(attachment_id="loop", source="org_dead", target="org_a"),
    ]
    rep = analyse(g.finalize())
    # both are driven by FBA0 and neither reaches back into it
    assert set(rep.organs.values()) == {"neutral_structure"}


def test_bidirectional_attachment_satisfies_both_directions():
    g = fba0_genome()
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_a", kind="lif_cluster", size=8))
    g.attachments.append(Attachment(attachment_id="att",
                                    source="fba0:medulla", target="org_a",
                                    direction="bidirectional"))
    rep = analyse(g.finalize())
    assert rep.organs["org_a"] == "functional"


def test_structure_report_reaches_the_phenotype():
    phen = develop(_grown(), base_neurons=1000)
    assert phen["structure"]["functional"] is True
    assert phen["structure"]["n_functional_neurons"] > 0


# ------------------------------------------------------------ persistence
def test_births_record_the_operator_and_its_outcome(service):
    """§3: "this child grew an organ" must be a recorded fact, not
    something re-derived from the genome (which cannot tell growth from
    inheritance)."""
    rows = service.db.conn.execute(
        "SELECT operator, outcome, kind FROM mutations").fetchall()
    assert rows, "the seeded generation recorded no mutations"
    assert all(r["operator"] for r in rows)
    assert {r["outcome"] for r in rows} <= {OUTCOME_APPLIED, OUTCOME_AT_LIMIT,
                                            OUTCOME_NO_TARGET}
    assert {r["kind"] for r in rows} <= {"parameter", "structural", "prune"}


def test_genome_rows_carry_their_structure(service):
    import json

    rows = service.db.conn.execute(
        "SELECT structure_json FROM genomes").fetchall()
    reports = [json.loads(r["structure_json"]) for r in rows]
    assert any(r for r in reports), "no structure recorded"
    for rep in reports:
        if rep:
            assert "counts" in rep and "n_artificial_neurons" in rep


def test_structural_events_are_emitted(service):
    events = service.db.list_events(service.experiment_id, 0, 10000)
    types = {e["type"] for e in events}
    # the seeded generation is 8 children under the default weights, so a
    # structural operator is essentially certain to have fired
    assert M.EV_STRUCTURAL_MUTATION in types


def test_mutation_is_still_deterministic_given_rng_state():
    base = fba0_genome()
    a, ra = mutate(base, random.Random(3), 0, 1)
    b, rb = mutate(base, random.Random(3), 0, 1)
    assert a.genome_id == b.genome_id
    assert [r.to_dict() for r in ra] == [r.to_dict() for r in rb]
    c, _ = mutate(base, random.Random(4), 0, 1)
    assert c.genome_id != a.genome_id


def test_parameter_weights_can_suppress_an_insensitive_parameter():
    """§10 feeds back into §3: a parameter the sweep shows is inert gets a
    lower draw probability."""
    cfg = merged_config({"evolution": {"mutation": {
        "parameter_weights": {"wScale": 1.0, "tauMem": 0.0, "tauSyn": 0.0,
                              "vThr": 0.0, "tRefrac": 0.0}}}})
    g = fba0_genome()
    rng = random.Random(5)
    paths = {apply_operator(g, rng, "SCALE_PARAMETER", cfg, _prov(g)).target
             for _ in range(30)}
    assert paths == {"wScale"}


def test_per_parameter_scale_ranges_are_honoured():
    cfg = merged_config({"evolution": {"mutation": {
        "parameter_scale": [0.99, 1.01],
        "parameter_scale_by_path": {"vThr": [0.6, 1.4]}}}})
    g = fba0_genome()
    rng = random.Random(6)
    values = {}
    for _ in range(200):
        rec = apply_operator(g, rng, "SCALE_PARAMETER", cfg, _prov(g))
        values.setdefault(rec.target, []).append(rec.value)
    assert max(values["vThr"]) > 1.01
    for path, vals in values.items():
        if path != "vThr":
            assert 0.99 <= min(vals) and max(vals) <= 1.01
