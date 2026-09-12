"""End-to-end runner test: raw JSONL + manifest + analysis artifacts."""

from __future__ import annotations

import json
from pathlib import Path

from experiments.fi0.runners.clients import MockModel, SensitiveMockModel
from experiments.fi0.runners.runner import analyze, run_evaluation
from experiments.fi0.sweep import SweepConfig, VariantAxes, load_config

CFG_DIR = Path(__file__).resolve().parents[1] / "configs"


def _small_cfg() -> SweepConfig:
    return SweepConfig(
        name="test",
        seed=0,
        reps=2,
        tasks=("binary_decision", "tool_selection"),
        variants=VariantAxes(
            orders=("sys_mem_user", "user_mem_sys"),
            formats=("plain", "json"),
            whitespace=("tight",),
            metadata_positions=("prefix", "suffix"),
        ),
    )


def test_run_evaluation_writes_artifacts(tmp_path):
    cfg = _small_cfg()
    out = run_evaluation(cfg, MockModel(), tmp_path, run_id="t1")
    run_dir = Path(out["run_dir"])
    assert (run_dir / "raw.jsonl").exists()
    assert (run_dir / "manifest.json").exists()
    assert (run_dir / "analysis.json").exists()

    rows = [
        json.loads(l) for l in (run_dir / "raw.jsonl").read_text().splitlines()
    ]
    # 2 tasks x (2 orders*2 fmts*1 ws*2 meta + canonical) x 2 reps
    assert len(rows) == 2 * 9 * 2
    assert all("prompt" in r and "response" in r for r in rows)

    manifest = json.loads((run_dir / "manifest.json").read_text())
    for key in (
        "git_commit", "git_branch", "python_version", "config_sha256",
        "seed", "run_at_utc", "model", "n_rows",
    ):
        assert key in manifest
    assert manifest["seed"] == 0
    assert manifest["model"] == "mock"

    analysis = json.loads((run_dir / "analysis.json").read_text())
    ov = analysis["overall"]
    assert ov["mean_pairwise_agreement"] == 1.0
    assert ov["canonical_integrity_failures"] == 0
    assert ov["canonical_collapses_variants"] is True


def test_load_config_and_overrides():
    cfg = load_config(CFG_DIR / "smoke.yaml")
    assert cfg.name == "smoke"
    assert cfg.model == "mock"
    specs = cfg.variant_specs()
    # 2 orders x 2 fmts x 1 ws x 2 meta + canonical
    assert len(specs) == 2 * 2 * 1 * 2 + 1


def test_sensitive_control_verdict(tmp_path):
    cfg = _small_cfg()
    out = run_evaluation(
        cfg, SensitiveMockModel(), tmp_path, run_id="t-sensitive"
    )
    assert "DRIFT DETECTED" in out["analysis"]["verdict"]


def test_analyze_multi_task_structure():
    cfg = _small_cfg()
    import dataclasses

    cfg = dataclasses.replace(cfg, tasks=("binary_decision",))
    rows_analysis = None
    from experiments.fi0.sweep import run_sweep

    rows_analysis = analyze(list(run_sweep(cfg, MockModel())))
    assert "binary_decision" in rows_analysis["per_task"]
    pt = rows_analysis["per_task"]["binary_decision"]
    assert pt["cacheability"]["overall_mean_lcp_ratio"] >= 0.0
    assert pt["latency_ms"]["mean"] > 0
    assert pt["prompt_tokens"]["min"] > 0
