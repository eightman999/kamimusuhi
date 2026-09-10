"""Heterogeneous-GPU research validity + crash safety (second review pass).

R1/R24  execution batch never changes replicate seeds / results / fitness
R2/R3   profile-driven benchmark, VRAM-headroom-aware batch choice
R4/R7   idempotent result delivery with retry; current_job_id lifecycle
R5      atomic generation (children + jobs + counters + RNG state)
R6/R23  GPU identity follows --device, collect_runtime_info(device=...)
R8      replay detects GPU model / compute-capability drift
R9      scientific config immutable per experiment (see test_reproducibility)
R10     explicit synthetic edge count
"""
from __future__ import annotations

import re
import copy
import json
import random

import pytest

from experiments.mioba.coordinator.replay import (ReplayConfigMismatch,
                                                  build_plan,
                                                  compare_device_identity,
                                                  run_plan)
from experiments.mioba.coordinator.service import MiobaService
from experiments.mioba.development.phenotype import develop
from experiments.mioba.evolution import population as pop_mod
from experiments.mioba.evolution.population import PopulationController
from experiments.mioba.fba.registry import get_backend
from experiments.mioba.fba.replicates import (chunk_indices, replicate_seed,
                                              replicate_seeds)
from experiments.mioba.fba.runtime_info import collect_runtime_info
from experiments.mioba.genome.hashing import (runtime_config_hash,
                                              scientific_config_hash)
from experiments.mioba.genome.schema import fba0_genome
from experiments.mioba.storage.db import Database
from experiments.mioba.workers import gpu_info
from experiments.mioba.workers import worker as W
from experiments.mioba.workers.bench import (BenchRow, choose_batch,
                                             profile_backend_kwargs)

DRIVE_CFG = {"env": {"stim_fraction": 0.2, "stim_rate_hz": 200.0}}


def _register(client, worker_id, device="cpu", **extra):
    body = {"worker_id": worker_id, "hostname": "h", "gpu": [],
            "runtime_info": {}, "bench": [], "device": device}
    body.update(extra)
    r = client.post("/api/worker/register", json=body)
    assert r.status_code == 200


# --------------------------------------------------------------- R1 / R24
def test_replicate_seeds_deterministic_and_distinct():
    a = replicate_seeds(1234, 32)
    assert a == replicate_seeds(1234, 32)
    assert len(set(a)) == 32
    assert a[:8] == replicate_seeds(1234, 8)          # prefix-stable
    assert replicate_seed(1234, 3) == a[3]
    assert replicate_seeds(1235, 32) != a
    assert chunk_indices(32, 16) == [list(range(16)), list(range(16, 32))]
    assert chunk_indices(32, 4) == [list(range(i, i + 4))
                                    for i in range(0, 32, 4)]
    assert chunk_indices(5, 16) == [[0, 1, 2, 3, 4]]


def _job(n_rep=16, seed=777):
    return {"job_id": "job_x", "seed": seed, "replicates": n_rep,
            "replicate_seeds": replicate_seeds(seed, n_rep),
            "duration_ms": 60.0, "environment_id": "synthetic-quiet-v0",
            "config": DRIVE_CFG}


@pytest.mark.parametrize("backend_name", ["mock", "torch"])
def test_execution_batch_does_not_change_replicate_results(backend_name):
    """3060 (16+16) vs P100 (4x8) vs single lane: identical seed set,
    identical per-replicate results, identical fitness."""
    phen = develop(fba0_genome(seed=3))
    job = _job(16)
    outs = {}
    for eb in (1, 4, 16):
        kw = ({"n_neurons": 200} if backend_name == "mock"
              else {"synthetic": True, "synthetic_neurons": 200})
        b = get_backend(backend_name, **kw)
        outs[eb] = W.evaluate_replicates(b, phen, job, "cpu", eb)
    ref = outs[1]
    assert ref["summary"]["spikes_total"] > 0          # not vacuous
    for eb in (4, 16):
        o = outs[eb]
        assert o["execution_batch_size"] == eb
        assert o["requested_replicates"] == o["completed_replicates"] == 16
        assert o["replicate_seeds"] == ref["replicate_seeds"]
        assert o["summary"]["replicate_seeds"] == ref["summary"]["replicate_seeds"]
        assert o["summary"]["replicate_index"] == list(range(16))
        assert (o["summary"]["per_replicate_spike_counts"]
                == ref["summary"]["per_replicate_spike_counts"])
        assert o["summary"]["per_replicate_mean_rate_hz"] == pytest.approx(
            ref["summary"]["per_replicate_mean_rate_hz"], abs=1e-9)
        assert o["summary"]["mean_rate_hz"] == pytest.approx(
            ref["summary"]["mean_rate_hz"], abs=1e-9)
        assert pop_mod.fitness_placeholder(o["summary"], 5.0) == pytest.approx(
            pop_mod.fitness_placeholder(ref["summary"], 5.0), abs=1e-9)


def test_two_workers_different_execution_batch_same_fitness(tmp_path,
                                                            smoke_config):
    """End to end through the coordinator: worker A (execution_batch=1) and
    worker B (execution_batch=4) evaluate the *same* genome/seed/replicates
    in two experiments; stored replicate seeds and fitness are identical."""
    cfg = copy.deepcopy(smoke_config)
    cfg["evaluation"]["replicates"] = 8
    cfg["population"]["target_size"] = 1
    fits, seeds_db, meta = [], [], []
    for eb, wid in ((1, "wA"), (4, "wB")):
        from fastapi.testclient import TestClient
        from experiments.mioba.coordinator.app import create_app
        svc = MiobaService(copy.deepcopy(cfg), tmp_path / f"runs{eb}")
        client = TestClient(create_app(svc))
        _register(client, wid)
        r = client.post("/api/worker/claim", json={"worker_id": wid,
                                                   "batch_size": eb})
        job = r.json()
        assert job["replicates"] == 8 and len(job["replicate_seeds"]) == 8
        body = W.run_job(client, wid, job, "cpu", eb)
        assert body["status"] == "SUCCEEDED" and body["delivery"] == "accepted"
        ev = svc.db.list_evaluations(svc.experiment_id, limit=1)[0]
        fits.append(ev["fitness"])
        seeds_db.append(json.loads(ev["replicate_seeds_json"]))
        meta.append((ev["requested_replicates"], ev["completed_replicates"],
                     ev["execution_batch_size"]))
        svc.db.close()
    assert fits[0] == fits[1]
    assert seeds_db[0] == seeds_db[1] and len(seeds_db[0]) == 8
    assert meta == [(8, 8, 1), (8, 8, 4)]


def test_replicates_is_scientific_execution_batch_is_not(smoke_config):
    a = copy.deepcopy(smoke_config)
    b = copy.deepcopy(smoke_config)
    b["evaluation"]["replicates"] = a["evaluation"]["replicates"] + 1
    assert scientific_config_hash(a) != scientific_config_hash(b)
    c = copy.deepcopy(smoke_config)
    c["worker"]["execution_batch"] = 32
    c["worker"]["vram_headroom"] = 0.5
    assert scientific_config_hash(a) == scientific_config_hash(c)
    assert runtime_config_hash(a) != runtime_config_hash(c)


# --------------------------------------------------------------- R2 / R3
def test_worker_profile_reflects_experiment(client, service, smoke_config):
    p = client.get("/api/worker/profile").json()
    assert p["backend"] == smoke_config["evaluation"]["backend"]
    assert p["fba"] == smoke_config["fba"]
    assert p["evaluation"]["replicates"] == smoke_config["evaluation"]["replicates"]
    assert p["evaluation"]["duration_ms"] == smoke_config["evaluation"]["duration_ms"]
    assert p["worker"]["candidates"] == smoke_config["worker"]["candidates"]
    assert 0 < p["worker"]["vram_headroom"] <= 1
    assert p["scientific_config_hash"] == service.scientific_config_hash
    kw = profile_backend_kwargs(p)
    assert kw["synthetic_neurons"] == smoke_config["fba"]["synthetic_neurons"]


def test_bench_on_profile_uses_experiment_conditions(client):
    p = client.get("/api/worker/profile").json()
    p["worker"]["candidates"] = [1, 2]
    p["worker"]["bench_duration_ms"] = 20
    rows, chosen, headroom = W.bench_on_profile(p, "mock", "cpu")
    assert [r.batch for r in rows] == [1, 2]
    assert all(r.ok for r in rows)
    assert chosen in (1, 2) and headroom == p["worker"]["vram_headroom"]


def _row(batch, ok=True, reserved=None, total=16e9, thr=None):
    return BenchRow(batch=batch, sim_seconds_per_wall_second=thr,
                    throughput=thr, wall_s=1.0, vram_allocated_bytes=reserved,
                    vram_reserved_bytes=reserved, vram_total_bytes=total,
                    ok=ok, error=None if ok else "CUDA out of memory")


def test_choose_batch_respects_vram_headroom_and_throughput():
    rows = [_row(1, reserved=2e9, thr=10), _row(2, reserved=4e9, thr=18),
            _row(4, reserved=8e9, thr=30), _row(8, reserved=14e9, thr=34),
            _row(16, ok=False)]
    # 8 has the best throughput but 14/16 = 0.875 > 0.85 headroom
    assert choose_batch(rows, vram_headroom=0.85) == 4
    assert choose_batch(rows, vram_headroom=0.90) == 8
    # throughput, not "largest successful": 4 beats 8 when 8 is slower
    rows2 = [_row(4, reserved=8e9, thr=30), _row(8, reserved=9e9, thr=25)]
    assert choose_batch(rows2, vram_headroom=0.85) == 4
    assert choose_batch(rows, vram_headroom=0.85, max_batch=2) == 2
    assert choose_batch([_row(16, ok=False)]) == 1
    d = rows[3].to_dict()
    assert {"vram_allocated_bytes", "vram_reserved_bytes", "vram_total_bytes",
            "sim_seconds_per_wall_second", "throughput", "batch", "ok",
            "vram_fraction"} <= set(d)
    assert d["vram_fraction"] == pytest.approx(0.875)


def test_registration_stores_bench_rows_device_and_batch(client, service):
    rows = [_row(1, reserved=2e9, thr=10).to_dict()]
    _register(client, "p100", device="cuda:1", bench=rows, batch_size=1,
              gpu_identity={"gpu_model": "Tesla P100-PCIE-16GB"})
    w = service.db.get_worker(service.experiment_id, "p100")
    assert w["device"] == "cuda:1" and w["batch_size"] == 1
    assert json.loads(w["bench_json"])[0]["vram_fraction"] == pytest.approx(0.125)
    ws = client.get("/api/workers").json()["workers"]
    assert ws[0]["device"] == "cuda:1"


# --------------------------------------------------------------- R4 / R7
class _Resp:
    def __init__(self, code, data=None):
        self.status_code = code
        self._data = data or {}

    def json(self):
        return self._data


class _FlakyClient:
    """Wraps the TestClient: first N posts to /api/worker/result raise
    (connection lost) or return 500; the coordinator may or may not have
    committed depending on `commit_then_drop`."""

    def __init__(self, inner, fail_before=0, drop_response_after_commit=0):
        self.inner = inner
        self.fail_before = fail_before
        self.drop = drop_response_after_commit
        self.posts = 0

    def post(self, path, json=None):
        if path != "/api/worker/result":
            return self.inner.post(path, json=json)
        self.posts += 1
        if self.fail_before > 0:
            self.fail_before -= 1
            raise ConnectionError("coordinator unreachable")
        resp = self.inner.post(path, json=json)
        if self.drop > 0:
            self.drop -= 1
            raise ConnectionError("response lost after commit")
        return resp

    def get(self, *a, **k):
        return self.inner.get(*a, **k)


def test_result_id_deterministic():
    a = W.result_id_for("job_1", "w1", 0)
    assert a == W.result_id_for("job_1", "w1", 0)
    assert a != W.result_id_for("job_1", "w1", 1)
    assert a != W.result_id_for("job_1", "w2", 0)


def test_case_a_commit_then_lost_response_no_duplicate(client, service):
    _register(client, "w1")
    job = client.post("/api/worker/claim", json={"worker_id": "w1"}).json()
    flaky = _FlakyClient(client, drop_response_after_commit=1)
    body = W.run_job(flaky, "w1", job, "cpu", 1)
    assert body["delivery"] == "duplicate"        # 2nd POST acked as dup
    assert flaky.posts == 2
    evs = service.db.list_evaluations(service.experiment_id, limit=100)
    assert len(evs) == 1 and evs[0]["result_id"] == body["result_id"]
    assert service.db.get_job(job["job_id"])["status"] == "SUCCEEDED"
    assert service.counters()["evaluations_succeeded"] == 1
    assert W.STATE.current_job_id is None


def test_case_b_crash_before_commit_then_retry(client, service, monkeypatch):
    _register(client, "w1")
    job = client.post("/api/worker/claim", json={"worker_id": "w1"}).json()
    real = service.db.insert_evaluation
    calls = {"n": 0}

    def boom(*a, **k):
        calls["n"] += 1
        if calls["n"] == 1:
            raise RuntimeError("simulated coordinator crash before commit")
        return real(*a, **k)

    monkeypatch.setattr(service.db, "insert_evaluation", boom)
    # network failures first, then the 500 from the crash, then success
    flaky = _FlakyClient(client, fail_before=2)
    body = W.run_job(flaky, "w1", job, "cpu", 1, deliver=False)
    # simulate the crash on the first *reaching* POST -> 500 -> retry
    with pytest.raises(RuntimeError):
        service.worker_result(job["job_id"], "w1", body["status"],
                              body["evaluation"], None,
                              result_id=body["result_id"])
    assert service.db.get_job(job["job_id"])["status"] == "RUNNING"
    assert service.db.list_evaluations(service.experiment_id, limit=10) == []
    slept = []
    status = W.deliver_result(flaky, body, sleep=slept.append)
    assert status == "accepted"
    assert flaky.posts == 3 and len(slept) == 2
    evs = service.db.list_evaluations(service.experiment_id, limit=10)
    assert len(evs) == 1 and evs[0]["result_id"] == body["result_id"]


def test_case_c_stale_result_after_reclaim_stops_retrying(client, service):
    _register(client, "w1")
    _register(client, "w2")
    job = client.post("/api/worker/claim", json={"worker_id": "w1"}).json()
    body = W.run_job(client, "w1", job, "cpu", 1, deliver=False)
    # w1 goes dark: coordinator marks UNKNOWN and requeues, w2 reclaims
    service.db.mark_running_unknown(service.experiment_id, [job["job_id"]])
    service.db.requeue_unknown(service.experiment_id, max_attempts=3)
    job2 = client.post("/api/worker/claim", json={"worker_id": "w2"}).json()
    assert job2["job_id"] == job["job_id"] and job2["attempt"] == 1
    slept = []
    assert W.deliver_result(client, body, sleep=slept.append) == "rejected"
    assert slept == []                                  # no retry loop
    assert service.db.list_events(service.experiment_id,
                                  type_="stale_result_rejected")
    body2 = W.run_job(client, "w2", job2, "cpu", 1)
    assert body2["delivery"] == "accepted"
    assert body2["result_id"] != body["result_id"]
    assert len(service.db.list_evaluations(service.experiment_id,
                                           limit=10)) == 1


def test_deliver_retries_on_5xx_and_connection_errors():
    seq = [None, _Resp(500), _Resp(503), _Resp(200, {"ok": True})]

    class C:
        def post(self, path, json=None):
            r = seq.pop(0)
            if r is None:
                raise ConnectionError()
            return r

    slept = []
    assert W.deliver_result(C(), {"job_id": "j"}, sleep=slept.append) == "accepted"
    assert slept == [1.0, 2.0, 4.0]
    assert W.deliver_result(_FixedClient(404), {}, sleep=slept.append) == "gone"


class _FixedClient:
    def __init__(self, code):
        self.code = code

    def post(self, path, json=None):
        return _Resp(self.code)


def test_current_job_id_lifecycle(client, service, monkeypatch):
    _register(client, "w1")
    job = client.post("/api/worker/claim", json={"worker_id": "w1"}).json()
    seen = []
    real = client.post

    def spy(path, json=None):
        if path == "/api/worker/result":
            seen.append(("delivering", W.STATE.current_job_id, W.STATE.phase))
        return real(path, json=json)

    monkeypatch.setattr(client, "post", spy)
    assert W.STATE.current_job_id is None
    W.run_job(client, "w1", job, "cpu", 1)
    assert seen == [("delivering", job["job_id"], "delivering")]
    assert W.STATE.current_job_id is None and W.STATE.phase == "idle"
    hb = W.heartbeat_payload("w1", W.STATE.current_job_id, device="cpu")
    assert hb["current_job_id"] is None and hb["device"] == "cpu"
    W.STATE.set_job("job_z")
    hb = W.heartbeat_payload("w1", W.STATE.current_job_id, device="cpu")
    assert hb["current_job_id"] == "job_z"
    client.post("/api/worker/heartbeat", json=hb)
    ws = client.get("/api/workers").json()["workers"]
    assert ws[0]["current_job_id"] == "job_z"
    W.STATE.set_job(None)


# ---------------------------------------------------------------------- R5
def _run_generation(db_path, cfg, crash_at=None):
    """Seed gen 0, fake-evaluate it, advance to gen 1 (optionally crashing
    while creating child `crash_at`). Returns (db, rng_after)."""
    db = Database(db_path)
    exp_id = "EXP"
    if db.get_experiment(exp_id) is None:
        db.create_experiment(exp_id, cfg, "cfg", None)
    rng = random.Random(1)
    saved = _rng_state(db)
    if saved:
        rng.setstate((3, tuple(json.loads(saved)), None))

    def persist(born):
        db.set_rng_state(exp_id, json.dumps(list(rng.getstate()[1])))
        prev = json.loads(db.get_experiment(exp_id)["counters_json"] or "{}")
        db.set_counters(exp_id, {"births": prev.get("births", 0) + born})

    pc = PopulationController(db, exp_id, cfg, rng, persist=persist)
    pc.seed_if_empty()
    # complete every gen-0 job with a fake evaluation so gen 1 can start
    for j in db.list_jobs(exp_id, limit=100):
        if j["status"] == "QUEUED":
            db.claim_job(exp_id, "w")
    for j in db.list_jobs(exp_id, limit=100):
        if j["status"] == "RUNNING":
            with db.transaction():
                db.finish_job(j["job_id"], "SUCCEEDED", "w")
                db.insert_evaluation(exp_id, j["job_id"], j["genome_id"],
                                     {"summary": {"mean_rate_hz": 1.0},
                                      "backend": "mock", "seed": j["seed"],
                                      "batch_size": 1, "fitness": -1.0})
    real_mutate = pop_mod.mutate
    calls = {"n": 0}

    def crashing(parent, r, birth_index, generation):
        g = real_mutate(parent, r, birth_index=birth_index,
                        generation=generation)
        if crash_at is not None and birth_index == crash_at:
            raise RuntimeError(f"simulated crash at child {birth_index}")
        return g

    pop_mod.mutate = crashing
    try:
        try:
            pc.maybe_advance()
        except RuntimeError:
            pass
    finally:
        pop_mod.mutate = real_mutate
    return db, rng


def _rng_state(db):
    return db.get_experiment("EXP")["rng_state_json"]


def _counters(db):
    return json.loads(db.get_experiment("EXP")["counters_json"] or "{}")


def _lineage(db):
    gs = db.list_genomes("EXP", limit=1000)
    out = []
    for g in sorted(gs, key=lambda g: (g["generation"], g["birth_index"])):
        gj = json.loads(g["genome_json"])
        out.append((g["generation"], g["birth_index"], g["genome_id"],
                    tuple(gj["parent_ids"]),
                    tuple(sorted(m["mutation_id"] for m in
                                 db.genome_mutations(g["genome_id"]))),
                    tuple(o["organ_id"] for o in gj["artificial_organs"])))
    return out


@pytest.mark.parametrize("crash_at", [0, 3, 7])
def test_generation_crash_consistency(tmp_path, smoke_config, crash_at):
    cfg = copy.deepcopy(smoke_config)
    cfg["population"]["target_size"] = 8
    cfg["evolution"]["max_generations"] = 2
    # control: no crash
    ctrl, ctrl_rng = _run_generation(tmp_path / "ctrl.sqlite", cfg)
    ctrl_lineage = _lineage(ctrl)
    assert len([g for g in ctrl_lineage if g[0] == 1]) == 8
    ctrl_rng_after = _rng_state(ctrl)

    # crashed run: generation 1 partially built then aborted
    db, _ = _run_generation(tmp_path / "crash.sqlite", cfg, crash_at=crash_at)
    gen1 = [g for g in _lineage(db) if g[0] == 1]
    assert gen1 == []                                # nothing partial
    assert not [j for j in db.list_jobs("EXP", limit=100)
                if j["status"] == "QUEUED"]
    assert not db.list_events("EXP", type_="generation_advanced")
    rng_after_crash = _rng_state(db)
    db.close()

    # resume from the DB (RNG state reloaded from what was committed)
    db2, _ = _run_generation(tmp_path / "crash.sqlite", cfg)
    assert _lineage(db2) == ctrl_lineage
    assert _rng_state(db2) == ctrl_rng_after
    assert rng_after_crash != ctrl_rng_after         # crash did not advance RNG
    assert len(db2.list_events("EXP", type_="generation_advanced")) == 1
    gen1_ids = [g[2] for g in _lineage(db2) if g[0] == 1]
    assert len(gen1_ids) == len(set(gen1_ids)) == 8
    queued = [j for j in db2.list_jobs("EXP", limit=100) if j["status"] == "QUEUED"]
    assert len(queued) == 8
    assert _counters(db2)["births"] == 16      # gen 0 + gen 1
    db2.close()
    ctrl.close()


# ---------------------------------------------------------------- R6 / R23
_NVSMI = [
    {"index": 0, "name": "NVIDIA GeForce RTX 3060", "uuid": "GPU-3060",
     "memory_total_mb": 12288, "memory_used_mb": 4300, "temperature_c": 61,
     "utilization_pct": 87, "driver": "560.35", "cuda": "12.6"},
    {"index": 1, "name": "Tesla P100-PCIE-16GB", "uuid": "GPU-P100",
     "memory_total_mb": 16384, "memory_used_mb": 9000, "temperature_c": 55,
     "utilization_pct": 40, "driver": "560.35", "cuda": "12.6"},
]


def test_device_index_parsing():
    assert gpu_info.device_index("cuda:1") == 1
    assert gpu_info.device_index("cuda") == 0
    assert gpu_info.device_index("cpu") is None
    assert gpu_info.device_index(None) is None


def test_gpu_identity_follows_device(monkeypatch):
    monkeypatch.setattr(gpu_info, "query_gpus", lambda: copy.deepcopy(_NVSMI))

    def fake_props(d):
        i = gpu_info.device_index(d)
        if i is None:
            return {"gpu_index": None}
        return {"gpu_index": i, "gpu_model": _NVSMI[i]["name"],
                "gpu_uuid": _NVSMI[i]["uuid"],
                "compute_capability": "6.0" if i == 1 else "8.6",
                "vram_total_bytes": 16e9, "torch_version": "2.9.1+cu126",
                "cuda_runtime": "12.6"}

    monkeypatch.setattr(gpu_info, "torch_device_props", fake_props)
    p100 = gpu_info.gpu_identity("cuda:1")
    assert p100["device"] == "cuda:1" and p100["gpu_index"] == 1
    assert p100["gpu_model"] == "Tesla P100-PCIE-16GB"
    assert p100["gpu_uuid"] == "GPU-P100" and p100["compute_capability"] == "6.0"
    r3060 = gpu_info.gpu_identity("cuda:0")
    assert r3060["gpu_model"] == "NVIDIA GeForce RTX 3060"
    assert r3060["compute_capability"] == "8.6"
    # heartbeat telemetry for cuda:1 is the P100 row, not GPU 0
    g = gpu_info.query_device_gpu("cuda:1", p100)
    assert g["uuid"] == "GPU-P100" and g["temperature_c"] == 55
    hb = W.heartbeat_payload("p100", None, device="cuda:1", identity=p100)
    assert hb["gpu_uuid"] == "GPU-P100" and hb["vram_total_mb"] == 16384
    assert hb["mie_samples"][0]["source"] == "gpu:GPU-P100"
    cpu = gpu_info.gpu_identity("cpu")
    assert cpu["gpu_model"] is None and cpu["gpu_index"] is None


def test_collect_runtime_info_device_aware(monkeypatch):
    monkeypatch.setattr(gpu_info, "query_gpus", lambda: copy.deepcopy(_NVSMI))
    monkeypatch.setattr(gpu_info, "torch_device_props", lambda d: {})
    ri = collect_runtime_info(backend="torch", device="cuda:1")
    assert ri["device"] == "cuda:1" and ri["gpu_index"] == 1
    assert ri["gpu_model"] == "Tesla P100-PCIE-16GB"
    assert ri["gpu_uuid"] == "GPU-P100"
    ri0 = collect_runtime_info(backend="torch", device="cuda:0")
    assert ri0["gpu_model"] == "NVIDIA GeForce RTX 3060"
    assert collect_runtime_info(device="cpu")["gpu_model"] is None


# ---------------------------------------------------------------------- R8
def test_compare_device_identity():
    rec = {"gpu_model": "NVIDIA GeForce RTX 3060", "compute_capability": "8.6",
           "gpu_uuid": "GPU-3060", "gpu_index": 0, "device": "cuda:0",
           "torch_version": "2.9.1+cu126", "cuda_runtime": "12.6"}
    same = dict(rec)
    assert compare_device_identity(rec, same) == ([], [])
    other = dict(rec, gpu_uuid="GPU-3060-b", gpu_index=1, device="cuda:1")
    fatal, info = compare_device_identity(rec, other)
    assert fatal == [] and len(info) == 2
    p100 = dict(rec, gpu_model="Tesla P100-PCIE-16GB", compute_capability="6.0",
                gpu_uuid="GPU-P100", device="cuda:1")
    fatal, _ = compare_device_identity(rec, p100)
    assert any("GPU model" in f for f in fatal)
    assert any("compute capability" in f for f in fatal)


def _recorded_evaluation(tmp_path, smoke_config, runtime_info):
    from fastapi.testclient import TestClient
    from experiments.mioba.coordinator.app import create_app
    cfg = copy.deepcopy(smoke_config)
    cfg["population"]["target_size"] = 1
    cfg["evaluation"]["replicates"] = 3
    svc = MiobaService(cfg, tmp_path / "runs")
    client = TestClient(create_app(svc))
    _register(client, "rtx3060", device="cuda:0")
    job = client.post("/api/worker/claim", json={"worker_id": "rtx3060"}).json()
    # mock backend ignores the device string; record it as the GPU device
    body = W.run_job(client, "rtx3060", job, runtime_info["device"], 2,
                     runtime_info=runtime_info)
    assert body["delivery"] == "accepted"
    ev = svc.db.list_evaluations(svc.experiment_id, limit=1)[0]
    exp_dir = svc.run_dir
    svc.db.close()
    return exp_dir, ev["evaluation_id"], cfg


def test_replay_detects_gpu_model_cc_drift(tmp_path, smoke_config):
    rec_ri = {"device": "cuda:0", "gpu_index": 0, "gpu_uuid": "GPU-3060",
              "gpu_model": "NVIDIA GeForce RTX 3060",
              "compute_capability": "8.6", "torch_version": "2.9.1+cu126",
              "cuda_runtime": "12.6"}
    exp_dir, eid, cfg = _recorded_evaluation(tmp_path, smoke_config, rec_ri)
    p100 = {"device": "cuda:1", "gpu_index": 1, "gpu_uuid": "GPU-P100",
            "gpu_model": "Tesla P100-PCIE-16GB", "compute_capability": "6.0",
            "torch_version": "2.9.1+cu126", "cuda_runtime": "12.6"}
    # strict: RTX3060 recording replayed on P100 -> mismatch
    with pytest.raises(ReplayConfigMismatch) as ei:
        build_plan(exp_dir, eid, device="cuda:1", strict=True,
                   current_device_identity=p100, current_config=cfg)
    assert "compute capability" in str(ei.value)
    assert "--allow-device-drift" in str(ei.value)
    # strict + explicit parity mode: allowed, drift recorded
    plan = build_plan(exp_dir, eid, device="cuda:1", strict=True,
                      allow_device_drift=True, current_device_identity=p100,
                      current_config=cfg)
    assert any("parity" in w for w in plan.device_warnings)
    assert plan.identity["recorded_device"]["gpu_model"].endswith("3060")
    assert plan.identity["current_device"]["compute_capability"] == "6.0"
    # same GPU class, other physical card -> informational only
    twin = dict(rec_ri, gpu_uuid="GPU-3060-2", gpu_index=1, device="cuda:1")
    plan = build_plan(exp_dir, eid, device="cuda:1", strict=True,
                      current_device_identity=twin, current_config=cfg)
    assert any("UUID" in w for w in plan.device_warnings)
    # non-strict: warnings, not failure
    plan = build_plan(exp_dir, eid, device="cuda:1",
                      current_device_identity=p100, current_config=cfg)
    assert any("GPU model" in w for w in plan.warnings)


def test_replay_reproduces_replicates_with_other_execution_batch(tmp_path,
                                                                  smoke_config):
    cpu_ri = {"device": "cpu"}
    exp_dir, eid, cfg = _recorded_evaluation(tmp_path, smoke_config, cpu_ri)
    for eb in (1, 3):
        plan = build_plan(exp_dir, eid, device="cpu", strict=True,
                          current_device_identity={"device": "cpu"},
                          current_config=cfg, execution_batch=eb)
        assert plan.replicates == 3 and len(plan.replicate_seeds) == 3
        assert plan.execution_batch == eb
        res = run_plan(plan)
        assert res["diff"]["identical_replicate_seeds"]
        assert res["diff"]["identical_spike_counts"]
        assert res["conditions"]["replicates"] == 3


# --------------------------------------------------------------------- R10
def test_synthetic_edges_explicit():
    b = get_backend("torch", synthetic=True, synthetic_neurons=300,
                    synthetic_edges=900)
    b.initialize(develop(fba0_genome()), batch_size=1, seed=1, device="cpu")
    ident = b.dataset_identity()
    assert ident["version"] == "v0-n300-e900"
    # 900 sampled edges; self-edges dropped / duplicates coalesced
    assert 850 <= b.W._nnz() <= 900
    dense_p = get_backend("torch", synthetic=True, synthetic_neurons=300,
                          connectivity=0.01)
    assert dense_p.dataset_identity()["version"] == "v0-n300-p0.01"
    kw = profile_backend_kwargs({"backend": "torch",
                                 "fba": {"synthetic": True,
                                         "synthetic_neurons": 139000,
                                         "synthetic_edges": 14_000_000}})
    assert kw["synthetic_edges"] == 14_000_000


# ------------------------------------------------------------ schema / db
def test_schema_v2_db_migrates_to_v3(tmp_path):
    import sqlite3
    from experiments.mioba.storage import db as dbmod
    path = tmp_path / "old.sqlite"
    if sqlite3.sqlite_version_info < (3, 35):
        pytest.skip("ALTER TABLE DROP COLUMN needs sqlite >= 3.35")
    conn = sqlite3.connect(path)
    # sqlite DROP COLUMN chokes on comments inside CREATE TABLE
    conn.executescript(re.sub(r"--[^\n]*", "", dbmod._SCHEMA_SQL))
    # drop the v3 columns to fake a v2 database
    for table, col, _ in dbmod._V3_COLUMNS:
        conn.execute(f"ALTER TABLE {table} DROP COLUMN {col}")
    conn.execute("INSERT INTO schema_version(version) VALUES(2)")
    conn.commit()
    conn.close()
    db = Database(path)
    cols = {r[1] for r in db.conn.execute("PRAGMA table_info(evaluations)")}
    assert {"requested_replicates", "completed_replicates",
            "execution_batch_size", "replicate_seeds_json",
            "result_id"} <= cols
    jcols = {r[1] for r in db.conn.execute("PRAGMA table_info(evaluation_jobs)")}
    assert {"replicates", "result_id"} <= jcols
    assert db.conn.execute("SELECT version FROM schema_version").fetchone()[0] \
        == dbmod.SCHEMA_VERSION
    # v3 writes work on the migrated DB
    db.create_experiment("E", {}, "h", None)
    jid = db.enqueue_job("E", "g", "env", 1, "smoke", "mock", 10.0, [],
                         replicates=4)
    assert db.get_job(jid)["replicates"] == 4
    db.close()
