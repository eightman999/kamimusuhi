"""Re-evaluate a recorded evaluation under its *original* conditions.

Everything is reconstructed from the lineage DB (evaluation row, job row,
genome row, experiment config snapshot) — never from the current config
file:

    genome (json), genome_hash, seed, environment_id, duration_ms,
    backend, batch_size, scientific_config_hash, git_commit,
    dataset identity {dataset_id, version, manifest_hash, region_mode}.

The backend is instantiated from the dataset identity, so a real-FBA
evaluation is replayed on the real dataset or not at all: a missing or
mismatching dataset raises ``ReplayUnavailable`` instead of silently
falling back to synthetic. Scientific-config or git drift between the
recording and now is reported (``warnings``) or, in strict mode, raised
as ``ReplayConfigMismatch``.
"""
from __future__ import annotations

import json
import re
from dataclasses import dataclass, field
from pathlib import Path

from ..development.phenotype import develop
from ..fba.registry import get_backend
from ..genome.hashing import scientific_config_hash
from ..genome.schema import Genome
from ..mie.environments import make_drive
from ..storage.db import Database

_SYNTH_VERSION = re.compile(r"^v0-n(?P<n>\d+)-p(?P<p>[0-9.eE+-]+)$")


class ReplayUnavailable(RuntimeError):
    """Required dataset/backend for a faithful replay is not available."""


class ReplayConfigMismatch(RuntimeError):
    """Strict mode: recorded conditions differ from what can be run now."""


@dataclass
class ReplayPlan:
    evaluation_id: str
    genome: Genome
    backend: str
    seed: int
    batch_size: int
    environment_id: str
    duration_ms: float
    device: str
    dataset: dict
    backend_kwargs: dict
    config: dict                      # experiment config snapshot
    original_summary: dict
    warnings: list[str] = field(default_factory=list)
    identity: dict = field(default_factory=dict)


def _parse_synthetic_version(version: str | None) -> tuple[int, float]:
    m = _SYNTH_VERSION.match(version or "")
    if not m:
        raise ReplayUnavailable(
            f"cannot reconstruct synthetic network from dataset version "
            f"{version!r}")
    return int(m.group("n")), float(m.group("p"))


def _dataset_kwargs(dataset: dict, backend: str, data_dir: str | None,
                    run_dir: Path) -> dict:
    ds_id = dataset.get("dataset_id")
    if not ds_id:
        raise ReplayUnavailable(
            "evaluation has no recorded dataset identity; it predates "
            "schema v2 and cannot be replayed faithfully")
    if ds_id == "mock-fba":
        n, p = _parse_synthetic_version(dataset.get("version"))
        return {"n_neurons": n, "connectivity": p}
    if ds_id == "synthetic-fba":
        n, p = _parse_synthetic_version(dataset.get("version"))
        return {"data_dir": None, "synthetic": True, "synthetic_neurons": n,
                "connectivity": p, "runs_dir": str(run_dir),
                "region_mode": dataset.get("region_mode")}
    if ds_id.startswith("flywire"):
        if not data_dir or not Path(data_dir).is_dir():
            raise ReplayUnavailable(
                f"required dataset {ds_id}@{dataset.get('version')} "
                f"(manifest {dataset.get('manifest_hash')}) is not available"
                f" (data_dir={data_dir!r}); pass --data-dir")
        return {"data_dir": data_dir, "synthetic": False,
                "runs_dir": str(run_dir),
                "region_mode": dataset.get("region_mode")}
    raise ReplayUnavailable(f"unknown dataset id {ds_id!r}")


def build_plan(exp_dir: Path, evaluation_id: str, *, device: str = "cpu",
               data_dir: str | None = None, backend: str | None = None,
               current_config: dict | None = None,
               current_git_commit: str | None = None,
               strict: bool = False) -> ReplayPlan:
    db = Database(exp_dir / "lineage.sqlite")
    try:
        ev = db.get_evaluation(evaluation_id)
        if ev is None:
            raise KeyError(f"no evaluation {evaluation_id}")
        job = db.get_job(ev["job_id"])
        grow = db.get_genome(ev["genome_id"])
        exp = db.get_experiment(ev["experiment_id"]) or {}
    finally:
        db.close()
    if job is None or grow is None:
        raise ReplayUnavailable(
            f"evaluation {evaluation_id} lacks its job/genome rows")

    config = json.loads(exp.get("config_json") or "{}")
    dataset = json.loads(ev.get("dataset_json") or "{}")
    genome = Genome.from_json(grow["genome_json"])
    warnings: list[str] = []

    if ev.get("genome_hash") and ev["genome_hash"] != genome.genome_id:
        raise ReplayUnavailable(
            f"stored genome_hash {ev['genome_hash']} != genome row id "
            f"{genome.genome_id}")

    rec_sci = ev.get("scientific_config_hash") or exp.get(
        "scientific_config_hash")
    if rec_sci and current_config is not None:
        now_sci = scientific_config_hash(current_config)
        if now_sci != rec_sci:
            warnings.append(
                f"scientific_config_hash differs: recorded {rec_sci[:12]} "
                f"vs current {now_sci[:12]} (replay uses the recorded "
                "experiment config snapshot)")
    if ev.get("git_commit") and current_git_commit and \
            ev["git_commit"] != current_git_commit:
        warnings.append(f"git commit differs: recorded {ev['git_commit'][:12]}"
                        f" vs current {current_git_commit[:12]}")

    rec_backend = ev.get("backend") or job["backend"]
    use_backend = backend or rec_backend
    if use_backend != rec_backend:
        warnings.append(f"backend override: recorded {rec_backend}, "
                        f"replaying on {use_backend} (cross-backend parity "
                        "check, not a faithful replay)")
    rec_device = ev.get("device")
    if rec_device and rec_device.split(":")[0] != device.split(":")[0]:
        warnings.append(f"device type differs: recorded {rec_device}, "
                        f"replaying on {device}; bit-identity not expected")

    if strict and warnings:
        raise ReplayConfigMismatch("; ".join(warnings))

    kwargs = _dataset_kwargs(dataset, use_backend,
                             data_dir or (config.get("fba") or {}).get(
                                 "data_dir"), exp_dir)
    return ReplayPlan(
        evaluation_id=evaluation_id, genome=genome, backend=use_backend,
        seed=int(ev["seed"] if ev.get("seed") is not None else job["seed"]),
        batch_size=int(ev.get("batch_size") or 1),
        environment_id=ev.get("environment_id") or job["environment_id"],
        duration_ms=float(ev.get("duration_ms") or job["duration_ms"]),
        device=device, dataset=dataset, backend_kwargs=kwargs, config=config,
        original_summary=json.loads(ev["summary_json"] or "{}"),
        warnings=warnings,
        identity={"genome_hash": genome.genome_id,
                  "scientific_config_hash": rec_sci,
                  "config_hash": ev.get("config_hash"),
                  "git_commit": ev.get("git_commit"),
                  "recorded_device": rec_device,
                  "recorded_backend": rec_backend})


def run_plan(plan: ReplayPlan) -> dict:
    backend = get_backend(plan.backend, **plan.backend_kwargs)
    phen = develop(plan.genome)
    backend.initialize(phen, batch_size=plan.batch_size, seed=plan.seed,
                       device=plan.device)
    ident = backend.dataset_identity()
    for k in ("dataset_id", "version", "manifest_hash"):
        if plan.dataset.get(k) is not None and ident.get(k) != plan.dataset[k]:
            raise ReplayUnavailable(
                f"dataset {k} mismatch: recorded {plan.dataset[k]!r}, "
                f"available {ident.get(k)!r}")
    n_base = backend.n_base if hasattr(backend, "n_base") else 512
    backend.set_inputs(make_drive(plan.environment_id, n_base, plan.config))
    stats = backend.run(plan.duration_ms)
    summary = backend.get_state_summary()
    original = plan.original_summary
    orig_spikes = sum(original.get("per_batch_spike_counts") or [])
    diff = {
        "original_mean_rate_hz": original.get("mean_rate_hz"),
        "replay_mean_rate_hz": summary.get("mean_rate_hz"),
        "original_spikes": orig_spikes,
        "replay_spikes": stats["spikes_total"],
        "identical_spike_counts": (
            list(original.get("per_batch_spike_counts") or [])
            == list(summary.get("per_batch_spike_counts") or [])),
    }
    return {"identity": plan.identity, "dataset": ident,
            "conditions": {"backend": plan.backend, "seed": plan.seed,
                           "batch_size": plan.batch_size,
                           "environment_id": plan.environment_id,
                           "duration_ms": plan.duration_ms,
                           "device": plan.device},
            "warnings": plan.warnings, "original": original,
            "replay": summary, "diff": diff}
