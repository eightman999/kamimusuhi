"""M1 golden regression — the behaviour the M2 substrate refactor must
not change.

These tests pin the *observable* M1 contract, not the implementation:

- ``fba0_genome(seed)`` is deterministic, and legacy genome documents
  (schema v1-v3) still load with their historical ``genome_id`` intact.
- ``develop(genome)`` produces the same phenotype *fields* with the same
  meaning: ``base`` is the FBA0 reference description, ``species_base``
  is the genome's, ``n_extra_neurons`` counts enabled organ neurons,
  ``attachments`` carry only live endpoints, ``params`` are the resolved
  LIF parameters, ``structure`` is the M1 section-4 classification and
  ``genome_bytes`` is the canonical record size.
- The section-4 classification itself: ``fba0 -> organ -> fba0`` is
  ``functional``; a dangling organ is ``invalid_structure``; an organ
  island is ``neutral_structure``; a disabled organ is ``disabled``.
- Mutation replay: same parent + same RNG state + same
  (generation, birth_index) + same config gives a byte-identical child
  and identical mutation records.
- Evaluation replay: same genome + seed + environment gives the same
  summary on the mock backend.

Where the M2 migration legitimately changes a *value* (a v4 genome
serialises one more field, so ``genome_bytes`` grows), the test pins the
invariant — ``genome_bytes == len(genome.to_json())`` — rather than the
number. Nothing here depends on internals that the refactor may move.
"""
from __future__ import annotations

import json
import random

import pytest

from experiments.mioba.development.phenotype import develop
from experiments.mioba.genome.mutation import mutate
from experiments.mioba.genome.schema import (ArtificialOrgan, Attachment,
                                             Genome, fba0_genome)
from experiments.mioba.genome.structure import analyse


# ---------------------------------------------------------------- A-1 genome
def _canonical(genome: Genome) -> dict:
    d = genome.to_dict()
    d.pop("created_at", None)   # wallclock; excluded from genome_hash
    return d


def test_founder_genome_is_deterministic():
    a, b = fba0_genome(seed=7), fba0_genome(seed=7)
    assert a.genome_id == b.genome_id
    assert _canonical(a) == _canonical(b)
    c = fba0_genome(seed=8)
    assert c.genome_id != a.genome_id


def test_genome_json_round_trip_is_stable():
    g = fba0_genome(seed=3)
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_x", kind="lif_cluster", size=16))
    g.attachments += [
        Attachment(attachment_id="a1", source="fba0:medulla",
                   target="org_x"),
        Attachment(attachment_id="a2", source="org_x",
                   target="fba0:central_complex"),
    ]
    g = g.finalize()
    again = Genome.from_json(g.to_json())
    assert again.to_dict() == g.to_dict()
    assert again.genome_id == g.genome_id


def _legacy_doc(schema_version: int) -> dict:
    """A genome document as M0/M1 code wrote it: no ``substrates`` field,
    historical ``genome_id`` recorded verbatim."""
    return {
        "parent_ids": [], "generation": 0, "birth_index": 0,
        "random_seed": 11, "genome_id": "b2b:historical-founder",
        "schema_version": schema_version, "species_base": "fba0",
        "ancestral_base": "flywire-v783-shiu-lif",
        "artificial_organs": [], "attachments": [],
        "parameter_mutations": [], "development_rules": [],
        "plasticity_rules": [], "created_at": "2026-01-01T00:00:00+00:00",
    }


@pytest.mark.parametrize("version", (1, 2, 3))
def test_legacy_genome_documents_still_load(version):
    g = Genome.from_dict(_legacy_doc(version))
    # the historical identity is never recomputed on load
    assert g.genome_id == "b2b:historical-founder"
    assert g.species_base == "fba0"
    # ... and the genome still develops into a simulatable phenotype
    phen = develop(g, base_neurons=1000)
    assert phen["species_base"] == "fba0"
    assert phen["n_extra_neurons"] == 0


def test_founder_develops_to_fba0_reference():
    phen = develop(fba0_genome(seed=1), base_neurons=1000)
    assert phen["base"]["name"] == "flywire-v783-shiu-lif"
    assert phen["species_base"] == "fba0"


# ------------------------------------------------------------ A-2 develop
def _wired_genome() -> Genome:
    g = fba0_genome(seed=5)
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_a", kind="lif_cluster", size=24))
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_dead", kind="lif_cluster", size=8,
                        enabled=False))
    g.attachments += [
        Attachment(attachment_id="in", source="fba0:medulla",
                   target="org_a"),
        Attachment(attachment_id="out", source="org_a",
                   target="fba0:central_complex"),
        # dangling: names a disabled organ; must not reach the backend
        Attachment(attachment_id="dead", source="org_dead",
                   target="fba0:medulla"),
    ]
    return g.finalize()


def test_develop_fields_keep_their_meaning():
    g = _wired_genome()
    phen = develop(g, base_neurons=1000)
    for key in ("species_base", "base", "n_extra_neurons",
                "artificial_organs", "attachments", "params", "structure",
                "genome_bytes", "ancestry_fraction"):
        assert key in phen, key
    # the disabled organ is kept in the lineage but not built
    assert phen["n_extra_neurons"] == 24
    assert [o["organ_id"] for o in phen["artificial_organs"]] == ["org_a"]
    assert [a["attachment_id"] for a in phen["attachments"]] == ["in", "out"]
    # ancestry_fraction: FBA0 neurons / (FBA0 + artificial neurons)
    assert phen["ancestry_fraction"] == pytest.approx(1000 / 1024)
    # resolved LIF parameters ride on the phenotype
    assert phen["params"]["vThr"] == -45.0
    assert phen["params"]["tauMem"] == 20.0
    # genome storage cost = size of the canonical record itself
    assert phen["genome_bytes"] == len(g.to_json().encode("utf-8"))
    assert phen["structure"]["organs"]["org_a"] == "functional"


# ------------------------------------------------------- A-3 classification
def test_golden_structural_classification():
    # functional: fba0 -> organ -> fba0
    g = _wired_genome()
    rep = analyse(g)
    assert rep.organs["org_a"] == "functional"
    assert rep.organs["org_dead"] == "disabled"

    # invalid: fba0 -> organ, nothing out
    g = fba0_genome()
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_a", kind="lif_cluster", size=8))
    g.attachments.append(Attachment(attachment_id="in",
                                    source="fba0:medulla", target="org_a"))
    assert analyse(g.finalize()).organs["org_a"] == "invalid_structure"

    # neutral: organ island (wired both ways, never touches fba0)
    g = fba0_genome()
    g.artificial_organs += [
        ArtificialOrgan(organ_id="org_a", kind="lif_cluster", size=8),
        ArtificialOrgan(organ_id="org_b", kind="lif_cluster", size=8)]
    g.attachments += [
        Attachment(attachment_id="ab", source="org_a", target="org_b"),
        Attachment(attachment_id="ba", source="org_b", target="org_a")]
    rep = analyse(g.finalize())
    assert rep.organs == {"org_a": "neutral_structure",
                          "org_b": "neutral_structure"}


def test_report_counts_are_stable():
    rep = analyse(_wired_genome()).to_dict()
    assert rep["n_organs"] == 2
    assert rep["n_enabled_organs"] == 1
    assert rep["n_artificial_neurons"] == 24
    assert rep["n_functional_neurons"] == 24
    assert rep["dangling_attachments"] == ["dead"]
    assert rep["functional"] is True


# --------------------------------------------------------- A-4 mutation
def test_mutation_replay_is_byte_identical():
    """Same parent, RNG state, generation, birth_index and config give
    the same child genome hash and the same mutation records."""
    parent = _wired_genome()
    cfg = {"evolution": {"mutation": {"weights": {"parameter": 0.35,
                                                "structural": 0.55,
                                                "prune": 0.10}}}}
    runs = []
    for _ in range(2):
        rng = random.Random(42)
        child, records = mutate(parent, rng, birth_index=3, generation=2,
                                config=cfg)
        runs.append((_canonical(child), [r.to_dict() for r in records]))
    assert runs[0] == runs[1]


def test_lineage_replay_is_stable_over_generations():
    runs = []
    for _ in range(2):
        rng = random.Random(20260911)
        g, chain = fba0_genome(seed=20260911), []
        for gen in range(1, 12):
            g, records = mutate(g, rng, birth_index=gen % 4,
                                generation=gen)
            chain.append((g.genome_id,
                          [r.to_dict() for r in records]))
        runs.append(chain)
    assert runs[0] == runs[1]


# --------------------------------------------------------- A-5 evaluation
def _job(genome: Genome, config: dict | None = None) -> dict:
    from experiments.mioba.fba.replicates import replicate_seeds
    seed = int(genome.random_seed)
    return {"job_id": "golden", "seed": seed,
            "duration_ms": 50.0, "environment_id": "synthetic-quiet-v0",
            "replicates": 2, "replicate_seeds": replicate_seeds(seed, 2),
            "config": config or {}}


def _eval_summary(genome: Genome, config: dict | None = None) -> dict:
    from experiments.mioba.fba.registry import get_backend
    from experiments.mioba.workers.worker import evaluate_replicates
    backend = get_backend("mock", n_neurons=256, connectivity=0.02)
    job = _job(genome, config)
    phen = develop(genome, base_neurons=256)
    return evaluate_replicates(backend, phen, job, "cpu",
                               execution_batch=1)["summary"]


def test_mock_evaluation_replay_is_identical():
    g = _wired_genome()
    a, b = _eval_summary(g), _eval_summary(g)
    for key in ("mean_rate_hz", "rate_std_hz", "active_fraction",
                "spikes_total", "per_replicate_mean_rate_hz",
                "per_replicate_spike_counts", "replicate_seeds"):
        assert a[key] == b[key], key


def test_mock_evaluation_with_environment_replay_is_identical():
    """The seeded virtual-environment path (M1 §5-7) must also replay."""
    config = {"environment": {"enabled": True,
                              "virtual": {"slices": 4,
                                          "sensor_neurons_per_channel": 8,
                                          "sensor_rate_hz": 80.0,
                                          "debt_decay": 0.9,
                                          "debt_gain": 1.0,
                                          "debt_terminate": 6.0},
                              "disturbance": {"enabled": True}},
              "env": {"stim_fraction": 0.01, "stim_rate_hz": 50.0},
              "evaluation": {"target_rate_hz": 5.0}}
    g = _wired_genome()
    a, b = (_eval_summary(g, config)["episode"]
            for _ in range(2))
    assert a == b
