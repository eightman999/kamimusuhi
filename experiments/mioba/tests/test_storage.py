import random

import pytest

from experiments.mioba.genome.mutation import mutate
from experiments.mioba.genome.schema import fba0_genome
from experiments.mioba.storage.db import Database, InvalidTransition


@pytest.fixture
def db(tmp_path):
    d = Database(tmp_path / "lineage.sqlite")
    d.create_experiment("exp1", {"a": 1}, "hash", "abc")
    yield d
    d.close()


def test_schema_and_experiment(db):
    exp = db.get_experiment("exp1")
    assert exp["status"] == "created"


def test_genome_parents_ancestry(db):
    base = fba0_genome(seed=5)
    db.insert_genome("exp1", base, "initial")
    rng = random.Random(0)
    child = mutate(base, rng, birth_index=1, generation=1)
    db.insert_genome("exp1", child, "mutation")
    assert db.genome_parents(child.genome_id) == [base.genome_id]
    assert db.genome_children(base.genome_id) == [child.genome_id]
    anc = db.ancestry(child.genome_id)
    assert anc[0] == base.genome_id and anc[-1] == child.genome_id


def test_events(db):
    eid = db.emit("exp1", "genome_born", payload={"x": 1})
    rows = db.list_events("exp1", since_id=0)
    assert rows[-1]["event_id"] == eid
    assert db.list_events("exp1", since_id=eid) == []


def test_job_transitions(db):
    base = fba0_genome()
    db.insert_genome("exp1", base, "initial")
    jid = db.enqueue_job("exp1", base.genome_id, "env", 1, "smoke", "mock",
                         100, [])
    with pytest.raises(InvalidTransition):
        db.finish_job(jid, "SUCCEEDED")  # not RUNNING yet
    job = db.claim_job("exp1", "w1")
    assert job["job_id"] == jid and job["status"] == "RUNNING"
    db.finish_job(jid, "FAILED", "boom")
    with pytest.raises(InvalidTransition):
        db.cancel_job(jid)  # terminal
    with pytest.raises(InvalidTransition):
        db.finish_job(jid, "SUCCEEDED")


def test_cancel_queued(db):
    jid = db.enqueue_job("exp1", "g", "env", 0, "smoke", "mock", 10, [])
    db.cancel_job(jid)
    assert db.get_job(jid)["status"] == "CANCELLED"
