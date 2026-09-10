import time

from experiments.mioba.tests.conftest import run_worker_once


def _register(client, worker_id):
    r = client.post("/api/worker/register", json={
        "worker_id": worker_id, "hostname": "h", "gpu": [],
        "runtime_info": {}, "bench": []})
    assert r.status_code == 200


def test_status(client):
    s = client.get("/api/status").json()
    assert s["kind"] == "LIVE" and s["status"] == "running"
    assert s["counters"]["queued"] == 4
    assert s["population_size"] == 4


def test_control_requires_token(client, service):
    assert client.post("/api/control/pause").status_code == 401
    r = client.post("/api/control/pause",
                    headers={"X-Mioba-Token": service.token})
    assert r.status_code == 200


def test_two_workers_claim_different_jobs(client):
    _register(client, "w1")
    _register(client, "w2")
    r1 = client.post("/api/worker/claim", json={"worker_id": "w1"})
    r2 = client.post("/api/worker/claim", json={"worker_id": "w2"})
    assert r1.status_code == r2.status_code == 200
    assert r1.json()["job_id"] != r2.json()["job_id"]


def test_genome_evaluation_flow(client, service):
    _register(client, "w1")
    body = run_worker_once(client, "w1")
    assert body["status"] == "SUCCEEDED"
    evals = client.get("/api/evaluations").json()["evaluations"]
    assert len(evals) == 1
    assert evals[0]["fitness"] is not None
    gid = evals[0]["genome_id"]
    g = client.get(f"/api/genomes/{gid}").json()
    assert g["phenotype"]["base"]["name"] == "flywire-v783-shiu-lif"
    anc = client.get(f"/api/genomes/{gid}/ancestry").json()["ancestry"]
    assert anc[-1] == gid
    events = client.get("/api/events", params={"type": "evaluation_succeeded"}
                        ).json()["events"]
    assert events


def test_pause_blocks_claims(client, service):
    client.post("/api/control/pause",
                headers={"X-Mioba-Token": service.token})
    r = client.post("/api/worker/claim", json={"worker_id": "w1"})
    assert r.status_code == 204
    client.post("/api/control/resume",
                headers={"X-Mioba-Token": service.token})
    r = client.post("/api/worker/claim", json={"worker_id": "w1"})
    assert r.status_code == 200


def test_stop_marks_stopped_and_checkpoints(client, service):
    client.post("/api/control/stop", json={"timeout_s": 0.5},
                headers={"X-Mioba-Token": service.token})
    deadline = time.time() + 5
    while time.time() < deadline:
        if service.db.get_experiment(service.experiment_id)["status"] == "stopped":
            break
        time.sleep(0.2)
    exp = service.db.get_experiment(service.experiment_id)
    assert exp["status"] == "stopped"
    assert service.db.list_checkpoints(service.experiment_id)


def test_worker_loss_marks_unknown_and_requeues(client, service):
    _register(client, "doomed")
    _register(client, "survivor")
    r = client.post("/api/worker/claim", json={"worker_id": "doomed"})
    job_id = r.json()["job_id"]
    # stale the worker's heartbeat and run detection
    past = "2000-01-01T00:00:00+00:00"
    service.db._q("UPDATE worker_runs SET last_heartbeat_at=? WHERE "
                  "worker_id=?", (past, "doomed"))
    service.db.conn.commit()
    service._detect_stale_workers()
    job = service.db.get_job(job_id)
    assert job["status"] == "UNKNOWN"
    assert service.db.get_worker(service.experiment_id, "doomed")["status"] \
        == "lost"
    service.db.requeue_unknown(service.experiment_id, 3)
    assert service.db.get_job(job_id)["status"] == "QUEUED"
    # survivor can still claim and complete
    body = run_worker_once(client, "survivor")
    assert body["status"] == "SUCCEEDED"


def test_worker_never_marks_running_succeeded(client, service):
    _register(client, "w1")
    r = client.post("/api/worker/claim", json={"worker_id": "w1"})
    jid = r.json()["job_id"]
    result = {"job_id": jid, "worker_id": "w1", "status": "SUCCEEDED",
              "evaluation": {"summary": {"mean_rate_hz": 1.0}},
              "error": None}
    assert client.post("/api/worker/result", json=result).status_code == 200
    # a second result for the same job must not flip it back
    r2 = client.post("/api/worker/result", json=result)
    assert r2.status_code == 409
    # and a stale-claimed job can never be marked succeeded behind the
    # coordinator's back: status stays SUCCEEDED from the first result
    assert service.db.get_job(jid)["status"] == "SUCCEEDED"


def test_sigterm_mid_job_reports_failed_not_running(client, service):
    """A worker stopped mid-job reports FAILED(interrupted), never leaves
    the job RUNNING."""
    from experiments.mioba.workers import worker as w
    _register(client, "w1")
    r = client.post("/api/worker/claim", json={"worker_id": "w1"})
    jid = r.json()["job_id"]
    w._stop.set()
    try:
        body = w.run_job(client, "w1", r.json(), "cpu", 1, grace_s=0.0)
    finally:
        w._stop.clear()
    assert body["status"] == "FAILED"
    assert "interrupted" in body["error"]
    assert service.db.get_job(jid)["status"] == "FAILED"


def test_resume_config_hash_mismatch(tmp_path, smoke_config):
    import copy
    from experiments.mioba.coordinator.service import MiobaService

    cfg = copy.deepcopy(smoke_config)
    svc = MiobaService(cfg, tmp_path / "runs")
    exp_id = svc.experiment_id
    stored = svc.config_hash
    svc.db.close()

    cfg2 = copy.deepcopy(smoke_config)
    cfg2["mie"]["interval_s"] = 9  # operator-tuned interval
    svc2 = MiobaService(cfg2, tmp_path / "runs", resume=exp_id)
    assert svc2.config_hash != stored
    evs = svc2.db.list_events(exp_id, type_="config_hash_mismatch")
    assert evs and evs[-1]["severity"] == "warn"
    assert evs[-1]
    s = svc2.status()
    assert s["config_hash_stored"] == stored
    assert s["config_hash"] == svc2.config_hash
    svc2.db.close()
