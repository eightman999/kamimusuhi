"""End-to-end CLI tests: inspect + verify as real subprocesses,
including state persistence and reproducibility metadata."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

from experiments.fi3.cli.verify import (
    EXIT_MIGRATION_REQUIRED,
    EXIT_OK,
    EXIT_REVIEW_REQUIRED,
)
from experiments.fi3.tests.conftest import MODEL_V2, make_runtime_dir

REPO_ROOT = Path(__file__).resolve().parents[3]


def _run(module, *args):
    return subprocess.run(
        [sys.executable, "-m", module, *args],
        cwd=str(REPO_ROOT),
        capture_output=True,
        text=True,
        timeout=60,
    )


def test_inspect_prints_identity(tmp_path):
    rd = make_runtime_dir(tmp_path / "rt")
    r = _run(
        "experiments.fi3.cli.inspect", "--runtime-dir", str(rd)
    )
    assert r.returncode == 0, r.stderr
    assert "runtime_identity_hash" in r.stdout
    assert "components:" in r.stdout


def test_inspect_json_roundtrip(tmp_path):
    rd = make_runtime_dir(tmp_path / "rt")
    r = _run(
        "experiments.fi3.cli.inspect", "--runtime-dir", str(rd), "--json"
    )
    assert r.returncode == 0, r.stderr
    rec = json.loads(r.stdout)
    assert len(rec["runtime_identity_hash"]) == 64
    assert rec["missing_required_fields"] == []
    # reproducibility record present
    assert rec["recorded"]["git_commit"]
    assert rec["recorded"]["git_branch"]
    assert rec["recorded"]["python_version"]
    assert rec["recorded"]["timestamp_utc"]


def test_inspect_missing_runtime_dir(tmp_path):
    r = _run(
        "experiments.fi3.cli.inspect",
        "--runtime-dir",
        str(tmp_path / "does-not-exist"),
    )
    # collection succeeds (all-None manifest) but reports missing fields
    assert r.returncode == 0
    assert "missing_required" in r.stdout


def test_verify_full_lifecycle(tmp_path):
    rd = make_runtime_dir(tmp_path / "rt")
    sd = tmp_path / "state"
    v = "experiments.fi3.cli.verify"

    # 1. baseline init
    r = _run(v, "--runtime-dir", str(rd), "--state-dir", str(sd))
    assert r.returncode == EXIT_OK, r.stderr
    assert "baseline_initialized" in r.stdout

    # 2. identical -> continue
    r = _run(v, "--runtime-dir", str(rd), "--state-dir", str(sd))
    assert r.returncode == EXIT_OK
    assert "none" in r.stdout

    # 3. BREAKING -> exit 3, MIGRATION_REQUIRED persisted
    (rd / "model/model.bin").write_bytes(MODEL_V2)
    r = _run(v, "--runtime-dir", str(rd), "--state-dir", str(sd))
    assert r.returncode == EXIT_MIGRATION_REQUIRED
    assert "MIGRATION_REQUIRED" in r.stdout
    state = json.loads((sd / "state.json").read_text())
    assert state["status"] == "MIGRATION_REQUIRED"
    mig = json.loads((sd / "migration_required.json").read_text())
    assert mig["old_manifest"] != mig["new_manifest"]
    assert mig["classification"]["overall"] == "BREAKING"
    assert mig["diff"]

    # 4. still broken -> still blocked
    r = _run(v, "--runtime-dir", str(rd), "--state-dir", str(sd))
    assert r.returncode == EXIT_MIGRATION_REQUIRED


def test_verify_review_requires_flag(tmp_path):
    rd = make_runtime_dir(tmp_path / "rt")
    sd = tmp_path / "state"
    v = "experiments.fi3.cli.verify"

    assert _run(v, "--runtime-dir", str(rd), "--state-dir", str(sd)).returncode == 0

    cfg = json.loads((rd / "config/runtime.json").read_text())
    cfg["sampling"]["temperature"] = 1.5
    (rd / "config/runtime.json").write_text(json.dumps(cfg, indent=2))

    r = _run(v, "--runtime-dir", str(rd), "--state-dir", str(sd))
    assert r.returncode == EXIT_REVIEW_REQUIRED
    assert "REVIEW" in r.stdout

    r = _run(
        v, "--runtime-dir", str(rd), "--state-dir", str(sd), "--accept-review"
    )
    assert r.returncode == EXIT_OK

    # accepted -> subsequent verify is clean
    r = _run(v, "--runtime-dir", str(rd), "--state-dir", str(sd))
    assert r.returncode == EXIT_OK


def test_verify_json_output(tmp_path):
    rd = make_runtime_dir(tmp_path / "rt")
    sd = tmp_path / "state"
    v = "experiments.fi3.cli.verify"
    _run(v, "--runtime-dir", str(rd), "--state-dir", str(sd))
    r = _run(
        v, "--runtime-dir", str(rd), "--state-dir", str(sd), "--json"
    )
    rec = json.loads(r.stdout)
    assert rec["exit_code"] == 0
    assert rec["overall"] == "IDENTICAL"
    assert rec["recorded"]["git_commit"]
    assert rec["recorded"]["git_branch"]  # branch or 'HEAD' if detached


def test_verify_incomplete_runtime_refused_as_baseline(tmp_path):
    rd = tmp_path / "empty-runtime"
    rd.mkdir()
    sd = tmp_path / "state"
    r = _run(
        "experiments.fi3.cli.verify",
        "--runtime-dir",
        str(rd),
        "--state-dir",
        str(sd),
    )
    assert r.returncode == 4
    assert "missing required fields" in r.stdout


def test_history_records_repro_metadata(tmp_path):
    rd = make_runtime_dir(tmp_path / "rt")
    sd = tmp_path / "state"
    v = "experiments.fi3.cli.verify"
    _run(v, "--runtime-dir", str(rd), "--state-dir", str(sd), "--seed", "7")
    _run(v, "--runtime-dir", str(rd), "--state-dir", str(sd))
    state = json.loads((sd / "state.json").read_text())
    assert len(state["history"]) == 2
    first = state["history"][0]
    assert first["event"] == "baseline_initialized"
    rec = first["recorded"]
    assert rec["seed"] == 7
    assert rec["git_commit"] and rec["git_branch"]
    assert rec["python_version"].startswith("3.12")
    assert rec["timestamp_utc"].endswith("+00:00")
