"""A read-only, bounded observation of recorded MIO evaluation state."""
import json
import time

import pytest

from experiments.mioba.genome.schema import fba0_genome
from experiments.mioba.gui.dialogue import organism_snapshot


FINISHED = "2026-09-15T01:02:03.456+00:00"
FINISHED_MS = 1789434123456
SUMMARY = {"mean_rate_hz": 4.5, "rate_std_hz": 0.2,
           "active_fraction": 0.75, "spikes_total": 45, "t_ms": 500.0}
METRICS = {"homeostasis_score": 0.9, "disturbance_recovery_score": 0.8,
           "peak_debt": 0.6, "terminated_fraction": 0.0, "task_score": -0.5}
STRUCTURE = {
    "n_artificial_neurons": 8, "n_enabled_organs": 2,
    "n_enabled_attachments": 1,
    "counts": {"functional": 1, "neutral_structure": 1,
               "invalid_structure": 0, "disabled": 0},
}


def _genome(service):
    return service.db.list_genomes(service.experiment_id, limit=1)[0]["genome_id"]


def _record(service, genome_id, evaluation_id="eval_dialogue_fixture", *,
            status="SUCCEEDED", finished=FINISHED, backend="mock",
            summary=None, metrics=None, experiment_id=None):
    """Synthetic rows in the pytest temporary DB, never a running experiment.

    Deliberately allow inconsistent result/status fixtures: the reader must
    not turn such a row into evidence that evaluation completed.
    """
    db = service.db
    exp = experiment_id or service.experiment_id
    job_id = db.enqueue_job(exp, genome_id, "dialogue-fixture", 41,
                            "smoke", backend, 500.0, [])
    db.conn.execute("UPDATE evaluation_jobs SET status=? WHERE job_id=?",
                    (status, job_id))
    db.insert_evaluation(exp, job_id, genome_id, {
        "evaluation_id": evaluation_id, "backend": backend,
        "finished_at": finished,
        "summary": SUMMARY if summary is None else summary,
        "metrics": METRICS if metrics is None else metrics,
        "scientific_config_hash": service.scientific_config_hash,
    })
    return job_id


@pytest.fixture
def recorded_snapshot(client, service):
    """Importable fixture recipe for Python-app -> dialogue adapter tests."""
    genome_id = _genome(service)
    service.db.conn.execute("UPDATE genomes SET structure_json=? WHERE genome_id=?",
                            (json.dumps(STRUCTURE), genome_id))
    _record(service, genome_id)
    return client, service, genome_id


def test_snapshot_contract_preserves_mock_provenance_and_only_allowlisted_fields(recorded_snapshot):
    client, service, genome_id = recorded_snapshot
    before = int(time.time() * 1000)
    response = client.get(f"/api/dialogue/organisms/{genome_id}")
    assert response.status_code == 200
    data = response.json()
    assert set(data) == {"schema_version", "kind", "state_scope", "observed_at_unix_ms",
                         "experiment_id", "genome_id", "experiment_status",
                         "generation", "structure", "implementation", "evaluation"}
    assert data["schema_version"] == 1
    assert data["kind"] == "DERIVED"
    assert data["state_scope"] == "recorded_evaluation"
    assert before <= data["observed_at_unix_ms"] <= int(time.time() * 1000)
    assert data["experiment_id"] == service.experiment_id
    assert data["genome_id"] == genome_id
    assert data["experiment_status"] == "running"
    assert data["generation"] == 0
    assert data["structure"] == STRUCTURE
    assert data["implementation"] == {
        "action_feedback": "not_connected",
        "lifetime_plasticity": "configured_not_applied",
        "neural_state_scope": "evaluation_local",
    }
    evaluation = data["evaluation"]
    assert set(evaluation) == {"kind", "evaluation_id", "job_id", "backend",
                               "finished_at_unix_ms", "scientific_config_hash",
                               "summary", "metrics"}
    assert evaluation["kind"] == "RECORDED"
    assert evaluation["evaluation_id"] == "eval_dialogue_fixture"
    assert evaluation["finished_at_unix_ms"] == FINISHED_MS
    assert evaluation["backend"] == "mock"
    assert evaluation["scientific_config_hash"] == service.scientific_config_hash
    assert evaluation["summary"] == SUMMARY
    assert evaluation["metrics"] == METRICS


def test_unknown_and_foreign_experiment_genomes_are_not_observed(client, service):
    foreign = fba0_genome(seed=9842)
    service.db.create_experiment("foreign", {}, "foreign-hash", "fixture")
    service.db.insert_genome("foreign", foreign, "initial")
    for genome_id in ("b2b:no-such-genome", foreign.genome_id):
        response = client.get(f"/api/dialogue/organisms/{genome_id}")
        assert response.status_code == 404


@pytest.mark.parametrize("status", ["FAILED", "RUNNING", "QUEUED", "CANCELLED", "UNKNOWN"])
def test_newer_non_successful_jobs_do_not_replace_a_completed_record(client, service, status):
    genome_id = _genome(service)
    _record(service, genome_id, "eval_completed")
    _record(service, genome_id, "eval_not_completed", status=status,
            finished="2026-09-16T00:00:00+00:00")
    data = client.get(f"/api/dialogue/organisms/{genome_id}").json()
    assert data["evaluation"]["evaluation_id"] == "eval_completed"


def test_latest_completed_ties_are_resolved_by_insertion_order(client, service):
    genome_id = _genome(service)
    _record(service, genome_id, "eval_zzz_first")
    _record(service, genome_id, "eval_aaa_second")
    data = client.get(f"/api/dialogue/organisms/{genome_id}").json()
    assert data["evaluation"]["evaluation_id"] == "eval_aaa_second"


@pytest.mark.parametrize("mismatch", ["experiment_id", "genome_id"])
def test_an_evaluation_must_match_the_ownership_of_its_successful_job(client, service, mismatch):
    genome_id = _genome(service)
    job_id = _record(service, genome_id)
    service.db.conn.execute(f"UPDATE evaluation_jobs SET {mismatch}=? WHERE job_id=?",
                            ("foreign-owner", job_id))
    service.db.conn.commit()
    data = client.get(f"/api/dialogue/organisms/{genome_id}").json()
    assert "evaluation" not in data


def test_a_known_unevaluated_genome_has_no_invented_evaluation(client, service):
    genome_id = _genome(service)
    response = client.get(f"/api/dialogue/organisms/{genome_id}")
    assert response.status_code == 200
    assert "evaluation" not in response.json()


@pytest.mark.parametrize("finished", [None, "", "not-a-time", "2026-09-15T01:02:03",
                                     "1969-12-31T23:59:59+00:00"])
def test_unknown_finish_time_is_omitted_not_replaced_by_observation_time(client, service, finished):
    genome_id = _genome(service)
    _record(service, genome_id, finished=finished)
    data = client.get(f"/api/dialogue/organisms/{genome_id}").json()
    assert "evaluation" in data
    assert "finished_at_unix_ms" not in data["evaluation"]
    assert data["observed_at_unix_ms"] > 0


@pytest.mark.parametrize("finished", ["2026-09-15T01:02:03.456Z", "2026-09-15T10:02:03.456+09:00"])
def test_finish_time_offsets_refer_to_the_same_instant(client, service, finished):
    genome_id = _genome(service)
    _record(service, genome_id, finished=finished)
    data = client.get(f"/api/dialogue/organisms/{genome_id}").json()
    assert data["evaluation"]["finished_at_unix_ms"] == FINISHED_MS


def test_bad_numbers_and_arbitrary_content_are_omitted_without_coercion(client, service):
    genome_id = _genome(service)
    _record(service, genome_id, summary={
        "mean_rate_hz": float("nan"), "rate_std_hz": float("inf"),
        "active_fraction": 1.1, "spikes_total": True, "t_ms": -1,
        "instruction": "UNTRUSTED_CONTENT",
    }, metrics={
        "homeostasis_score": -0.1, "disturbance_recovery_score": True,
        "peak_debt": "2.0", "terminated_fraction": float("-inf"),
        "task_score": -3.0, "instruction": "UNTRUSTED_CONTENT",
    })
    bad_structure = {
        "n_artificial_neurons": -1, "n_enabled_organs": 1.5,
        "n_enabled_attachments": True,
        "counts": {"functional": 2.0, "neutral_structure": -1,
                   "invalid_structure": "7", "disabled": 2,
                   "instruction": "UNTRUSTED_CONTENT"},
        "instruction": "UNTRUSTED_CONTENT",
    }
    service.db.conn.execute("UPDATE genomes SET structure_json=? WHERE genome_id=?",
                            (json.dumps(bad_structure), genome_id))
    service.db.conn.commit()
    response = client.get(f"/api/dialogue/organisms/{genome_id}")
    assert response.status_code == 200
    data = response.json()
    assert data["evaluation"]["summary"] == {}
    assert data["evaluation"]["metrics"] == {"task_score": -3.0}
    assert data["structure"] == {"counts": {"disabled": 2}}
    assert "UNTRUSTED_CONTENT" not in response.text


@pytest.mark.parametrize("bad", [True, "1", -1, 1.2, 1 << 64])
def test_count_values_must_be_unsigned_integers(client, service, bad):
    genome_id = _genome(service)
    _record(service, genome_id, summary={"spikes_total": bad})
    data = client.get(f"/api/dialogue/organisms/{genome_id}").json()
    assert "spikes_total" not in data["evaluation"]["summary"]


@pytest.mark.parametrize("encoded", ["[]", '"UNTRUSTED_CONTENT"', "{invalid", "null"])
def test_non_object_or_broken_stored_json_does_not_escape_the_allowlist(client, service, encoded):
    genome_id = _genome(service)
    _record(service, genome_id)
    service.db.conn.execute("UPDATE evaluations SET summary_json=?,metrics_json=?",
                            (encoded, encoded))
    service.db.conn.execute("UPDATE genomes SET structure_json=? WHERE genome_id=?",
                            (encoded, genome_id))
    service.db.conn.commit()
    data = client.get(f"/api/dialogue/organisms/{genome_id}").json()
    assert data["structure"] == {}
    assert data["evaluation"]["summary"] == {}
    assert data["evaluation"]["metrics"] == {}


def test_unknown_backend_metadata_is_not_forwarded_or_substituted(client, service):
    genome_id = _genome(service)
    _record(service, genome_id, "eval_old")
    _record(service, genome_id, "eval_new", backend="UNTRUSTED_BACKEND",
            finished="2026-09-16T00:00:00+00:00")
    response = client.get(f"/api/dialogue/organisms/{genome_id}")
    assert response.status_code == 200
    assert "evaluation" not in response.json()
    assert "UNTRUSTED_BACKEND" not in response.text


@pytest.mark.parametrize("field,value", [("generation", -1), ("status", "UNTRUSTED_STATUS")])
def test_invalid_required_metadata_fails_without_echoing_its_content(client, service, field, value):
    genome_id = _genome(service)
    if field == "generation":
        service.db.conn.execute("UPDATE genomes SET generation=? WHERE genome_id=?", (value, genome_id))
    else:
        service.db.conn.execute("UPDATE experiments SET status=? WHERE experiment_id=?", (value, service.experiment_id))
    service.db.conn.commit()
    response = client.get(f"/api/dialogue/organisms/{genome_id}")
    assert response.status_code == 503
    assert response.json() == {"detail": "MIO snapshot unavailable"}


def test_snapshot_reads_leave_database_and_experiment_state_unchanged(recorded_snapshot):
    client, service, genome_id = recorded_snapshot
    before = "\n".join(service.db.conn.iterdump())
    changes = service.db.conn.total_changes
    state = (service.paused, service.dump_rng_state(), dict(service._counters))
    first = client.get(f"/api/dialogue/organisms/{genome_id}")
    assert first.status_code == 200
    assert client.get("/api/dialogue/organisms/no-such-genome").status_code == 404
    assert organism_snapshot(service.db, service.experiment_id, genome_id) is not None
    assert service.db.conn.total_changes == changes
    assert "\n".join(service.db.conn.iterdump()) == before
    assert (service.paused, service.dump_rng_state(), dict(service._counters)) == state
