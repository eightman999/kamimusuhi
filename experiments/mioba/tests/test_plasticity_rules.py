"""Lifetime plasticity: rules are heritable; their effects are not.

The mutation/plasticity boundary (M2 §14-15): genome.plasticity_rules
and deferred development rules describe *how a body may change during
life*. The changes themselves live in LifetimeState — runtime state
that is never part of the genome record, never hashed, never inherited.
"""
from __future__ import annotations

import random

from experiments.mioba.development.lifetime import (LifetimeState,
                                                    lifetime_rules_of)
from experiments.mioba.development.phenotype import develop
from experiments.mioba.genome.mutation import mutate
from experiments.mioba.genome.schema import fba0_genome


def _genome_with_plasticity():
    g = fba0_genome(seed=9)
    g.plasticity_rules.append(
        {"op": "SCALE_WEIGHT", "target": "org_a", "factor": 1.5})
    g.development_rules.append(
        {"op": "GROW_AFTER_N_STEPS", "organ_id": "org_a", "steps": 2,
         "delta": 4})
    return g.finalize()


def test_plasticity_rules_are_genome_content_and_inherited():
    g = _genome_with_plasticity()
    child, _ = mutate(g, random.Random(1), birth_index=0, generation=1)
    assert child.plasticity_rules == g.plasticity_rules
    assert child.development_rules == g.development_rules


def test_phenotype_advertises_lifetime_rules():
    phen = develop(_genome_with_plasticity(), base_neurons=100)
    rules = phen["plasticity"]["rules"]
    assert [r["op"] for r in rules] == ["SCALE_WEIGHT",
                                        "GROW_AFTER_N_STEPS"]


def test_lifetime_state_is_per_individual_not_genome():
    g = _genome_with_plasticity()
    state = LifetimeState()
    state.apply({"op": "SCALE_WEIGHT", "target": "org_a",
                 "factor": 1.5})
    state.step = 3
    state.apply({"op": "GROW_AFTER_N_STEPS", "organ_id": "org_a",
                 "steps": 2, "delta": 4})
    # runtime state accrued — on the individual, not the genome
    adj = state.effective_organ("org_a")
    assert adj["weight_scale"] == 1.5 and adj["extra_neurons"] == 4
    assert "LifetimeState" not in repr(g.to_dict())
    # and a fresh individual of the same genome starts clean
    assert LifetimeState().effective_organ("org_a")["weight_scale"] == 1.0


def test_child_inherits_rules_not_adjustments():
    g = _genome_with_plasticity()
    parent_state = LifetimeState()
    parent_state.apply({"op": "SCALE_WEIGHT", "target": "org_a",
                        "factor": 1.5})
    child, _ = mutate(g, random.Random(1), birth_index=0, generation=1)
    # child's rules come through the genome; the parent's accrued
    # weight_scale does not — a new LifetimeState() is the child's state
    assert lifetime_rules_of(child)
    child_state = LifetimeState()
    assert child_state.effective_organ("org_a")["weight_scale"] == 1.0


def test_thresholded_rule_is_pending_until_its_step():
    state = LifetimeState()
    rule = {"op": "GROW_AFTER_N_STEPS", "organ_id": "org_a",
            "steps": 5, "delta": 8}
    ev = state.apply(rule)
    assert ev.outcome == "pending"
    state.step = 5
    ev = state.apply(rule)
    assert ev.outcome == "applied"
    assert state.effective_organ("org_a")["extra_neurons"] == 8


def test_state_serialises_for_the_record():
    import json
    state = LifetimeState()
    state.apply({"op": "DISABLE_IF_RESOURCE_LOW", "organ_id": "org_a"})
    doc = state.to_dict()
    json.dumps(doc)
    assert doc["adjustments"]["org_a"]["enabled"] is False
