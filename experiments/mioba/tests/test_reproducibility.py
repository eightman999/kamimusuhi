"""Blockers 4, 6, 10: deterministic mutation/evolution, atomic
job-success + evaluation, scientific vs runtime config identity."""
import copy
import json
import random

import pytest

from experiments.mioba.coordinator.service import (MiobaService,
                                                   ScientificConfigMismatch)
from experiments.mioba.genome.hashing import (runtime_config_hash,
                                              scientific_config_hash)
from experiments.mioba.genome.mutation import mutate_child as mutate
from experiments.mioba.genome.schema import fba0_genome
from experiments.mioba.storage import models as M
from experiments.mioba.tests.conftest import run_worker_once


# ------------------------------------------------------------ blocker 4
def _lineage_fingerprint(g):
    return (g.genome_id, g.parent_ids, g.generation, g.birth_index,
            g.random_seed,
            [(m.mutation_id, m.path, m.op, m.value)
             for m in g.parameter_mutations],
            [(o.organ_id, o.size, o.provenance.birth_mutation_id)
             for o in g.artificial_organs],
            [(a.attachment_id, a.source, a.target, a.direction,
              a.weight_scale) for a in g.attachments])


def test_mutate_is_deterministic_given_rng_state():
    base = fba0_genome(seed=7)
    runs = []
    for _ in range(2):
        rng = random.Random(2024)
        parent, chain = base, []
        for gen in range(1, 40):
            child = mutate(parent, rng, birth_index=gen % 3, generation=gen)
            chain.append(_lineage_fingerprint(child))
            parent = child
        runs.append(chain)
    assert runs[0] == runs[1]
    assert any(c[6] for c in runs[0]), "no organ mutation in 40 gens"
    ids = [c[0] for c in runs[0]]
    assert len(set(ids)) == len(ids)


def test_mutation_ids_do_not_depend_on_wallclock_or_uuid(monkeypatch):
    import experiments.mioba.genome.mutation as mut
    assert not hasattr(mut, "uuid")
    base = fba0_genome(seed=1)
    a = mutate(base, random.Random(3), 0, 1)
    b = mutate(base, random.Random(3), 0, 1)
    da, db_ = a.to_dict(), b.to_dict()
    da.pop("created_at"), db_.pop("created_at")  # excluded from the hash
    assert da == db_ and a.genome_id == b.genome_id


def _run_experiment(tmp_path, cfg, name, generations):
    svc = MiobaService(cfg, tmp_path / name)
    from fastapi.testclient import TestClient
    from experiments.mioba.coordinator.app import create_app
    client = TestClient(create_app(svc))
    client.post("/api/worker/register",
                json={"worker_id": "w", "hostname": "h", "gpu": [],
                      "runtime_info": {}, "bench": [], "batch_size": 1})
    for _ in range(generations):
        while run_worker_once(client, "w") is not None:
            pass
        svc._counters["births"] += svc.population.maybe_advance()
    rows = svc.db.list_genomes(svc.experiment_id, limit=10000)
    out = [(r["generation"], r["birth_index"], r["genome_id"],
            json.loads(r["parent_ids_json"])) for r in rows]
    genomes = {r["genome_id"]: json.loads(r["genome_json"]) for r in rows}
    svc.db.close()
    return sorted(out), genomes


def test_two_runs_same_seed_identical_lineage(tmp_path, smoke_config):
    cfg = copy.deepcopy(smoke_config)
    cfg["evolution"]["max_generations"] = 3
    cfg["population"]["target_size"] = 6
    a, ga = _run_experiment(tmp_path, cfg, "a", 3)
    b, gb = _run_experiment(tmp_path, cfg, "b", 3)
    assert a == b
    assert {g for g, *_ in a} == {0, 1, 2, 3}
    for gid in ga:
        da, db_ = dict(ga[gid]), dict(gb[gid])
        da.pop("created_at"), db_.pop("created_at")
        assert da == db_


# ------------------------------------------------------------ blocker 6
def _claimed_job(client, service):
    client.post("/api/worker/register",
                json={"worker_id": "w1", "hostname": "h", "gpu": [],
                      "runtime_info": {}, "bench": [], "batch_size": 1})
    r = client.post("/api/worker/claim", json={"worker_id": "w1"})
    assert r.status_code == 200
    return r.json()["job_id"]


EVAL = {"backend": "mock", "seed": 1, "batch_size": 1,
        "summary": {"mean_rate_hz": 4.0}, "dataset": {"dataset_id": "mock-fba"}}


def _assert_consistent(service, job_id):
    job = service.db.get_job(job_id)
    evs = service.db.list_evaluations(service.experiment_id, limit=1000)
    evs = [e for e in evs if e["job_id"] == job_id]
    if job["status"] == M.JOB_SUCCEEDED:
        assert len(evs) == 1 and evs[0]["summary_json"]
    else:
        assert job["status"] in (M.JOB_RUNNING, M.JOB_UNKNOWN, M.JOB_QUEUED,
                                 M.JOB_FAILED)
        assert evs == []


@pytest.mark.parametrize("fail_at", ["before_insert", "after_insert"])
def test_crash_around_evaluation_insert_never_leaves_partial_success(
        client, service, monkeypatch, fail_at):
    job_id = _claimed_job(client, service)
    db = service.db
    real_insert = db.insert_evaluation

    def boom_insert(*a, **k):
        if fail_at == "before_insert":
            raise RuntimeError("injected before insert")
        eid = real_insert(*a, **k)
        raise RuntimeError("injected after insert")
    monkeypatch.setattr(db, "insert_evaluation", boom_insert)

    with pytest.raises(RuntimeError, match="injected"):
        service.worker_result(job_id, "w1", M.JOB_SUCCEEDED, EVAL, None)

    _assert_consistent(service, job_id)
    job = db.get_job(job_id)
    assert job["status"] == M.JOB_RUNNING          # recoverable, not lost
    assert db.get_worker(service.experiment_id, "w1")["completed_jobs"] == 0
    assert db.list_events(service.experiment_id,
                          type_="evaluation_succeeded") == []
    # a fresh sqlite connection sees the same (rolled back) state
    import sqlite3
    con = sqlite3.connect(str(service.db_path))
    assert con.execute("SELECT status FROM evaluation_jobs WHERE job_id=?",
                       (job_id,)).fetchone()[0] == M.JOB_RUNNING
    assert con.execute("SELECT COUNT(*) FROM evaluations").fetchone()[0] == 0
    con.close()

    # the worker (or a requeue) can still complete the job afterwards
    monkeypatch.setattr(db, "insert_evaluation", real_insert)
    service.worker_result(job_id, "w1", M.JOB_SUCCEEDED, EVAL, None)
    _assert_consistent(service, job_id)
    assert db.get_job(job_id)["status"] == M.JOB_SUCCEEDED
    assert db.get_worker(service.experiment_id, "w1")["completed_jobs"] == 1


def test_success_without_evaluation_is_recorded_as_failure(client, service):
    job_id = _claimed_job(client, service)
    service.worker_result(job_id, "w1", M.JOB_SUCCEEDED, None, None)
    job = service.db.get_job(job_id)
    assert job["status"] == M.JOB_FAILED
    _assert_consistent(service, job_id)


def test_evaluation_record_carries_replay_identity(client, service):
    client.post("/api/worker/register",
                json={"worker_id": "w1", "hostname": "h", "gpu": [],
                      "runtime_info": {}, "bench": [], "batch_size": 1})
    run_worker_once(client, "w1")
    ev = service.db.list_evaluations(service.experiment_id, limit=1)[0]
    ds = json.loads(ev["dataset_json"])
    assert ds["dataset_id"] == "mock-fba"
    assert ev["environment_id"] == "synthetic-quiet-v0"
    assert ev["duration_ms"] == 100.0
    assert ev["device"] == "cpu"
    assert ev["scientific_config_hash"] == service.scientific_config_hash
    assert ev["genome_hash"] and ev["seed"] is not None


# ------------------------------------------------------------ blocker 10
def test_scientific_vs_runtime_hash_split(smoke_config):
    base = copy.deepcopy(smoke_config)
    ops = copy.deepcopy(base)
    ops["worker"]["heartbeat_s"] = 3
    ops["checkpoint"]["interval_s"] = 60
    ops["gui"]["enabled"] = True
    assert scientific_config_hash(ops) == scientific_config_hash(base)
    assert runtime_config_hash(ops) != runtime_config_hash(base)
    for path, val in ((("evaluation", "backend"), "torch"),
                      (("evaluation", "duration_ms"), 999),
                      (("evaluation", "environment_id"), "other-env"),
                      (("population", "target_size"), 5),
                      (("evolution", "mutation_seed"), 1),
                      (("fba", "synthetic_neurons"), 77)):
        sci = copy.deepcopy(base)
        sci[path[0]][path[1]] = val
        assert scientific_config_hash(sci) != scientific_config_hash(base), path
    # a moved dataset directory is not a scientific change
    moved = copy.deepcopy(base)
    moved["fba"]["data_dir"] = "/mnt/elsewhere"
    assert scientific_config_hash(moved) == scientific_config_hash(base)


def test_resume_allows_runtime_change_blocks_scientific(tmp_path,
                                                        smoke_config):
    svc = MiobaService(copy.deepcopy(smoke_config), tmp_path / "runs")
    exp_id = svc.experiment_id
    svc.db.close()

    ops = copy.deepcopy(smoke_config)
    ops["worker"]["heartbeat_s"] = 3
    svc2 = MiobaService(ops, tmp_path / "runs", resume=exp_id)
    assert svc2.db.list_events(exp_id, type_="runtime_config_changed")
    assert not svc2.db.list_events(exp_id, type_="scientific_config_mismatch")
    svc2.db.close()

    sci = copy.deepcopy(smoke_config)
    sci["evaluation"]["duration_ms"] = 250
    with pytest.raises(ScientificConfigMismatch):
        MiobaService(sci, tmp_path / "runs", resume=exp_id)

    # no override exists: a replicate-count change is scientific too
    sci2 = copy.deepcopy(smoke_config)
    sci2["evaluation"]["replicates"] = 99
    with pytest.raises(ScientificConfigMismatch):
        MiobaService(sci2, tmp_path / "runs", resume=exp_id)
    # execution batch / headroom are operational -> resume is fine
    ops2 = copy.deepcopy(smoke_config)
    ops2["worker"]["execution_batch"] = 16
    ops2["worker"]["vram_headroom"] = 0.5
    svc4 = MiobaService(ops2, tmp_path / "runs", resume=exp_id)
    from ..storage.db import Database
    db = Database(tmp_path / "runs" / exp_id / "lineage.sqlite")
    evs = db.list_events(exp_id, type_="scientific_config_mismatch")
    assert evs and all(e["severity"] == "error" for e in evs)
    assert "allowed" not in json.loads(evs[-1]["payload_json"])
    db.close()
    svc4.db.close()
