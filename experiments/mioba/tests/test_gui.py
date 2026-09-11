"""Observatory GUI endpoint tests (read-only aggregate endpoints)."""
import json

import pytest

from experiments.mioba.tests.conftest import run_worker_once


@pytest.fixture
def gui_client(client, service):
    client.post("/api/worker/register", json={
        "worker_id": "gw1", "hostname": "ghost", "gpu": [],
        "runtime_info": {"hostname": "ghost"}, "bench": []})
    run_worker_once(client, "gw1")
    service.db.heartbeat(service.experiment_id, "gw1",
                         {"temperature_c": 42.0, "current_job_id": None})
    service.db.insert_telemetry(service.experiment_id, [{
        "timestamp": service.db._now(), "source": "host:cpu",
        "domain": "machine_interoception", "signal_type": "cpu_percent",
        "value": 12.0, "confidence": 1.0}])
    return client


def test_index_serves_spa(gui_client):
    r = gui_client.get("/")
    assert r.status_code == 200
    assert "text/html" in r.headers["content-type"]
    assert "MIOBA Observatory" in r.text


def test_static_file_and_traversal(gui_client):
    assert gui_client.get("/static/index.html").status_code == 200
    assert gui_client.get("/static/..%2Fdb.py").status_code in (404, 400)
    r = gui_client.get("/static/..%2F..%2Fstorage%2Fschema.sql")
    assert r.status_code in (404, 400)
    assert gui_client.get("/static/nope.txt").status_code == 404


def test_dashboard(gui_client):
    d = gui_client.get("/api/gui/dashboard").json()
    for key in ("status", "workers", "throughput", "recent_events"):
        assert key in d
    assert d["status"]["kind"] == "LIVE"
    assert d["workers"] and all(w["kind"] == "LIVE" for w in d["workers"])
    assert d["throughput"]["kind"] == "DERIVED"


def test_gui_genomes_list_and_detail(gui_client, service):
    rows = gui_client.get("/api/gui/genomes").json()["genomes"]
    assert len(rows) >= 4
    for r in rows:
        for key in ("clade_name", "organ_count", "children_count",
                    "ancestry_fraction", "best_fitness"):
            assert key in r
    gid = rows[0]["genome_id"]
    g = gui_client.get(f"/api/gui/genomes/{gid}").json()
    assert g["ancestry_chain"]["kind"] == "DERIVED"
    assert g["phenotype"]["kind"] == "DERIVED"
    assert g["ancestry_chain"]["rows"][-1]["genome_id"] == gid
    assert gui_client.get("/api/gui/genomes/nope").status_code == 404


def test_gui_telemetry(gui_client):
    t = gui_client.get("/api/gui/telemetry", params={"minutes": 5}).json()
    assert "telemetry" in t and "worker_heartbeats" in t
    assert "host:cpu|cpu_percent" in t["telemetry"]
    assert "gw1" in t["worker_heartbeats"]
    assert t["worker_heartbeats"]["gw1"]


def test_ancestry_fraction_synthetic(client, service):
    """Organ genome under synthetic mode must get fraction in (0,1)."""
    import random
    from experiments.mioba.genome.mutation import mutate_child as mutate
    from experiments.mioba.genome.schema import Genome, fba0_genome
    from experiments.mioba.development.phenotype import develop

    base = fba0_genome()
    rng = random.Random(0)
    organ_genome = None
    for i in range(200):
        g = mutate(base, rng, i, 1)
        # a genome whose only organs are disabled develops no extra
        # neurons, so its ancestry fraction is legitimately 1.0
        if any(o.enabled for o in g.artificial_organs):
            organ_genome = g
            break
    assert organ_genome is not None
    phen = develop(organ_genome, base_neurons=service.fba_base_neurons())
    assert 0.0 < phen["ancestry_fraction"] < 1.0


def test_evaluation_runtime_info_from_worker(client, service):
    client.post("/api/worker/register", json={
        "worker_id": "w-rt", "hostname": "rthost", "gpu": [],
        "runtime_info": {"hostname": "rthost", "backend": "mock"},
        "bench": []})
    body = run_worker_once(client, "w-rt")
    evs = service.db.list_evaluations(service.experiment_id)
    assert evs
    ri = json.loads(evs[0]["runtime_info_json"])
    assert ri.get("hostname") == "rthost"
    assert "gpu" in ri


# ---- follow-up fixes (2026-09-11 GPU validation findings) -------------------

def test_lost_worker_clears_current_job_id(client, service):
    """A worker that is lost/offline no longer advertises a current job:
    the job it held is requeued and finished elsewhere, and the GUI must
    not show a dead card as '実行中ジョブ'."""
    db, exp = service.db, service.experiment_id
    client.post("/api/worker/register", json={
        "worker_id": "wl1", "hostname": "h", "gpu": [],
        "runtime_info": {"hostname": "h"}, "bench": []})
    db.heartbeat(exp, "wl1", {"current_job_id": "job_deadbeef"})
    assert db.get_worker(exp, "wl1")["current_job_id"] == "job_deadbeef"

    db.mark_worker_status(exp, "wl1", "lost")
    assert db.get_worker(exp, "wl1")["current_job_id"] is None
    assert db.get_worker(exp, "wl1")["status"] == "lost"

    w = next(w for w in client.get("/api/gui/dashboard").json()["gpu_workers"]
             if w["worker_id"] == "wl1")
    assert w["current_job_id"] is None


def test_offline_worker_clears_current_job_id(client, service):
    db, exp = service.db, service.experiment_id
    client.post("/api/worker/register", json={
        "worker_id": "wl2", "hostname": "h", "gpu": [],
        "runtime_info": {"hostname": "h"}, "bench": []})
    db.heartbeat(exp, "wl2", {"current_job_id": "job_cafe"})
    db.mark_worker_status(exp, "wl2", "offline")
    assert db.get_worker(exp, "wl2")["current_job_id"] is None


def test_dashboard_groups_worker_runs_by_physical_gpu(client, service):
    """Restarting a worker adds a worker_runs row; the dashboard must keep
    one card per physical GPU (grouped by GPU UUID) and expose the earlier
    registrations as history, not as extra GPUs."""
    ri0 = {"hostname": "h", "gpu_uuid": "GPU-AAA", "gpu_model": "RTX 3060",
           "device": "cuda:0", "gpu_index": 0, "compute_capability": "8.6"}
    ri1 = {"hostname": "h", "gpu_uuid": "GPU-BBB", "gpu_model": "Tesla P100",
           "device": "cuda:1", "gpu_index": 1, "compute_capability": "6.0"}
    for _ in range(3):                      # three registrations of the 3060
        client.post("/api/worker/register", json={
            "worker_id": "rtx3060", "hostname": "h", "gpu": [],
            "runtime_info": ri0, "bench": [], "device": "cuda:0"})
    client.post("/api/worker/register", json={
        "worker_id": "p100", "hostname": "h", "gpu": [],
        "runtime_info": ri1, "bench": [], "device": "cuda:1"})

    d = client.get("/api/gui/dashboard").json()
    assert len(d["workers"]) == 4                 # raw runs unchanged
    gw = {g["gpu_uuid"]: g for g in d["gpu_workers"] if g.get("gpu_uuid")}
    assert set(gw) == {"GPU-AAA", "GPU-BBB"}
    assert gw["GPU-AAA"]["run_count"] == 3
    assert len(gw["GPU-AAA"]["past_runs"]) == 2
    assert all(r["status"] == "offline" for r in gw["GPU-AAA"]["past_runs"])
    assert gw["GPU-AAA"]["status"] == "online"
    assert gw["GPU-BBB"]["run_count"] == 1
    assert gw["GPU-BBB"]["past_runs"] == []
    # physical GPUs are ordered by GPU index, so cards stay stable
    assert [g["gpu_uuid"] for g in d["gpu_workers"]] == ["GPU-AAA", "GPU-BBB"]


def test_gpu_group_totals_span_all_runs(client, service):
    db, exp = service.db, service.experiment_id
    ri = {"hostname": "h", "gpu_uuid": "GPU-CCC", "device": "cuda:0",
          "gpu_index": 0}
    client.post("/api/worker/register", json={
        "worker_id": "wg", "hostname": "h", "gpu": [], "runtime_info": ri,
        "bench": [], "device": "cuda:0"})
    db.worker_finished_job(exp, "wg", True)
    db.worker_finished_job(exp, "wg", True)
    client.post("/api/worker/register", json={          # restart
        "worker_id": "wg", "hostname": "h", "gpu": [], "runtime_info": ri,
        "bench": [], "device": "cuda:0"})
    db.worker_finished_job(exp, "wg", False)

    g = next(g for g in client.get("/api/gui/dashboard").json()["gpu_workers"]
             if g.get("gpu_uuid") == "GPU-CCC")
    assert g["completed_jobs"] == 0 and g["failed_jobs"] == 1   # current run
    assert g["completed_jobs_total"] == 2
    assert g["failed_jobs_total"] == 1
