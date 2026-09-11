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
        db.finish_job(jid, "SUCCEEDED", "w1")  # not RUNNING yet
    job = db.claim_job("exp1", "w1")
    assert job["job_id"] == jid and job["status"] == "RUNNING"
    with pytest.raises(InvalidTransition):
        db.finish_job(jid, "SUCCEEDED", "w-other")  # wrong worker
    db.finish_job(jid, "FAILED", "w1", "boom")
    with pytest.raises(InvalidTransition):
        db.cancel_job(jid)  # terminal
    with pytest.raises(InvalidTransition):
        db.finish_job(jid, "SUCCEEDED", "w1")


def test_cancel_queued(db):
    jid = db.enqueue_job("exp1", "g", "env", 0, "smoke", "mock", 10, [])
    db.cancel_job(jid)
    assert db.get_job(jid)["status"] == "CANCELLED"


def test_list_genomes_view(db):
    base = fba0_genome(seed=2)
    cid = db.create_clade("exp1", "fba0-root", None)
    db.insert_genome("exp1", base, "initial", clade_id=cid)
    rng = random.Random(1)
    child = mutate(base, rng, birth_index=1, generation=1)
    db.insert_genome("exp1", child, "mutation")
    jid = db.enqueue_job("exp1", base.genome_id, "env", 1, "smoke", "mock",
                         10, [])
    db.claim_job("exp1", "w1")
    db.finish_job(jid, "SUCCEEDED", "w1")
    db.insert_evaluation("exp1", jid, base.genome_id,
                         {"worker_id": "w1", "fitness": -2.5,
                          "summary": {"mean_rate_hz": 7.5}})
    view = db.list_genomes_view("exp1")
    assert len(view) == 2
    base_row = next(v for v in view if v["genome_id"] == base.genome_id)
    assert base_row["clade_name"] == "fba0-root"
    assert base_row["children_count"] == 1
    assert base_row["best_fitness"] == -2.5
    assert base_row["organ_count"] == len(base.artificial_organs)
    assert base_row["ancestry_fraction"] in (1.0, None)
    assert base_row["parent_ids"] == []
    assert db.count_evaluations("exp1") == 1


def test_children_and_ancestry_chain(db):
    base = fba0_genome(seed=3)
    db.insert_genome("exp1", base, "initial")
    rng = random.Random(9)
    child = mutate(base, rng, birth_index=1, generation=1)
    db.insert_genome("exp1", child, "mutation")
    assert db.children_of(base.genome_id) == [child.genome_id]
    chain = db.ancestry_chain(child.genome_id)
    assert [c["genome_id"] for c in chain] == [base.genome_id,
                                             child.genome_id]
    assert chain[0]["generation"] == 0 and chain[1]["generation"] == 1
    assert db.ancestry_chain("nonexistent") == []


def test_telemetry_and_heartbeat_series(db):
    db.insert_telemetry("exp1", [
        {"timestamp": "2026-01-01T00:00:01+00:00", "source": "host:cpu",
         "domain": "machine_interoception", "signal_type": "cpu_percent",
         "value": 10.0, "confidence": 1.0},
        {"timestamp": "2026-01-01T00:00:02+00:00", "source": "host:ram",
         "domain": "machine_interoception", "signal_type": "ram_percent",
         "value": 20.0, "confidence": 1.0},
        {"timestamp": "2026-01-01T00:00:03+00:00", "source": "host:cpu",
         "domain": "machine_interoception", "signal_type": "cpu_percent",
         "value": 30.0, "confidence": 1.0},
    ])
    rows = db.telemetry_series("exp1", None, ["cpu_percent"])
    assert [r["value"] for r in rows] == [10.0, 30.0]
    rows = db.telemetry_series("exp1", "2026-01-01T00:00:02+00:00", None)
    assert [r["value"] for r in rows] == [20.0, 30.0]

    db.register_worker("exp1", "w1", "h", [], {}, [], 1)
    db.heartbeat("exp1", "w1", {"temperature_c": 40.0})
    db.heartbeat("exp1", "w1", {"temperature_c": 41.0})
    series = db.worker_heartbeat_series("exp1", "w1", None)
    assert len(series) == 2
    assert series[0]["at"] <= series[1]["at"]
    assert db.worker_heartbeat_series("exp1", "w1",
                                      "2999-01-01T00:00:00+00:00") == []
