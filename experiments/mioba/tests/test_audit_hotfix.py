"""Regression tests for the third-pass MIOBA audit fixes."""
from __future__ import annotations

import copy
import random
from contextlib import contextmanager
from pathlib import Path

import pytest

from experiments.mioba.coordinator.app import create_app
from experiments.mioba.coordinator.service import MiobaService
from experiments.mioba.evolution.population import PopulationController
from experiments.mioba.storage import models as M
from experiments.mioba.storage.db import Database
from experiments.mioba.workers import worker as W


def test_torch_generators_use_exact_indexed_device_source():
    src = (Path(__file__).resolve().parents[1] / "fba" /
           "torch_backend.py").read_text()
    assert "torch.Generator(device=self.device)" in src
    assert "torch.Generator(device=self.device.type)" not in src


def test_worker_local_data_dir_overrides_coordinator_path():
    cfg = {"fba": {"data_dir": "/coordinator/flywire",
                   "synthetic": False, "synthetic_neurons": 10}}
    assert W.backend_kwargs(cfg, "/run", data_dir="/worker/flywire")["data_dir"] \
        == "/worker/flywire"
    assert W.backend_kwargs(cfg, "/run")["data_dir"] == "/coordinator/flywire"


def test_retryable_classifier_and_oom_classifier():
    assert W._is_oom(RuntimeError("CUDA out of memory"))
    assert W._is_retryable_infra(RuntimeError("CUDA out of memory"))
    assert W._is_retryable_infra(RuntimeError("CUDA error: device unavailable"))
    assert not W._is_retryable_infra(ValueError("bad scientific phenotype"))


def test_atomic_generation_restores_rng_same_process():
    class DummyDB:
        @contextmanager
        def transaction(self):
            yield

    pc = object.__new__(PopulationController)
    pc.db = DummyDB()
    pc.rng = random.Random(1234)
    before = pc.rng.getstate()
    with pytest.raises(RuntimeError):
        with pc._atomic_generation():
            pc.rng.random()
            pc.rng.randrange(1000)
            raise RuntimeError("same-process rollback")
    assert pc.rng.getstate() == before


def test_manual_job_api_requires_token_and_uses_scientific_config(
        tmp_path, smoke_config):
    from fastapi.testclient import TestClient
    cfg = copy.deepcopy(smoke_config)
    cfg["evaluation"]["replicates"] = 7
    cfg["evaluation"]["duration_ms"] = 321
    svc = MiobaService(cfg, tmp_path / "runs")
    client = TestClient(create_app(svc))
    gid = svc.db.list_genomes(svc.experiment_id, limit=1)[0]["genome_id"]

    hostile = {"genome_id": gid, "backend": "genn", "seed": 999,
               "duration_ms": 9999, "environment_id": "evil"}
    assert client.post("/api/jobs", json=hostile).status_code == 401
    r = client.post("/api/jobs", json=hostile,
                    headers={"X-Mioba-Token": svc.token})
    assert r.status_code == 200
    j = svc.db.get_job(r.json()["job_id"])
    grow = svc.db.get_genome(gid)
    assert j["backend"] == cfg["evaluation"]["backend"]
    assert j["duration_ms"] == cfg["evaluation"]["duration_ms"]
    assert j["environment_id"] == cfg["evaluation"]["environment_id"]
    assert j["replicates"] == 7
    assert j["seed"] == grow["random_seed"]
    svc.db.close()


def test_retryable_result_is_unknown_not_scientific_failure(
        tmp_path, smoke_config):
    from fastapi.testclient import TestClient
    cfg = copy.deepcopy(smoke_config)
    cfg["population"]["target_size"] = 1
    svc = MiobaService(cfg, tmp_path / "runs")
    client = TestClient(create_app(svc))
    client.post("/api/worker/register", json={
        "worker_id": "w1", "hostname": "h", "gpu": [],
        "runtime_info": {}, "bench": [], "batch_size": 1})
    job = client.post("/api/worker/claim",
                      json={"worker_id": "w1"}).json()
    body = {"job_id": job["job_id"], "worker_id": "w1",
            "result_id": "res_retry", "status": "RETRY",
            "evaluation": None, "error": "CUDA out of memory"}
    r = client.post("/api/worker/result", json=body)
    assert r.status_code == 200 and r.json()["retryable"] is True
    assert svc.db.get_job(job["job_id"])["status"] == M.JOB_UNKNOWN
    assert svc.counters()["evaluations_failed"] == 0
    # lost response -> same result id is an idempotent duplicate ack
    r2 = client.post("/api/worker/result", json=body)
    assert r2.status_code == 200 and r2.json()["duplicate"] is True
    svc.db.requeue_unknown(svc.experiment_id, max_attempts=1)
    assert svc.db.get_job(job["job_id"])["status"] == M.JOB_QUEUED
    # Same worker is avoided for the retryable job; another worker may take it.
    assert svc.db.claim_job(svc.experiment_id, "w1") is None
    assert svc.db.claim_job(svc.experiment_id, "w2")["job_id"] == job["job_id"]
    svc.db.close()


def test_reregister_updates_only_latest_worker_run(tmp_path):
    db = Database(tmp_path / "lineage.sqlite")
    db.create_experiment("E", {}, "cfg", None)
    db.register_worker("E", "w", "h", [], {}, [], 1, device="cpu")
    db.register_worker("E", "w", "h", [], {}, [], 2, device="cpu")
    rows = db.list_workers("E")
    assert len(rows) == 2
    assert [r["status"] for r in rows].count("online") == 1
    assert [r["status"] for r in rows].count("offline") == 1
    db.heartbeat("E", "w", {"current_job_id": "j"})
    db.worker_finished_job("E", "w", True)
    rows = db.list_workers("E")
    latest = rows[-1]
    older = rows[0]
    assert latest["completed_jobs"] == 1
    assert older["completed_jobs"] == 0
    assert older["status"] == "offline"
    db.close()


def test_readonly_badge_rendering_source():
    html = (Path(__file__).resolve().parents[1] / "gui" / "static" /
            "index.html").read_text()
    assert "$('#ro').textContent = T().readonly || '';" in html
