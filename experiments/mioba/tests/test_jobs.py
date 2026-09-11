import pytest

from experiments.mioba.storage.db import Database, InvalidTransition


@pytest.fixture
def db(tmp_path):
    d = Database(tmp_path / "db.sqlite")
    d.create_experiment("e", {}, "h", None)
    yield d
    d.close()


def test_stale_result_rejected(service):
    """A claims; job UNKNOWN->requeued; B claims; A's late result rejected."""
    db = service.db
    exp = service.experiment_id
    # clear the seeded queue so the claim below is deterministic
    for j in db.list_jobs(exp, status="QUEUED", limit=10000):
        db.cancel_job(j["job_id"])
    gid = db.list_genomes(exp)[0]["genome_id"]
    jid = db.enqueue_job(exp, gid, "env", 0, "smoke", "mock", 10, [])

    job_a = db.claim_job(exp, "workerA")
    assert job_a["job_id"] == jid
    db.mark_running_unknown(exp, [jid])
    db.requeue_unknown(exp, max_attempts=3)
    job_b = db.claim_job(exp, "workerB")
    assert job_b["job_id"] == jid
    assert job_b["claimed_by_worker"] == "workerB"

    with pytest.raises(InvalidTransition):
        service.worker_result(jid, "workerA", "SUCCEEDED",
                              {"summary": {"mean_rate_hz": 1.0}}, None)
    assert db.list_evaluations(exp, genome_id=gid) == []
    evs = db.list_events(exp, type_="stale_result_rejected")
    assert evs and evs[-1]["severity"] == "warn"

    service.worker_result(jid, "workerB", "SUCCEEDED",
                          {"summary": {"mean_rate_hz": 1.0}}, None)
    assert db.get_job(jid)["status"] == "SUCCEEDED"
    assert len(db.list_evaluations(exp, genome_id=gid)) == 1


def _job(db, jid=None):
    return db.enqueue_job("e", "g1", "env", 0, "smoke", "mock", 10, [],
                          job_id=jid)


def test_claim_atomic(db):
    jid = _job(db)
    a = db.claim_job("e", "w1")
    b = db.claim_job("e", "w2")
    assert a is not None and a["job_id"] == jid
    assert b is None
    assert db.get_job(jid)["claimed_by_worker"] == "w1"


def test_running_to_unknown_and_requeue(db):
    jid = _job(db)
    db.claim_job("e", "w1")
    ids = db.mark_running_unknown("e")
    assert ids == [jid]
    assert db.get_job(jid)["status"] == "UNKNOWN"
    out = db.requeue_unknown("e", max_attempts=3)
    assert out == [(jid, "requeued")]
    job = db.get_job(jid)
    assert job["status"] == "QUEUED" and job["attempt"] == 1
    assert job["claimed_by_worker"] is None


def test_max_attempts_fails(db):
    jid = _job(db)
    for _ in range(2):
        db.claim_job("e", "w1")
        db.mark_running_unknown("e")
        db.requeue_unknown("e", max_attempts=3)
    db.claim_job("e", "w1")
    db.mark_running_unknown("e")
    out = db.requeue_unknown("e", max_attempts=3)
    assert out == [(jid, "failed")]
    assert db.get_job(jid)["status"] == "FAILED"
