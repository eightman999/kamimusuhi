"""Observation API boundaries, without starting an evaluator or GPU work."""
from __future__ import annotations

import copy
import json

import pytest

from experiments.mioba.fba.replicates import replicate_seed
from experiments.mioba.gui.neural_activity import (
    MAX_FRAME_BYTES, TTL_MS, NeuralActivityCache,
)
from experiments.mioba.tests.test_coordinator import _register


PUBLISH = "/api/worker/neural-activity"


def _view(client, genome_id):
    return client.get(f"/api/gui/individual/{genome_id}/neural-activity")


def _claim(client, worker_id="activity-worker"):
    _register(client, worker_id)
    response = client.post("/api/worker/claim", json={"worker_id": worker_id})
    assert response.status_code == 200, response.text
    return response.json()


def _frame(job, worker_id="activity-worker", **updates):
    frame = {
        "schema_version": 1,
        "experiment_id": job["experiment_id"],
        "genome_id": job["genome_id"],
        "job_id": job["job_id"],
        "worker_id": worker_id,
        "attempt": job["attempt"],
        "evaluation_pass": 0,
        "sequence": 0,
        "backend": job["backend"],
        "phase": "evaluation",
        "replicate_index": 0,
        "replicate_seed": job["replicate_seeds"][0],
        "n_neurons": 100,
        "n_base": 90,
        "neuron_indices": [0, 10, 90, 99],
        "spike_counts": [0, 2, 0, 5],
        "window_start_ms": 0.0,
        "window_end_ms": 25.0,
    }
    frame.update(updates)
    return frame


@pytest.fixture
def active_frame(client):
    return _frame(_claim(client))


def _database_dump(service):
    return "\n".join(service.db.conn.iterdump())


def test_roundtrip_zero_activity_and_missing_frame_are_distinct_and_read_only(
        client, service, active_frame):
    cache = client.app.state.neural_activity
    cache.clock = lambda: 20.0
    cache.wall_clock = lambda: 1_700_000_000.25
    before = _database_dump(service)
    missing = _view(client, active_frame["genome_id"]).json()
    assert (missing["kind"], missing["status"], missing["frame"]) == (
        "UNAVAILABLE", "waiting", None)
    assert missing["age_ms"] is None

    active_frame["spike_counts"] = [0, 0, 0, 0]
    response = client.post(PUBLISH, json=active_frame)
    assert response.status_code == 200, response.text
    snapshot = _view(client, active_frame["genome_id"]).json()
    assert (snapshot["kind"], snapshot["status"], snapshot["age_ms"]) == (
        "LIVE", "running", 0)
    assert snapshot["experiment_id"] == service.experiment_id
    assert snapshot["genome_id"] == active_frame["genome_id"]
    assert snapshot["ttl_ms"] == TTL_MS
    assert snapshot["frame"] == {
        **active_frame, "received_at_unix_ms": 1_700_000_000_250,
    }
    assert _database_dump(service) == before


@pytest.mark.parametrize("updates", [
    {"experiment_id": "other-experiment"},
    {"genome_id": "other-genome"},
    {"job_id": "other-job"},
    {"worker_id": "other-worker"},
    {"attempt": 1},
    {"backend": "torch"},
    {"replicate_seed": 0},
    {"replicate_index": 1},
])
def test_identity_and_replicate_mismatches_do_not_enter_cache(
        client, service, active_frame, updates):
    bad = {**active_frame, **updates}
    before = _database_dump(service)
    response = client.post(PUBLISH, json=bad)
    assert response.status_code == 409, response.text
    assert _view(client, active_frame["genome_id"]).json()["frame"] is None
    assert _database_dump(service) == before


@pytest.mark.parametrize("updates", [
    {"schema_version": True},
    {"experiment_id": "unsafe/id"},
    {"genome_id": "x" * 257},
    {"job_id": ""},
    {"worker_id": None},
    {"phase": "training"},
    {"backend": "genn"},
    {"attempt": True},
    {"sequence": -1},
    {"evaluation_pass": 65},
    {"replicate_index": 0.0},
    {"replicate_seed": "123"},
    {"n_neurons": 0},
    {"n_neurons": 10_000_001},
    {"n_base": 101},
    {"n_base": False},
    {"neuron_indices": [0, 10, 90]},
    {"neuron_indices": [], "spike_counts": []},
    {"neuron_indices": [0, 10, 90, 100]},
    {"neuron_indices": [0, 10, 10, 99]},
    {"neuron_indices": [10, 0, 90, 99]},
    {"neuron_indices": [False, 10, 90, 99]},
    {"spike_counts": [0, -1, 0, 0]},
    {"spike_counts": [0, True, 0, 0]},
    {"spike_counts": [0, 2.5, 0, 0]},
    {"neuron_indices": list(range(513)), "spike_counts": [0] * 513,
     "n_neurons": 513},
    {"window_start_ms": True},
    {"window_start_ms": -0.1},
    {"window_end_ms": 0},
    {"window_end_ms": "25"},
    {"window_end_ms": 1e13},
    {"unexpected": "not a frame field"},
])
def test_malformed_fields_are_rejected_without_replacing_a_good_frame(
        client, active_frame, updates):
    assert client.post(PUBLISH, json=active_frame).status_code == 200
    expected = _view(client, active_frame["genome_id"]).json()["frame"]
    bad = {**active_frame, **updates}
    response = client.post(PUBLISH, json=bad)
    assert response.status_code == 400, response.text
    assert _view(client, active_frame["genome_id"]).json()["frame"] == expected


@pytest.mark.parametrize("body, expected", [
    (b"{", 400),
    (b"[]", 400),
    (b"null", 400),
    (b"\xff", 400),
    (b" " * (MAX_FRAME_BYTES + 1), 413),
])
def test_invalid_or_oversized_body_is_rejected(client, body, expected):
    response = client.post(PUBLISH, content=body,
                           headers={"Content-Type": "application/json"})
    assert response.status_code == expected, response.text


@pytest.mark.parametrize("size, expected", [
    (MAX_FRAME_BYTES, 200), (MAX_FRAME_BYTES + 1, 413),
])
def test_body_size_limit_also_applies_to_otherwise_valid_frames(
        client, active_frame, size, expected):
    body = json.dumps(active_frame).encode()
    body += b" " * (size - len(body))
    response = client.post(PUBLISH, content=body,
                           headers={"Content-Type": "application/json"})
    assert response.status_code == expected, response.text


@pytest.mark.parametrize("value", [float("nan"), float("inf"), float("-inf")])
def test_nonfinite_window_is_rejected(client, active_frame, value):
    active_frame["window_end_ms"] = value
    response = client.post(PUBLISH, content=json.dumps(active_frame),
                           headers={"Content-Type": "application/json"})
    assert response.status_code == 400, response.text


def test_sequence_windows_passes_and_replicates_are_monotonic(
        client, service, active_frame):
    service.db._q("UPDATE evaluation_jobs SET replicates=2 WHERE job_id=?",
                  (active_frame["job_id"],))
    service.db.conn.commit()
    assert client.post(PUBLISH, json=active_frame).status_code == 200
    assert client.post(PUBLISH, json=active_frame).status_code == 409
    next_frame = {**active_frame, "sequence": 1,
                  "window_start_ms": 25.0, "window_end_ms": 50.0}
    assert client.post(PUBLISH, json=next_frame).status_code == 200
    for updates in (
        {"sequence": 0},
        {"sequence": 2, "window_start_ms": 24.0},
        {"sequence": 2, "window_start_ms": 50.0, "window_end_ms": 75.0,
         "n_neurons": 101},
        {"sequence": 2, "window_start_ms": 50.0, "window_end_ms": 75.0,
         "n_base": 89},
        {"sequence": 2, "window_start_ms": 50.0, "window_end_ms": 75.0,
         "neuron_indices": [1, 10, 90, 99]},
    ):
        assert client.post(PUBLISH, json={**next_frame, **updates}).status_code == 409

    # An OOM retry stays in the job attempt but begins a new evaluation pass.
    restarted = {**active_frame, "sequence": 2, "evaluation_pass": 1}
    assert client.post(PUBLISH, json=restarted).status_code == 200
    assert client.post(PUBLISH, json={**restarted, "sequence": 3,
                                     "evaluation_pass": 0}).status_code == 409
    job = service.db.get_job(active_frame["job_id"])
    replicate = {**restarted, "sequence": 3, "replicate_index": 1,
                 "replicate_seed": replicate_seed(job["seed"], 1)}
    assert client.post(PUBLISH, json=replicate).status_code == 200
    assert client.post(PUBLISH, json={**restarted, "sequence": 4}).status_code == 409
    assert _view(client, active_frame["genome_id"]).json()["frame"]["sequence"] == 3


def test_freshness_uses_receiver_monotonic_clock_and_finished_job_is_recorded(
        client, service, active_frame):
    cache = client.app.state.neural_activity
    now = [100.0]
    wall = [1_700_000_000.0]
    cache.clock = lambda: now[0]
    cache.wall_clock = lambda: wall[0]
    assert client.post(PUBLISH, json=active_frame).status_code == 200
    now[0] += TTL_MS / 1000
    wall[0] -= 300  # Wall clock adjustment cannot rejuvenate received frames.
    assert _view(client, active_frame["genome_id"]).json()["kind"] == "LIVE"
    now[0] += 0.1
    stale = _view(client, active_frame["genome_id"]).json()
    assert (stale["kind"], stale["status"]) == ("RECORDED", "stale")
    assert stale["age_ms"] > TTL_MS
    service.db._q("UPDATE evaluation_jobs SET status='SUCCEEDED' WHERE job_id=?",
                  (active_frame["job_id"],))
    service.db.conn.commit()
    done = _view(client, active_frame["genome_id"]).json()
    assert (done["kind"], done["status"], done["job_status"]) == (
        "RECORDED", "recorded", "SUCCEEDED")
    assert done["frame"] == stale["frame"]
    assert client.post(PUBLISH, json={**active_frame, "sequence": 1}).status_code == 409


def test_requeue_and_new_job_cannot_resurrect_old_attempt_or_other_genome(
        client, service, active_frame):
    assert client.post(PUBLISH, json=active_frame).status_code == 200
    other_job = _claim(client, "other-worker")
    assert other_job["genome_id"] != active_frame["genome_id"]
    assert _view(client, other_job["genome_id"]).json()["frame"] is None
    assert _view(client, "unknown-genome").status_code == 404

    service.db._q("UPDATE evaluation_jobs SET status='UNKNOWN', priority=100 "
                  "WHERE job_id=?", (active_frame["job_id"],))
    service.db.conn.commit()
    unknown = _view(client, active_frame["genome_id"]).json()
    assert (unknown["kind"], unknown["status"]) == ("RECORDED", "stale")
    service.db.requeue_unknown(service.experiment_id, 3)
    waiting = _view(client, active_frame["genome_id"]).json()
    assert (waiting["status"], waiting["frame"]) == ("waiting", None)
    retry = _claim(client, "retry-worker")
    assert retry["job_id"] == active_frame["job_id"]
    assert retry["attempt"] == active_frame["attempt"] + 1
    assert _view(client, active_frame["genome_id"]).json()["frame"] is None
    assert client.post(PUBLISH, json={**active_frame, "sequence": 1}).status_code == 409
    retry_frame = _frame(retry, "retry-worker")
    assert client.post(PUBLISH, json=retry_frame).status_code == 200
    assert _view(client, active_frame["genome_id"]).json()["frame"]["attempt"] == 1

    # A later evaluation of the same genome must await its own frame.
    service.db._q("UPDATE evaluation_jobs SET status='SUCCEEDED' WHERE job_id=?",
                  (retry["job_id"],))
    service.db.conn.commit()
    later_id = service.db.enqueue_job(
        service.experiment_id, retry["genome_id"], retry["environment_id"],
        retry["seed"], "screen", retry["backend"], retry["duration_ms"], [],
        priority=200)
    assert _view(client, retry["genome_id"]).json()["frame"] is None
    later_job = _claim(client, "later-worker")
    assert later_job["job_id"] == later_id
    assert _view(client, retry["genome_id"]).json()["frame"] is None


def test_cache_is_bounded_and_observations_are_detached_copies(client, service):
    cache = NeuralActivityCache(service, max_jobs=2)
    client.app.state.neural_activity = cache
    frames = [_frame(_claim(client, f"worker-{i}"), f"worker-{i}")
              for i in range(3)]
    originals = copy.deepcopy(frames)
    for frame in frames:
        cache.publish(frame)
        frame["spike_counts"][0] = 999
    assert cache.snapshot(originals[0]["genome_id"])["frame"] is None
    for frame in originals[1:]:
        snapshot = cache.snapshot(frame["genome_id"])
        assert snapshot["frame"]["spike_counts"] == frame["spike_counts"]
        snapshot["frame"]["spike_counts"][0] = 777
        assert cache.snapshot(frame["genome_id"])["frame"]["spike_counts"][0] == 0
    before = _database_dump(service)
    assert _view(client, originals[0]["genome_id"]).json()["frame"] is None
    assert _database_dump(service) == before
