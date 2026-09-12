"""M1 §2.4: simulator versioning, seed separation, scheduling semantics.

These pin the rules that keep the speed work honest:

- a recording says which simulator produced it, and a different one
  refuses to replay it rather than producing a plausible wrong number;
- how the run was *scheduled* (slots, execution batch, which worker)
  never changes a result;
- a device resource limit is a scheduling event, never a dead organism;
- an individual is charged for the circuit it added, not for the shared
  substrate it was born into.
"""
from __future__ import annotations

import copy
import json

import pytest

from experiments.mioba.coordinator.replay import ReplayUnavailable
from experiments.mioba.development.phenotype import develop
from experiments.mioba.fba import semantics as sem_mod
from experiments.mioba.fba.seeds import (derive, disturbance_seed,
                                         environment_seed, noise_seed,
                                         seed_manifest, structure_seed)
from experiments.mioba.fba.semantics import (SemanticsMismatch,
                                             assert_replayable,
                                             check_replayable, semantics,
                                             unversioned_semantics)
from experiments.mioba.genome.hashing import scientific_config_hash
from experiments.mioba.genome.schema import fba0_genome
from experiments.mioba.storage import models as M
from experiments.mioba.workers import worker as W

from .conftest import run_worker_once


# ------------------------------------------------------------ versioning
def test_semantics_are_recorded_on_every_evaluation(client, service):
    body = run_worker_once(client, "w1")
    assert body["status"] == "SUCCEEDED"
    ev = service.db.list_evaluations(service.experiment_id, limit=1)[0]
    # the smoke config runs the mock backend, which has its own (never
    # revised) semantics rather than the torch backend's history
    assert ev["simulator_semantics_version"] == 1
    assert ev["rng_protocol_version"] == 1
    assert ev["propagation_backend"] == "mock"


def test_torch_backend_declares_m1_semantics():
    torch = pytest.importorskip("torch")
    del torch
    from experiments.mioba.fba.torch_backend import TorchBackend

    b = TorchBackend(synthetic=True, synthetic_neurons=100)
    assert b.semantics() == {
        "simulator_semantics_version": 3,
        "rng_protocol_version": 2,
        "propagation_backend": "event_csc",
    }
    d = TorchBackend(synthetic=True, synthetic_neurons=100,
                     propagation_backend="sparse_csr")
    assert d.semantics()["propagation_backend"] == "sparse_csr"


def test_an_untagged_recording_is_treated_as_m0_and_refused():
    """A row with no semantics columns predates M1 tagging; it must be
    reported as M0, not assumed compatible."""
    problems = check_replayable(None, semantics())
    assert len(problems) == 2
    assert all("inferred" in p for p in problems)
    with pytest.raises(SemanticsMismatch):
        assert_replayable({}, semantics())
    # same-version recordings pass, whatever the propagation path
    assert check_replayable(semantics("event_csc"),
                            semantics("sparse_csr")) == []
    assert check_replayable(unversioned_semantics("mock"),
                            unversioned_semantics("mock")) == []


def test_replay_refuses_a_recording_from_another_simulator(tmp_path,
                                                           smoke_config):
    from experiments.mioba.coordinator.replay import build_plan, run_plan
    from experiments.mioba.coordinator.service import MiobaService
    from fastapi.testclient import TestClient
    from experiments.mioba.coordinator.app import create_app
    import sqlite3

    svc = MiobaService(copy.deepcopy(smoke_config), tmp_path / "runs")
    cl = TestClient(create_app(svc))
    run_worker_once(cl, "w1")
    ev = svc.db.list_evaluations(svc.experiment_id, limit=1)[0]
    exp_dir = svc.run_dir
    svc.db.close()

    con = sqlite3.connect(str(exp_dir / "lineage.sqlite"))
    con.execute("UPDATE evaluations SET simulator_semantics_version=99 "
                "WHERE evaluation_id=?", (ev["evaluation_id"],))
    con.commit()
    con.close()

    plan = build_plan(exp_dir, ev["evaluation_id"])
    assert plan.semantics["simulator_semantics_version"] == 99
    with pytest.raises(ReplayUnavailable, match="simulator semantics"):
        run_plan(plan)


def test_semantics_are_part_of_the_scientific_config_hash(smoke_config,
                                                          monkeypatch):
    before = scientific_config_hash(smoke_config)
    monkeypatch.setattr(sem_mod, "SIMULATOR_SEMANTICS_VERSION", 99)
    after = scientific_config_hash(smoke_config)
    assert before != after, ("a simulator change must force a new "
                             "experiment id, not continue the old one")


def test_propagation_backend_choice_is_scientific(smoke_config):
    a = copy.deepcopy(smoke_config)
    b = copy.deepcopy(smoke_config)
    b.setdefault("fba", {})["propagation_backend"] = "sparse_csr"
    assert scientific_config_hash(a) != scientific_config_hash(b)


# ------------------------------------------------------------------ seeds
def test_seed_purposes_are_independent_and_deterministic():
    s = 20260911
    streams = {"environment": environment_seed(s),
               "structure": structure_seed(s),
               "noise0": noise_seed(s, 0), "noise1": noise_seed(s, 1),
               "dist0": disturbance_seed(s, 0),
               "dist1": disturbance_seed(s, 1)}
    assert len(set(streams.values())) == len(streams)
    assert environment_seed(s) == streams["environment"]     # pure function
    assert environment_seed(s + 1) != streams["environment"]
    # a purpose at index i never collides with another purpose at index i
    assert derive(s, "noise", 3) != derive(s, "disturbance", 3)
    with pytest.raises(ValueError):
        derive(s, "not-a-purpose")


def test_seed_manifest_covers_every_stream_of_an_evaluation():
    m = seed_manifest(7, 3)
    assert m["evaluation_seed"] == 7
    assert len(m["replicate_seeds"]) == 3
    assert len(m["noise_seeds"]) == 3
    assert len(m["disturbance_seeds"]) == 3
    # the replicate stream is distinct from the per-replicate noise and
    # disturbance streams at the same index
    for i in range(3):
        assert len({m["replicate_seeds"][i], m["noise_seeds"][i],
                    m["disturbance_seeds"][i]}) == 3


def test_slot_count_does_not_change_results():
    """§2.4-6: slots are a runtime optimisation. Same genome, same seeds,
    different concurrency -> identical summaries."""
    pytest.importorskip("torch")
    from experiments.mioba.perf.evalbench import founder_population, run_slots

    cfg = {"evaluation": {"backend": "torch", "duration_ms": 20,
                          "replicates": 2,
                          "environment_id": "synthetic-quiet-v0"},
           "fba": {"synthetic": True, "synthetic_neurons": 400,
                   "synthetic_edges": 2000, "base_seed": 5},
           "env": {"stim_fraction": 0.05, "stim_rate_hz": 200}}
    genomes = founder_population(4)
    one = run_slots(genomes, cfg, "cpu", slots=1)
    three = run_slots(genomes, cfg, "cpu", slots=3)
    assert one["succeeded"] == three["succeeded"] == 4

    def rates(rep):
        return [r["mean_rate_hz"] for r in rep["per_evaluation"]]

    assert rates(one) == rates(three)


def test_execution_batch_and_slots_are_runtime_not_scientific(smoke_config):
    from experiments.mioba.genome.hashing import runtime_config_hash

    a = copy.deepcopy(smoke_config)
    b = copy.deepcopy(smoke_config)
    b.setdefault("worker", {})["slots"] = 3
    assert scientific_config_hash(a) == scientific_config_hash(b)
    assert runtime_config_hash(a) != runtime_config_hash(b)


# -------------------------------------------------------- resource limits
def test_oom_is_a_scheduling_retry_not_a_dead_individual(client, service,
                                                         monkeypatch):
    """§2.4-7: a GPU OOM must requeue the job under the same evaluation
    seed and must not count as a failed evaluation."""
    def boom(*a, **k):
        raise RuntimeError("CUDA out of memory. Tried to allocate 2.00 GiB")

    monkeypatch.setattr(W, "evaluate_replicates", boom)
    r = client.post("/api/worker/claim",
                    json={"worker_id": "w1", "batch_size": 1})
    job = r.json()
    body = W.run_job(client, "w1", job, "cpu", 1)
    assert body["status"] == "RETRY"
    assert body["retry_reason"] == M.RETRY_RUNTIME_RESOURCE

    types = [e["type"] for e in service.db.list_events(service.experiment_id,
                                                       0, 1000)]
    assert M.EV_RUNTIME_RESOURCE_RETRY in types
    assert M.EV_EVALUATION_FAILED not in types
    assert service.counters()["evaluations_failed"] == 0

    ev = next(e for e in service.db.list_events(service.experiment_id, 0, 1000)
              if e["type"] == M.EV_RUNTIME_RESOURCE_RETRY)
    assert json.loads(e_payload(ev))["fitness_affected"] is False

    # the job goes back to the queue with its evaluation seed intact, so
    # the re-run is the same experiment, not a different draw
    from experiments.mioba.coordinator import jobs as jobs_mod

    row = service.db.get_job(job["job_id"])
    assert row["status"] in (M.JOB_UNKNOWN, M.JOB_QUEUED)
    assert row["seed"] == job["seed"]
    list(jobs_mod.requeue(service.db, service.experiment_id, service.config))
    assert service.db.get_job(job["job_id"])["status"] == M.JOB_QUEUED
    claimed = None
    for _ in range(50):
        resp = client.post("/api/worker/claim",
                           json={"worker_id": "w1", "batch_size": 1})
        if resp.status_code != 200:
            break
        if resp.json()["job_id"] == job["job_id"]:
            claimed = resp.json()
            break
    assert claimed is not None, "the requeued job was never handed out again"
    assert claimed["seed"] == job["seed"]
    assert claimed["replicate_seeds"] == job["replicate_seeds"]


def e_payload(event: dict) -> str:
    return event.get("payload_json") or json.dumps(event.get("payload") or {})


def test_non_resource_failures_are_still_failures(client, service,
                                                  monkeypatch):
    def boom(*a, **k):
        raise ValueError("genome is nonsense")

    monkeypatch.setattr(W, "evaluate_replicates", boom)
    r = client.post("/api/worker/claim",
                    json={"worker_id": "w1", "batch_size": 1})
    body = W.run_job(client, "w1", r.json(), "cpu", 1)
    assert body["status"] == M.JOB_FAILED and body["retry_reason"] is None
    assert service.counters()["evaluations_failed"] == 1


def test_slot_governor_backs_off_one_way():
    gov = W.SlotGovernor(3)
    assert gov.limit == 3
    assert gov.back_off("cuda_oom") == 2
    assert gov.back_off("cuda_oom") == 1
    assert gov.back_off("cuda_oom") == 1          # never below the minimum
    assert [b["applied"] for b in gov.backoffs] == [True, True, False]


# ------------------------------------------------------------ activity
def test_activity_and_marginal_cost_are_reported():
    pytest.importorskip("torch")
    from experiments.mioba.fba.torch_backend import TorchBackend
    from experiments.mioba.genome.schema import ArtificialOrgan, Attachment
    from experiments.mioba.mie.environments import make_drive

    g = fba0_genome()
    organ = ArtificialOrgan(organ_id="org_a", kind="lif_cluster", size=32)
    g.artificial_organs.append(organ)
    g.attachments.append(Attachment(attachment_id="att_a",
                                    source="fba0:medulla",
                                    target=organ.organ_id))
    n = 700
    b = TorchBackend(synthetic=True, synthetic_neurons=n,
                     synthetic_edges=4000)
    b.initialize(develop(g.finalize(), base_neurons=n), batch_size=1, seed=1,
                 device="cpu")
    b.set_inputs(make_drive("synthetic-quiet-v0", b.n_base,
                            {"env": {"stim_fraction": 0.2,
                                     "stim_rate_hz": 500}}))
    b.run(20)

    act = b.activity_stats()
    assert act["steps"] > 0
    assert act["active_neurons_per_step"] > 0
    assert 0 < act["active_edge_ratio"] < 1
    assert act["propagation_backend"] == "event_csc"

    res = b.marginal_resource_cost()
    # only the artificial circuit is charged: 32 neurons, not 732
    assert res["artificial_neuron_count"] == 32
    assert res["artificial_edge_count"] == b.nnz_extra
    assert res["marginal_memory_cost_bytes"] > 0
    assert res["genome_bytes"] > 0
    assert res["per_organ_buffer_bytes"]["org_a"] > 0
    # the shared base is not part of the individual's cost
    base_only = TorchBackend(synthetic=True, synthetic_neurons=n,
                             synthetic_edges=4000)
    base_only.initialize(develop(fba0_genome(), base_neurons=n), batch_size=1,
                         seed=1, device="cpu")
    assert base_only.marginal_resource_cost()[
        "marginal_memory_cost_bytes"] == 0


def test_high_activity_individual_is_flagged_not_penalised(client, service,
                                                           monkeypatch):
    """§2.4-4: a hyperactive organism slows its worker down; that must be
    visible as an event and must not touch its fitness."""
    real = W.evaluate_replicates

    def loud(*a, **k):
        rep = real(*a, **k)
        rep["summary"]["propagation_activity"] = {
            "active_edge_ratio": 0.9,
            "active_edges_per_step": 1e6,
            "active_neurons_per_step": 5000,
            "propagation_backend": "event_csc"}
        return rep

    monkeypatch.setattr(W, "evaluate_replicates", loud)
    body = run_worker_once(client, "w1")
    assert body["status"] == "SUCCEEDED"
    events = service.db.list_events(service.experiment_id, 0, 1000)
    hot = [e for e in events if e["type"] == M.EV_HIGH_ACTIVITY]
    assert len(hot) == 1
    payload = json.loads(e_payload(hot[0]))
    assert payload["active_edge_ratio"] == 0.9
    assert payload["fitness_affected"] is False
    ev = service.db.list_evaluations(service.experiment_id, limit=1)[0]
    assert ev["fitness"] is not None      # scored normally


def test_hybrid_switch_matches_the_event_path():
    """The dense fallback must compute the same sum as the event path."""
    torch = pytest.importorskip("torch")
    from experiments.mioba.fba.eventgraph import EventGraph

    gen = torch.Generator().manual_seed(11)
    n, m, b = 48, 300, 2
    graph = EventGraph.from_coo(torch.randint(0, n, (m,), generator=gen),
                                torch.randint(0, n, (m,), generator=gen),
                                torch.rand(m, generator=gen), n, n)
    spikes = (torch.rand((b, n), generator=gen) < 0.6).float()
    ev_out, dn_out = torch.zeros((b, n)), torch.zeros((b, n))
    events_e, edges_e = graph.propagate(spikes, ev_out, dense_above=None)
    events_d, edges_d = graph.propagate(spikes, dn_out, dense_above=0.1)
    assert torch.allclose(ev_out, dn_out, atol=1e-5)
    assert events_e == events_d           # same events, different path
    assert edges_d == graph.nnz           # dense touches the whole matrix


@pytest.mark.slow
def test_activity_break_even_reports_both_paths():
    pytest.importorskip("torch")
    from experiments.mioba.perf.evalbench import activity_break_even

    cfg = {"evaluation": {"backend": "torch", "duration_ms": 5,
                          "environment_id": "synthetic-quiet-v0"},
           "fba": {"synthetic": True, "synthetic_neurons": 300,
                   "synthetic_edges": 1500, "base_seed": 1},
           "env": {}}
    rep = activity_break_even(cfg, "cpu", stim_rates_hz=(50.0, 500.0),
                              duration_ms=5.0)
    assert len(rep["rows"]) == 2
    for row in rep["rows"]:
        assert row["event_wall_s"] > 0 and row["dense_wall_s"] > 0
        assert row["active_edge_ratio"] >= 0
