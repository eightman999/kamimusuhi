"""M3: end-to-end substrate neutrality — reflex0, the second substrate.

``substrate/reflex0.py`` is a three-region sensorimotor arc that is not
FBA0. These tests prove the M2 abstraction is a real boundary, not an
FBA0 wrapper: a non-FBA genome goes

    genome -> develop -> evaluate -> lesion -> departure
           -> mutate -> reproduce -> persist -> replay

through exactly the same machinery as the founder line — and the
generic layers (development, genome, mutation, departure, worker,
coordinator) contain no reflex0-specific branch.

It also pins the Phase-1 semantic cleanup: a native-v4 genome that
declares substrate genes but disables every one is *substrate-less* —
that is explicit content, not the missing field a legacy record has,
so nothing silently substitutes the ancestral FBA0.
"""
from __future__ import annotations

import ast
import copy
import json
import random
from pathlib import Path

import pytest

from experiments.mioba.development.phenotype import develop
from experiments.mioba.fba.mock_backend import MockBackend
from experiments.mioba.fba.replicates import replicate_seeds
from experiments.mioba.genome import mutation as mut
from experiments.mioba.genome.mutation import (SUBSTRATE_OPERATORS,
                                               apply_operator,
                                               merged_config, mutate)
from experiments.mioba.genome.schema import (ArtificialOrgan, Attachment,
                                             Genome, SubstrateGene,
                                             fba0_genome)
from experiments.mioba.genome.structure import (TOPOLOGY_GENERIC_CAUSAL,
                                                TOPOLOGY_M1_FBA0_LOOP,
                                                analyse)
from experiments.mioba.m2.departure import evaluate_departure
from experiments.mioba.substrate.base import check_substrate
from experiments.mioba.substrate.reflex0 import (KIND, N_NEURONS,
                                                 PORT_NAMES, REGIONS,
                                                 Reflex0Adapter)
from experiments.mioba.substrate.registry import (NoEnabledSubstrate,
                                                  UnknownSubstrate,
                                                  default_registry,
                                                  substrate_genes_of,
                                                  substrate_ids_of)
from experiments.mioba.workers.worker import evaluate_replicates

GENERIC = TOPOLOGY_GENERIC_CAUSAL


def reflex0_genome(seed: int = 0) -> Genome:
    """A native-v4 genome on the second substrate: the reflex0 arc and
    nothing else."""
    g = Genome(random_seed=seed,
               substrates=[SubstrateGene(substrate_id="reflex0",
                                         kind=KIND)])
    return g.finalize()


def _wired_reflex0(seed: int = 0) -> Genome:
    """reflex0 genome with one organ on a source -> sink path."""
    g = Genome(random_seed=seed,
               substrates=[SubstrateGene(substrate_id="reflex0",
                                         kind=KIND)])
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_a", kind="lif_cluster", size=16))
    g.attachments += [
        Attachment(attachment_id="in",
                   source="substrate:reflex0/sensory_in", target="org_a"),
        Attachment(attachment_id="out", source="org_a",
                   target="substrate:reflex0/turn_out")]
    return g.finalize()


# ============================================================ Phase 1
# all-disabled substrate genes are explicit v4 content — substrate-less,
# never the implicit FBA0 a missing *field* means.
def test_all_disabled_substrate_genes_mean_no_substrate():
    g = Genome(substrates=[SubstrateGene(substrate_id="fba0",
                                         enabled=False),
                           SubstrateGene(substrate_id="reflex0",
                                         kind=KIND, enabled=False)])
    g = g.finalize()
    assert substrate_ids_of(g) == []
    assert substrate_genes_of(g) == []
    with pytest.raises(NoEnabledSubstrate):
        develop(g)


def test_no_substrate_records_still_means_the_founder():
    """The unchanged legacy rule: no substrate *records* at all (a
    v1-v3 document, a programmatically built Genome, an empty list) is
    the M1 condition — the implicit ancestral FBA0."""
    for g in (Genome(), fba0_genome(seed=3),
              Genome.from_dict({
                  "parent_ids": [], "generation": 0, "birth_index": 0,
                  "random_seed": 11, "genome_id": "b2b:historical",
                  "schema_version": 2, "species_base": "fba0",
                  "ancestral_base": "flywire-v783-shiu-lif",
                  "artificial_organs": [], "attachments": [],
                  "parameter_mutations": [], "development_rules": [],
                  "plasticity_rules": [],
                  "created_at": "2026-01-01T00:00:00+00:00"})):
        assert substrate_ids_of(g) == ["fba0"]
        assert develop(g, base_neurons=64)["base"]["name"] == \
            "flywire-v783-shiu-lif"


def test_all_disabled_is_not_confused_with_absent():
    """The two cases must not collapse: declared-and-disabled is
    substrate-less; absent is the founder."""
    declared = Genome(substrates=[SubstrateGene(substrate_id="reflex0",
                                              kind=KIND,
                                              enabled=False)])
    assert substrate_ids_of(declared) == []
    absent = Genome(substrates=[])
    assert substrate_ids_of(absent) == ["fba0"]


def test_substrateless_genome_still_analyses_and_mutates():
    """Only the processing that needs a substrate fails. The structural
    record and the mutation machinery still work — a no_target outcome
    is data, not a crash."""
    g = Genome(substrates=[SubstrateGene(substrate_id="reflex0",
                                         kind=KIND, enabled=False)],
               random_seed=5)
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_a", kind="lif_cluster", size=8))
    g.attachments += [
        Attachment(attachment_id="in",
                   source="substrate:reflex0/sensory_in", target="org_a"),
        Attachment(attachment_id="out", source="org_a",
                   target="substrate:reflex0/turn_out")]
    g = g.finalize()
    rep = analyse(g, topology_mode=GENERIC)
    assert rep.dangling_attachments == ["in", "out"]
    assert rep.organs == {"org_a": "invalid_structure"}
    child, records = mutate(g, random.Random(3), birth_index=0,
                            generation=1)
    assert child.schema_version == 4
    assert substrate_ids_of(child) == []          # inherits the disable
    assert records                                # attempts are recorded
    with pytest.raises(NoEnabledSubstrate):
        develop(child)


def test_substrateless_evaluation_fails_loudly(tmp_path, smoke_config):
    """The fail-loud is not a quiet skip: a substrate-less genome that
    reaches evaluation dies at develop() with the named error in the
    job record."""
    from fastapi.testclient import TestClient

    from experiments.mioba.coordinator.app import create_app
    from experiments.mioba.coordinator.service import MiobaService
    from experiments.mioba.tests.conftest import run_worker_once
    from experiments.mioba.storage import models as M

    cfg = copy.deepcopy(smoke_config)
    cfg["population"]["target_size"] = 0          # seed nothing
    svc = MiobaService(cfg, tmp_path / "runs")
    client = TestClient(create_app(svc))
    client.post("/api/worker/register",
                json={"worker_id": "w", "hostname": "h", "gpu": [],
                      "runtime_info": {}, "bench": [], "batch_size": 1})
    g = Genome(substrates=[SubstrateGene(substrate_id="reflex0",
                                         kind=KIND, enabled=False)],
               random_seed=5).finalize()
    svc.db.insert_genome(svc.experiment_id, g, "import")
    svc.db.enqueue_job(svc.experiment_id, g.genome_id,
                       "synthetic-quiet-v0", int(g.random_seed), "smoke",
                       "mock", 100.0, [], replicates=2)
    body = run_worker_once(client, "w")
    assert body["status"] == M.JOB_FAILED
    assert "NoEnabledSubstrate" in body["error"]
    svc.db.close()


# ============================================================ Phase 2
# the reflex0 adapter: contract conformance without any FBA import.
def test_reflex0_satisfies_the_protocol_and_is_registered():
    adapter = default_registry().get("reflex0")
    assert check_substrate(adapter) == []
    assert adapter.substrate_id == "reflex0"
    assert adapter.kind == KIND
    assert "reflex0" in default_registry().known()


def test_reflex0_ports_regions_groups_and_typed_signals():
    a = Reflex0Adapter()
    ports = a.ports()
    assert [p.name for p in ports] == list(PORT_NAMES)
    assert [p.endpoint for p in ports] == [
        f"substrate:reflex0/{n}" for n in PORT_NAMES]
    # the anatomy is the arc; the attachable namespace is the ports
    assert REGIONS == ("sensor", "integrator", "motor")
    by_name = {p.name: p for p in ports}
    assert by_name["sensory_in"].kind == "input"
    assert by_name["sensory_in"].signal == "event"
    # a non-event signal: the drive is a continuous modulatory input,
    # exercising the M2 typed-port disciplines
    assert by_name["drive_in"].signal == "continuous"
    assert by_name["turn_out"].kind == "output"
    assert a.regions() == list(PORT_NAMES)
    assert a.port_groups() == {"upstream": ["sensory_in", "drive_in"],
                               "downstream": ["turn_out", "forward_out"]}
    desc = a.describe()
    assert desc["n_neurons"] == N_NEURONS == 48
    assert desc["internal_edges"] == [["sensor", "integrator"],
                                     ["integrator", "motor"]]
    res = a.resource_summary()
    assert res["n_neurons"] == 48 and res["n_regions"] == 3
    assert res["n_ports"] == 4 and res["n_internal_edges"] == 2


def test_reflex0_is_deterministic_and_gene_parameterised():
    a = Reflex0Adapter()
    l1 = a.lesion(0.25, 48, seed=99)
    l2 = a.lesion(0.25, 48, seed=99)
    l3 = a.lesion(0.25, 48, seed=100)
    assert l1.neuron_ids == l2.neuron_ids != l3.neuron_ids
    assert len(l1.neuron_ids) == 12
    assert all(0 <= i < 48 for i in l1.neuron_ids)
    a.reset(seed=3)
    snap = a.snapshot()
    a.restore(snap)
    with pytest.raises(ValueError):
        a.restore({"substrate_id": "fba0"})
    # per-genome parameterisation comes from the gene record
    gene = SubstrateGene(substrate_id="reflex0", kind=KIND,
                         params={"n_neurons": 24})
    assert Reflex0Adapter(gene).neuron_count() == 24


def test_reflex0_never_imports_the_fba_implementation():
    """The second substrate is independent — an FBA import inside the
    adapter would make the neutrality proof circular."""
    src = (Path(__file__).resolve().parents[1]
           / "substrate" / "reflex0.py").read_text()
    mods = set()
    for node in ast.walk(ast.parse(src)):
        if isinstance(node, ast.Import):
            mods.update(a.name for a in node.names)
        elif isinstance(node, ast.ImportFrom):
            mods.add(node.module or "")
    assert mods and all("fba" not in m for m in mods), mods


# ============================================================ Phase 3
# native-v4 development of the second substrate.
def test_native_v4_reflex0_genome_develops():
    phen = develop(_wired_reflex0(seed=7), topology_mode=GENERIC)
    assert phen["base"]["name"] == KIND
    assert phen["substrates"][0]["substrate_id"] == "reflex0"
    assert phen["substrates"][0]["kind"] == KIND
    assert phen["substrate_neurons"] == {"reflex0": 48}
    # reflex0's own parameter vocabulary — not the LIF defaults
    assert phen["params"]["gain"] == 1.0
    assert "wScale" not in phen["params"]
    # the organ IR the backends consume
    organ = phen["artificial_organs"][0]
    assert organ["organ_id"] == "org_a" and organ["size"] == 16
    assert organ["ports"] == {"in": {"direction": "input",
                                     "signal": "event"},
                              "out": {"direction": "output",
                                      "signal": "event"}}
    assert phen["attachments"][0]["signal"] == "event"
    # on the generic causal topology the organ is functional
    assert phen["structure"]["organs"]["org_a"] == "functional"
    assert phen["ancestry_fraction"] == 48 / 64
    assert phen["structural_ancestry_fraction"] == \
        phen["ancestry_fraction"]
    assert phen["fba0_structural_fraction"] is None


def test_develop_is_deterministic_and_never_writes_back():
    g = _wired_reflex0(seed=7)
    g.development_rules.append(
        {"op": "ADD_ORGAN_AT_BIRTH", "size": 8, "organ_id": "dev_0"})
    g = g.finalize()
    before = g.to_dict()
    a = develop(g, topology_mode=GENERIC)
    b = develop(g, topology_mode=GENERIC)
    assert a == b
    assert json.dumps(a, sort_keys=True) == json.dumps(b, sort_keys=True)
    # development adds a body the genome does not carry — and the genome
    # record itself is untouched (genome != mature phenotype)
    assert a["n_extra_neurons"] == 24
    assert g.to_dict() == before
    assert [o.organ_id for o in g.artificial_organs] == ["org_a"]


def test_develop_deep_copy_separation():
    """Rule-grown sizes land on phenotype copies: the gene's organ keeps
    its authored size (the §14 genome/product boundary)."""
    g = _wired_reflex0(seed=7)
    g.development_rules.append(
        {"op": "GROW_ORGAN_AT_BIRTH", "organ_id": "org_a", "delta": 40})
    g = g.finalize()
    phen = develop(g, topology_mode=GENERIC)
    assert phen["artificial_organs"][0]["size"] == 56
    assert g.artificial_organs[0].size == 16


def test_reflex0_organs_under_the_historical_topology():
    """Under the frozen M1 rule only the founder substrate is external:
    reflex0 endpoints are dangling there — the same answer M2's generic
    mode gives for a substrate the genome does not carry. The wiring
    filter itself stays honest either way (the attachment is dropped or
    kept by substrate membership, not by mode)."""
    g = _wired_reflex0(seed=7)
    rep = analyse(g, topology_mode=TOPOLOGY_M1_FBA0_LOOP)
    assert rep.dangling_attachments == ["in", "out"]
    assert rep.organs == {"org_a": "invalid_structure"}


# ============================================================ Phase 4
# the generic mutation machinery on the second substrate.
def test_endpoint_pool_comes_from_the_reflex0_adapter():
    """Mutation draws endpoints from the genome's adapters — for a
    reflex0 genome there is no FBA0 port in the pool."""
    g = _wired_reflex0(seed=3)
    eps = mut._endpoints(g)
    assert set(eps) == {
        "substrate:reflex0/sensory_in", "substrate:reflex0/drive_in",
        "substrate:reflex0/turn_out", "substrate:reflex0/forward_out",
        "org_a"}
    assert not any("fba0" in e for e in eps)
    up, down = mut._wiring_ports(g)
    assert up == ["substrate:reflex0/sensory_in",
                  "substrate:reflex0/drive_in"]
    assert down == ["substrate:reflex0/turn_out",
                    "substrate:reflex0/forward_out"]


def test_new_organ_wires_reflex0_source_to_sink():
    g = reflex0_genome(seed=3)
    rec = apply_operator(g, random.Random(2), "NEW_ORGAN",
                         merged_config(None), g.provenance())
    assert rec.outcome == "applied"
    assert rec.detail["source"].startswith("substrate:reflex0/")
    assert rec.detail["target"].startswith("substrate:reflex0/")
    phen = develop(g.finalize(), topology_mode=GENERIC)
    assert phen["structure"]["organs"][rec.target] == "functional"


def test_rewire_uses_the_adapter_endpoint_pool():
    g = _wired_reflex0(seed=3)
    for i in range(6):
        rec = apply_operator(g, random.Random(i), "REWIRE_ATTACHMENT",
                             merged_config(None), g.provenance())
    for att in g.attachments:
        for e in (att.source, att.target):
            assert e == "org_a" or e.startswith("substrate:reflex0/")


def test_mutate_replays_byte_identical_on_reflex0():
    parent = _wired_reflex0(seed=5)
    runs = []
    for _ in range(2):
        child, records = mutate(parent, random.Random(42), birth_index=3,
                                generation=2)
        d = child.to_dict()
        d.pop("created_at")                    # wallclock, not content
        runs.append((d, [r.to_dict() for r in records]))
    assert runs[0] == runs[1]


def test_staged_substrate_operator_applies_to_reflex0():
    """apply_operator reaches the staged M3 operators; the selection
    pool never draws them (pinned in test_m3_staged_operators)."""
    g = _wired_reflex0(seed=4)
    rec = apply_operator(g, random.Random(0), "DISABLE_SUBSTRATE_REGION",
                         merged_config(None), g.provenance())
    assert rec.outcome == "applied"
    sid, region = rec.target.split(":", 1)
    assert sid == "reflex0"
    assert region in g.substrates[0].params["disabled_regions"]
    # the port is gone from the gene's adapter
    adapter = default_registry().get("reflex0", g.substrates[0])
    assert region not in [p.name for p in adapter.ports()]
    assert region not in adapter.regions()


def test_genome_disabled_reflex0_region_drops_its_endpoints():
    """The staged operator's written effect: a port in
    ``disabled_regions`` is absent from the adapter and its endpoints
    are dangling — the same rule the founder's regions follow."""
    g = Genome(random_seed=4, substrates=[SubstrateGene(
        substrate_id="reflex0", kind=KIND,
        params={"disabled_regions": ["turn_out"]})])
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_a", kind="lif_cluster", size=8))
    g.attachments += [
        Attachment(attachment_id="in",
                   source="substrate:reflex0/sensory_in", target="org_a"),
        Attachment(attachment_id="out", source="org_a",
                   target="substrate:reflex0/turn_out")]
    g = g.finalize()
    adapter = default_registry().get("reflex0", g.substrates[0])
    assert "turn_out" not in [p.name for p in adapter.ports()]
    phen = develop(g, topology_mode=GENERIC)
    assert [a["attachment_id"] for a in phen["attachments"]] == ["in"]
    assert phen["structure"]["dangling_attachments"] == ["out"]
    assert phen["structure"]["organs"]["org_a"] == "invalid_structure"


def test_substrate_region_operator_tolerates_null_params():
    """Review obs #3: a gene loaded with ``"params": null`` must take
    the staged operator — the operator creates the params map."""
    g = Genome(substrates=[SubstrateGene(substrate_id="reflex0",
                                         kind=KIND, params=None)])
    rec = apply_operator(g, random.Random(0), "DISABLE_SUBSTRATE_REGION",
                         merged_config(None), g.provenance())
    assert rec.outcome == "applied"
    assert isinstance(g.substrates[0].params, dict)


def test_unknown_substrate_id_fails_loud_at_development():
    g = Genome(substrates=[SubstrateGene(substrate_id="nosuch",
                                         kind="k")])
    with pytest.raises(UnknownSubstrate):
        develop(g.finalize())


def test_duplicate_substrate_ids_fail_loud():
    """Two genes naming the same substrate cannot be represented —
    ``substrate_neurons`` is keyed by id — so development refuses."""
    g = Genome(substrates=[SubstrateGene(substrate_id="reflex0",
                                         kind=KIND),
                           SubstrateGene(substrate_id="reflex0",
                                         kind=KIND)])
    with pytest.raises(ValueError, match="more than once"):
        develop(g.finalize())


def test_endpoint_to_a_substrate_the_genome_lacks_is_dropped():
    g = reflex0_genome(seed=1)
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_a", kind="lif_cluster", size=8))
    g.attachments += [
        Attachment(attachment_id="in", source="substrate:nosuch/x",
                   target="org_a"),
        Attachment(attachment_id="out", source="org_a",
                   target="substrate:nosuch/y")]
    g = g.finalize()
    phen = develop(g, topology_mode=GENERIC)
    assert phen["attachments"] == []
    assert phen["structure"]["dangling_attachments"] == ["in", "out"]


def test_unknown_port_fails_loud_on_a_resolving_backend():
    """Port existence is the backend's contract: develop() keeps an
    attachment whose substrate is real but whose port is unknown, and
    a backend that resolves endpoints refuses it rather than wiring at
    random."""
    pytest.importorskip("torch")
    from experiments.mioba.fba.registry import get_backend
    g = reflex0_genome(seed=1)
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_a", kind="lif_cluster", size=8))
    g.attachments.append(
        Attachment(attachment_id="in",
                   source="substrate:reflex0/no_such_port",
                   target="org_a"))
    phen = develop(g.finalize(), topology_mode=GENERIC)
    assert phen["attachments"]                  # kept — see docstring
    b = get_backend("torch", synthetic=True, synthetic_neurons=64)
    with pytest.raises(ValueError, match="no_such_port"):
        b.initialize(phen, batch_size=1, seed=0, device="cpu")


# ============================================================ Phase 5
# the functional-departure battery on the second substrate.
_DEP_CFG = {
    "env": {"stim_fraction": 0.2, "stim_rate_hz": 200.0},
    "evaluation": {"target_rate_hz": 5.0},
    "functional_departure": {"enabled": True, "substrate_id": "reflex0",
                             "severities": [0.1, 0.5],
                             "controls": {"founder": True, "sham": True}},
}


def _dep_job(n_rep: int = 3) -> dict:
    return {"seed": 4242, "replicates": n_rep, "duration_ms": 100,
            "environment_id": "synthetic-quiet-v0", "config": _DEP_CFG}


def test_departure_battery_runs_all_conditions_on_reflex0():
    phen = develop(_wired_reflex0(seed=3), topology_mode=GENERIC)
    job = _dep_job()
    dep = evaluate_departure(MockBackend(n_neurons=48), phen, job,
                             _DEP_CFG,
                             seeds=replicate_seeds(job["seed"], 3))
    assert set(dep["conditions"]) >= {
        "intact", "sham", "founder", "organ_ablation",
        "reflex0_lesion_0.10", "reflex0_lesion_0.50"}
    assert dep["substrate_id"] == "reflex0"
    assert dep["conditions"]["reflex0_lesion_0.10"]["n_silenced"] == 5
    assert dep["conditions"]["reflex0_lesion_0.50"]["n_silenced"] == 24
    assert dep["conditions"]["organ_ablation"]["n_silenced"] == 16
    # the negative control: severity-0 mask through the real adapter,
    # same result as intact
    assert dep["sham_score"] == dep["intact_score"]
    assert dep["conditions"]["sham"]["n_silenced"] == 0
    # raw + derived metrics are all present under the substrate's own id
    assert set(dep["reflex0_lesion_loss"]) == {"0.10", "0.50"}
    assert dep["reflex0_dependency"] is not None
    assert dep["reflex0_dependency_normalized"] is not None
    assert dep["reflex0_lesion_10_score"] == \
        dep["conditions"]["reflex0_lesion_0.10"]["task_score"]
    assert dep["reflex0_lesion_50_score"] == \
        dep["conditions"]["reflex0_lesion_0.50"]["task_score"]
    assert dep["structural_ancestry_fraction"] == \
        phen["structural_ancestry_fraction"]


def test_reflex0_lesion_changes_measured_function():
    """Simple does not mean insensitive: silencing half the substrate
    measurably moves the score (no special-casing — this is the mock
    LIF population losing driven neurons)."""
    phen = develop(_wired_reflex0(seed=3), topology_mode=GENERIC)
    job = _dep_job()
    dep = evaluate_departure(MockBackend(n_neurons=48), phen, job,
                             _DEP_CFG,
                             seeds=replicate_seeds(job["seed"], 3))
    intact = dep["conditions"]["intact"]["mean_rate_hz"]
    lesioned = dep["conditions"]["reflex0_lesion_0.50"]["mean_rate_hz"]
    assert intact > 0
    assert lesioned != intact
    assert dep["reflex0_lesion_loss"]["0.50"] != 0.0


def test_reflex0_departure_is_deterministic_per_seed():
    phen = develop(_wired_reflex0(seed=3), topology_mode=GENERIC)
    job = _dep_job()
    a = evaluate_departure(MockBackend(n_neurons=48), phen, job,
                           _DEP_CFG,
                           seeds=replicate_seeds(job["seed"], 3))
    b = evaluate_departure(MockBackend(n_neurons=48), phen, job,
                           _DEP_CFG,
                           seeds=replicate_seeds(job["seed"], 3))
    assert a == b


def test_reflex0_founder_is_the_substrate_alone():
    """The founder condition must equal evaluating the plain reflex0
    genome under the same seeds."""
    job = _dep_job()
    seeds = replicate_seeds(job["seed"], 3)
    dep = evaluate_departure(MockBackend(n_neurons=48),
                             develop(_wired_reflex0(seed=3),
                                     topology_mode=GENERIC),
                             job, _DEP_CFG, seeds=seeds)
    founder = evaluate_departure(MockBackend(n_neurons=48),
                                 develop(reflex0_genome(seed=job["seed"]),
                                         topology_mode=GENERIC),
                                 job, _DEP_CFG, seeds=seeds)
    assert dep["founder_score"] == founder["intact_score"]


# ============================================================ Phase 6
# one evaluation pipeline for both substrates (CPU mock backend).
def _eval_job(genome: Genome, config: dict | None = None) -> dict:
    seed = int(genome.random_seed)
    return {"job_id": "j", "seed": seed, "replicates": 2,
            "replicate_seeds": replicate_seeds(seed, 2),
            "duration_ms": 50.0,
            "environment_id": "synthetic-quiet-v0",
            "config": config or {}}


def test_one_pipeline_evaluates_fba0_and_reflex0_alike():
    """Same develop(), same evaluate_replicates(), same summary shape —
    the only difference is the substrate id recorded under activity."""
    cfg = {"env": {"stim_fraction": 0.2, "stim_rate_hz": 200.0}}
    out = {}
    for g in (fba0_genome(seed=5), _wired_reflex0(seed=5)):
        phen = develop(g, topology_mode=GENERIC)
        rep = evaluate_replicates(MockBackend(n_neurons=48), phen,
                                  _eval_job(g, cfg), "cpu",
                                  execution_batch=2)
        out[g.substrates[0].substrate_id] = rep["summary"]
    for sid, summary in out.items():
        assert summary["spikes_total"] > 0
        assert summary["mean_rate_hz"] > 0
        assert sid in summary["activity"], summary["activity"].keys()
    assert "fba0" not in out["reflex0"]["activity"]


def _reflex0_eval(tmp_path, cfg, substrate_id="reflex0", seed=5):
    """Record one reflex0 evaluation through the coordinator + worker —
    the same path every genome takes."""
    from fastapi.testclient import TestClient

    from experiments.mioba.coordinator.app import create_app
    from experiments.mioba.coordinator.service import MiobaService
    from experiments.mioba.tests.conftest import run_worker_once

    cfg = copy.deepcopy(cfg)
    cfg["population"]["target_size"] = 1      # one fba0 founder job too
    svc = MiobaService(cfg, tmp_path / "runs")
    client = TestClient(create_app(svc))
    client.post("/api/worker/register",
                json={"worker_id": "w", "hostname": "h", "gpu": [],
                      "runtime_info": {}, "bench": [], "batch_size": 1})
    g = _wired_reflex0(seed=seed)
    svc.db.insert_genome(svc.experiment_id, g, "import",
                         structure=mut.structural_summary(g))
    # higher priority than the seeded founder job: claimed first
    svc.db.enqueue_job(svc.experiment_id, g.genome_id,
                       "synthetic-quiet-v0", int(g.random_seed), "smoke",
                       "mock", 100.0, [], priority=10,
                       replicates=int(cfg["evaluation"]["replicates"]))
    body = run_worker_once(client, "w")
    return svc, body, g


def test_reflex0_evaluation_runs_end_to_end(tmp_path, smoke_config):
    svc, body, g = _reflex0_eval(tmp_path, smoke_config)
    assert body["status"] == "SUCCEEDED"
    assert body["delivery"] == "accepted"
    ev = svc.db.list_evaluations(svc.experiment_id, limit=1)[0]
    assert ev["genome_id"] == g.genome_id
    summary = json.loads(ev["summary_json"])
    assert summary["spikes_total"] > 0
    # the activity record names the actual substrate, not a hardcoded fba0
    assert "reflex0" in summary["activity"]
    svc.db.close()


def test_reflex0_departure_battery_runs_in_the_worker_path(
        tmp_path, smoke_config):
    cfg = copy.deepcopy(smoke_config)
    cfg["functional_departure"] = {
        "enabled": True, "substrate_id": "reflex0",
        "severities": [0.25], "controls": {"founder": True, "sham": True}}
    cfg["env"] = {"stim_fraction": 0.2, "stim_rate_hz": 200.0}
    svc, body, g = _reflex0_eval(tmp_path, cfg)
    assert body["status"] == "SUCCEEDED"
    dep = json.loads(svc.db.list_evaluations(svc.experiment_id, limit=1)
                     [0]["summary_json"])["departure"]
    assert dep["substrate_id"] == "reflex0"
    assert dep["sham_score"] == dep["intact_score"]
    assert dep["conditions"]["reflex0_lesion_0.25"]["n_silenced"] > 0
    metrics = json.loads(svc.db.list_evaluations(svc.experiment_id,
                                                 limit=1)[0]
                       ["metrics_json"])
    assert metrics["departure_resistance"] is not None
    svc.db.close()


# ============================================================ Phase 7
# reproduction: parent -> mutation -> child, on the second substrate.
def test_reflex0_reproduction_produces_a_native_v4_child():
    parent = _wired_reflex0(seed=9)
    before = parent.to_dict()
    child, records = mutate(parent, random.Random(7), birth_index=0,
                            generation=1)
    # the child is a native v4 record carrying the parent's substrate
    assert child.schema_version == 4
    assert child.source_schema_version is None
    assert child.parent_ids == [parent.genome_id]
    assert child.generation == 1 and child.birth_index == 0
    assert [s.substrate_id for s in child.substrates] == ["reflex0"]
    # the parent is untouched and the mutation attempts are recorded
    assert parent.to_dict() == before
    assert records and all(r.mutation_id for r in records)
    # the child develops and evaluates on the same machinery
    phen = develop(child, topology_mode=GENERIC)
    assert phen["substrates"][0]["substrate_id"] == "reflex0"
    rep = evaluate_replicates(MockBackend(n_neurons=48), phen,
                              _eval_job(child), "cpu", execution_batch=1)
    assert rep["summary"]["spikes_total"] >= 0


def test_reflex0_lineage_persists_and_replays(tmp_path):
    """Stored lineage: parent + child + mutation rows, and a re-drawn
    child under the same RNG state is byte-identical (the M0/M1 replay
    contract, now on reflex0)."""
    from experiments.mioba.storage.db import Database
    db = Database(tmp_path / "lineage.sqlite")
    db.create_experiment("EXP", {}, "cfg", None)
    parent = _wired_reflex0(seed=9)
    db.insert_genome("EXP", parent, "import")
    runs = []
    for _ in range(2):
        child, records = mutate(parent, random.Random(7), birth_index=0,
                                generation=1)
        d = child.to_dict()
        d.pop("created_at")
        runs.append((d, [r.to_dict() for r in records]))
    assert runs[0] == runs[1]
    child, records = mutate(parent, random.Random(7), birth_index=0,
                            generation=1)
    db.insert_genome("EXP", child, "mutation", mutations=records)
    assert db.genome_parents(child.genome_id) == [parent.genome_id]
    assert db.genome_children(parent.genome_id) == [child.genome_id]
    stored = {m["mutation_id"] for m in db.genome_mutations(child.genome_id)}
    assert stored == {r.mutation_id for r in records}
    # the stored child reloads as a native-v4 reflex0 record
    row = db.get_genome(child.genome_id)
    again = Genome.from_json(row["genome_json"])
    assert again.schema_version == 4
    assert [s.substrate_id for s in again.substrates] == ["reflex0"]
    db.close()


# ============================================================ Phase 8
# replay of a recorded reflex0 evaluation.
def test_reflex0_evaluation_replays_identically(tmp_path, smoke_config):
    from experiments.mioba.coordinator.replay import build_plan, run_plan
    svc, body, g = _reflex0_eval(tmp_path, smoke_config)
    ev = svc.db.list_evaluations(svc.experiment_id, limit=1)[0]
    exp_dir = svc.run_dir
    svc.db.close()
    plan = build_plan(exp_dir, ev["evaluation_id"],
                      current_config=smoke_config)
    assert plan.backend == "mock" and plan.seed == ev["seed"]
    res = run_plan(plan)
    assert res["diff"]["identical_spike_counts"] is True
    assert res["diff"]["original_spikes"] == res["diff"]["replay_spikes"]
    assert res["identity"]["genome_hash"] == g.genome_id


def test_replay_detects_substrate_identity_drift(tmp_path, smoke_config):
    """The stored genome_hash binds the substrate identity: rewrite the
    genome row to a different substrate and the replay refuses — the
    recorded evaluation and the record no longer describe one organism."""
    import sqlite3

    from experiments.mioba.coordinator.replay import (ReplayUnavailable,
                                                      build_plan)
    svc, body, g = _reflex0_eval(tmp_path, smoke_config)
    ev = svc.db.list_evaluations(svc.experiment_id, limit=1)[0]
    other = fba0_genome(seed=5)                 # a different substrate
    con = sqlite3.connect(str(svc.run_dir / "lineage.sqlite"))
    con.execute("UPDATE genomes SET genome_json=? WHERE genome_id=?",
                (other.to_json(), g.genome_id))
    con.commit()
    con.close()
    exp_dir = svc.run_dir
    svc.db.close()
    with pytest.raises(ReplayUnavailable, match="genome_hash"):
        build_plan(exp_dir, ev["evaluation_id"])


def test_reflex0_replay_config_drift_is_flagged(tmp_path, smoke_config):
    from experiments.mioba.coordinator.replay import (ReplayConfigMismatch,
                                                      build_plan)
    svc, body, g = _reflex0_eval(tmp_path, smoke_config)
    ev = svc.db.list_evaluations(svc.experiment_id, limit=1)[0]
    exp_dir = svc.run_dir
    svc.db.close()
    drift = copy.deepcopy(smoke_config)
    drift["evaluation"]["duration_ms"] = 999
    plan = build_plan(exp_dir, ev["evaluation_id"], current_config=drift)
    assert any("scientific_config_hash differs" in w
               for w in plan.warnings)
    with pytest.raises(ReplayConfigMismatch):
        build_plan(exp_dir, ev["evaluation_id"], current_config=drift,
                   strict=True)


# ============================================================ Phase 9
# the audit tripwire: no substrate-specific branch may appear in the
# generic layer.
def test_no_reflex0_branch_in_the_generic_layer():
    """reflex0 is reachable *only* through registry registration: the
    adapter module itself and the registry that lists it. Everywhere
    else the name must not appear — that is the whole claim of M3."""
    root = Path(__file__).resolve().parents[1]
    allowed = {root / "substrate" / "reflex0.py",
               root / "substrate" / "registry.py",
               root / "tests" / "test_m3_reflex0.py"}
    offenders = []
    for path in root.rglob("*.py"):
        if "__pycache__" in path.parts or path in allowed:
            continue
        if "reflex0" in path.read_text():
            offenders.append(path.relative_to(root))
    assert offenders == []


# ============================================================ hardening
def test_randomized_small_genomes_obey_the_contract():
    """Property-style: 200 randomized genomes — mixed substrate ids,
    disabled genes, null params, bogus endpoints — must either develop
    deterministically or fail with a contract-typed error; save/load,
    structure analysis and mutation replay never produce an untyped
    crash."""
    rng = random.Random(20260912)
    sids = ("fba0", "reflex0", "nosuch")
    port_pool = ([f"substrate:reflex0/{n}" for n in PORT_NAMES]
                 + ["fba0:medulla", "fba0:bogus",
                    "substrate:nosuch/x", "env:habitat/forage",
                    "org_none"])
    for i in range(200):
        g = Genome(random_seed=rng.randrange(1 << 30))
        g.substrates = [
            SubstrateGene(substrate_id=rng.choice(sids), kind="k",
                          enabled=rng.random() > 0.3,
                          params=rng.choice(
                              [None, {}, {"n_neurons": 24},
                               {"disabled_regions": ["turn_out"]}]))
            for _ in range(rng.randrange(0, 3))]
        for j in range(rng.randrange(0, 3)):
            g.artificial_organs.append(
                ArtificialOrgan(organ_id=f"org_{j}", kind="lif_cluster",
                                size=rng.randrange(4, 32),
                                enabled=rng.random() > 0.2))
        pool = port_pool + [o.organ_id for o in g.artificial_organs]
        for j in range(rng.randrange(0, 5)):
            g.attachments.append(
                Attachment(attachment_id=f"a{j}",
                           source=rng.choice(pool),
                           target=rng.choice(pool)))
        g = g.finalize()

        # save/load is lossless
        assert Genome.from_json(g.to_json()).to_dict() == g.to_dict()

        # the structural record never crashes — it reports
        rep = analyse(g, topology_mode=GENERIC)
        assert isinstance(rep.to_dict()["counts"], dict)

        # development is either a deterministic phenotype or a
        # contract-typed refusal — never an untyped crash
        try:
            phen = develop(g, topology_mode=GENERIC)
        except (NoEnabledSubstrate, UnknownSubstrate, ValueError):
            phen = None
        if phen is not None:
            assert phen == develop(g, topology_mode=GENERIC)

        # mutation replay: same parent + same RNG state = same child
        c1, r1 = mutate(g, random.Random(i), birth_index=0, generation=1)
        c2, r2 = mutate(g, random.Random(i), birth_index=0, generation=1)
        assert c1.genome_id == c2.genome_id
        assert [x.to_dict() for x in r1] == [x.to_dict() for x in r2]

    # lesion masks stay deterministic for every registered substrate
    for sid in ("fba0", "reflex0"):
        a = default_registry().get(sid)
        assert a.lesion(0.3, 40, 7).neuron_ids == \
            a.lesion(0.3, 40, 7).neuron_ids
