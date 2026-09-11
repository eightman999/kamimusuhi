"""Blocker 5: `mioba replay` reconstructs the recorded conditions and
never falls back to synthetic for a real-FBA recording."""
import copy
import json

import pytest

from experiments.mioba.coordinator.replay import (ReplayConfigMismatch,
                                                  ReplayUnavailable,
                                                  build_plan, run_plan)
from experiments.mioba.coordinator.service import MiobaService
from experiments.mioba.tests.conftest import run_worker_once


def _one_eval(tmp_path, cfg, device="cpu"):
    svc = MiobaService(cfg, tmp_path / "runs")
    from fastapi.testclient import TestClient
    from experiments.mioba.coordinator.app import create_app
    client = TestClient(create_app(svc))
    client.post("/api/worker/register",
                json={"worker_id": "w", "hostname": "h", "gpu": [],
                      "runtime_info": {}, "bench": [], "batch_size": 1})
    run_worker_once(client, "w", device=device)
    ev = svc.db.list_evaluations(svc.experiment_id, limit=1)[0]
    exp_dir = svc.run_dir
    svc.db.close()
    return exp_dir, ev


def test_replay_mock_uses_recorded_conditions_and_is_identical(tmp_path,
                                                                smoke_config):
    exp_dir, ev = _one_eval(tmp_path, copy.deepcopy(smoke_config))
    plan = build_plan(exp_dir, ev["evaluation_id"], current_config=smoke_config)
    assert plan.backend == "mock" and plan.seed == ev["seed"]
    assert plan.duration_ms == 100.0
    assert plan.environment_id == "synthetic-quiet-v0"
    assert plan.backend_kwargs == {"n_neurons": 512, "connectivity": 0.02}
    assert plan.warnings == []
    res = run_plan(plan)
    assert res["diff"]["identical_spike_counts"] is True
    assert res["diff"]["original_spikes"] == res["diff"]["replay_spikes"]
    assert res["identity"]["genome_hash"] == ev["genome_id"]


def test_replay_torch_synthetic_reconstructs_network(tmp_path, smoke_config):
    pytest.importorskip("torch")
    cfg = copy.deepcopy(smoke_config)
    cfg["evaluation"]["backend"] = "torch"
    cfg["fba"]["synthetic_neurons"] = 300
    exp_dir, ev = _one_eval(tmp_path, cfg)
    ds = json.loads(ev["dataset_json"])
    assert ds["dataset_id"] == "synthetic-fba" and ds["region_mode"]
    plan = build_plan(exp_dir, ev["evaluation_id"])
    assert plan.backend == "torch"
    assert plan.backend_kwargs["synthetic_neurons"] == 300
    assert plan.backend_kwargs["region_mode"] == ds["region_mode"]
    res = run_plan(plan)
    assert res["diff"]["identical_spike_counts"] is True
    assert res["dataset"] == ds


def test_replay_real_fba_without_dataset_fails_explicitly(tmp_path,
                                                          smoke_config):
    exp_dir, ev = _one_eval(tmp_path, copy.deepcopy(smoke_config))
    # rewrite the record as if it had been a real-FlyWire torch evaluation
    import sqlite3
    con = sqlite3.connect(str(exp_dir / "lineage.sqlite"))
    con.execute("UPDATE evaluations SET dataset_json=?, backend='torch' "
                "WHERE evaluation_id=?",
                (json.dumps({"dataset_id": "flywire-v783-shiu-lif",
                             "version": "2025_783",
                             "manifest_hash": "deadbeef",
                             "region_mode": None}), ev["evaluation_id"]))
    con.commit(); con.close()
    with pytest.raises(ReplayUnavailable, match="required dataset "
                       "flywire-v783-shiu-lif@2025_783 .* is not available"):
        build_plan(exp_dir, ev["evaluation_id"])
    with pytest.raises(ReplayUnavailable):
        build_plan(exp_dir, ev["evaluation_id"],
                   data_dir=str(tmp_path / "does-not-exist"))


def test_replay_manifest_mismatch_is_rejected(tmp_path, smoke_config):
    pytest.importorskip("torch")
    cfg = copy.deepcopy(smoke_config)
    cfg["evaluation"]["backend"] = "torch"
    cfg["fba"]["synthetic_neurons"] = 200
    exp_dir, ev = _one_eval(tmp_path, cfg)
    import sqlite3
    con = sqlite3.connect(str(exp_dir / "lineage.sqlite"))
    ds = json.loads(ev["dataset_json"])
    ds["version"] = "v0-n999-p0.01"  # a different synthetic network
    con.execute("UPDATE evaluations SET dataset_json=? WHERE evaluation_id=?",
                (json.dumps(ds), ev["evaluation_id"]))
    con.commit(); con.close()
    plan = build_plan(exp_dir, ev["evaluation_id"])
    assert plan.backend_kwargs["synthetic_neurons"] == 999
    # backend reports the identity it actually built -> matches the record
    assert run_plan(plan)["dataset"]["version"] == "v0-n999-p0.01"
    ds["version"] = "not-a-version"
    con = sqlite3.connect(str(exp_dir / "lineage.sqlite"))
    con.execute("UPDATE evaluations SET dataset_json=? WHERE evaluation_id=?",
                (json.dumps(ds), ev["evaluation_id"]))
    con.commit(); con.close()
    with pytest.raises(ReplayUnavailable):
        build_plan(exp_dir, ev["evaluation_id"])


def test_replay_legacy_record_without_dataset_identity(tmp_path,
                                                       smoke_config):
    exp_dir, ev = _one_eval(tmp_path, copy.deepcopy(smoke_config))
    import sqlite3
    con = sqlite3.connect(str(exp_dir / "lineage.sqlite"))
    con.execute("UPDATE evaluations SET dataset_json='{}' WHERE evaluation_id=?",
                (ev["evaluation_id"],))
    con.commit(); con.close()
    with pytest.raises(ReplayUnavailable, match="no recorded dataset"):
        build_plan(exp_dir, ev["evaluation_id"])


def test_replay_config_drift_warns_or_stops_in_strict(tmp_path, smoke_config):
    exp_dir, ev = _one_eval(tmp_path, copy.deepcopy(smoke_config))
    drift = copy.deepcopy(smoke_config)
    drift["evaluation"]["duration_ms"] = 999
    plan = build_plan(exp_dir, ev["evaluation_id"], current_config=drift)
    assert any("scientific_config_hash differs" in w for w in plan.warnings)
    assert plan.duration_ms == 100.0  # recorded value wins over current cfg
    with pytest.raises(ReplayConfigMismatch):
        build_plan(exp_dir, ev["evaluation_id"], current_config=drift,
                   strict=True)
    ops = copy.deepcopy(smoke_config)
    ops["worker"]["heartbeat_s"] = 42
    assert build_plan(exp_dir, ev["evaluation_id"], current_config=ops,
                      strict=True).warnings == []


def test_replay_backend_override_is_flagged(tmp_path, smoke_config):
    exp_dir, ev = _one_eval(tmp_path, copy.deepcopy(smoke_config))
    plan = build_plan(exp_dir, ev["evaluation_id"], backend="torch")
    assert any("backend override" in w for w in plan.warnings)
    with pytest.raises(ReplayConfigMismatch):
        build_plan(exp_dir, ev["evaluation_id"], backend="torch", strict=True)


def test_cli_replay_exit_codes(tmp_path, smoke_config):
    from experiments.mioba.cli import build_parser
    cfg = copy.deepcopy(smoke_config)
    exp_dir, ev = _one_eval(tmp_path, cfg)
    cfg_path = tmp_path / "cfg.yaml"
    import yaml
    cfg_path.write_text(yaml.safe_dump(cfg))
    ap = build_parser()
    args = ap.parse_args(["--config", str(cfg_path), "--runs-dir",
                          str(tmp_path / "runs"), "--experiment-id",
                          exp_dir.name, "replay", ev["evaluation_id"]])
    assert args.fn(args) == 0
    out = json.loads((exp_dir / "replays" / f"{ev['evaluation_id']}.json")
                     .read_text())
    assert out["diff"]["identical_spike_counts"] is True
    args = ap.parse_args(["--config", str(cfg_path), "--runs-dir",
                          str(tmp_path / "runs"), "--experiment-id",
                          exp_dir.name, "replay", ev["evaluation_id"],
                          "--backend", "torch", "--strict"])
    assert args.fn(args) == 5


def test_repeated_replays_are_archived_not_overwritten(tmp_path, smoke_config):
    """Replaying one evaluation several times (strict / device-drift /
    different execution batch) must keep every report: the per-run
    device_warnings of an earlier replay are not overwritten by a later
    one. <evaluation_id>.json remains the latest pointer."""
    from experiments.mioba.cli import build_parser
    import yaml
    cfg = copy.deepcopy(smoke_config)
    exp_dir, ev = _one_eval(tmp_path, cfg)
    cfg_path = tmp_path / "cfg.yaml"
    cfg_path.write_text(yaml.safe_dump(cfg))
    ap = build_parser()
    base = ["--config", str(cfg_path), "--runs-dir", str(tmp_path / "runs"),
            "--experiment-id", exp_dir.name, "replay", ev["evaluation_id"]]

    assert ap.parse_args(base).fn(ap.parse_args(base)) == 0
    args = ap.parse_args(base + ["--execution-batch", "1"])
    assert args.fn(args) == 0

    replays = exp_dir / "replays"
    archived = sorted(replays.glob(f"{ev['evaluation_id']}.*.json"))
    assert len(archived) == 2, [p.name for p in archived]
    latest = json.loads((replays / f"{ev['evaluation_id']}.json").read_text())
    assert latest["conditions"]["execution_batch"] == 1
    assert json.loads(archived[-1].read_text())["conditions"]["execution_batch"] == 1


def test_replay_out_option(tmp_path, smoke_config):
    from experiments.mioba.cli import build_parser
    import yaml
    cfg = copy.deepcopy(smoke_config)
    exp_dir, ev = _one_eval(tmp_path, cfg)
    cfg_path = tmp_path / "cfg.yaml"
    cfg_path.write_text(yaml.safe_dump(cfg))
    out = tmp_path / "reports" / "parity.json"
    ap = build_parser()
    args = ap.parse_args(["--config", str(cfg_path), "--runs-dir",
                          str(tmp_path / "runs"), "--experiment-id",
                          exp_dir.name, "replay", ev["evaluation_id"],
                          "--out", str(out)])
    assert args.fn(args) == 0
    assert json.loads(out.read_text())["diff"]["identical_spike_counts"] is True
    assert list((exp_dir / "replays").glob(f"{ev['evaluation_id']}.*.json"))
