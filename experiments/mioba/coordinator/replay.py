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

Device identity: the recorded runtime_info (gpu_model, compute_capability,
gpu_uuid, gpu_index, torch_version, cuda_runtime) is compared with the
replay device. ``--strict`` fails on GPU model / compute-capability drift
(an RTX 3060 cc 8.6 recording replayed on a P100 cc 6.0 is *not* a
faithful replay); ``allow_device_drift=True`` turns that into an explicit
cross-GPU parity check whose device warnings are recorded, not fatal.

Replicates: the recorded ``replicate_seeds`` / ``requested_replicates``
are replayed in chunks of the replay ``execution_batch``; results are
per replicate, so the chunk width is free to differ from the recording.
"""
from __future__ import annotations

import json
import re
from dataclasses import dataclass, field
from pathlib import Path

from ..development.phenotype import develop
from ..fba.registry import get_backend
from ..fba.replicates import replicate_seeds
from ..fba.semantics import check_replayable
from ..fba.semantics import semantics as current_semantics
from ..genome.hashing import scientific_config_hash
from ..genome.schema import Genome
from ..storage.db import Database
from ..workers.gpu_info import gpu_identity

# v0 = base graph sampled from the genome seed (M0, and the mock backend
# which has no separate base seed); v1 = sampled from fba.base_seed, which
# the version string carries as a "-s<seed>" suffix.
_SYNTH_VERSION = re.compile(r"^v[01]-n(?P<n>\d+)-p(?P<p>[0-9.eE+-]+)$")
_SYNTH_EDGES_VERSION = re.compile(r"^v[01]-n(?P<n>\d+)-e(?P<e>\d+)$")
_BASE_SEED_SUFFIX = re.compile(r"^(?P<head>v1-.*)-s(?P<s>\d+)$")

DEVICE_IDENTITY_KEYS = ("gpu_model", "compute_capability", "gpu_uuid",
                        "gpu_index", "device", "torch_version",
                        "cuda_runtime")


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
    replicates: int = 1
    replicate_seeds: list[int] = field(default_factory=list)
    execution_batch: int = 1
    device_warnings: list[str] = field(default_factory=list)
    recorded_device: dict = field(default_factory=dict)
    current_device: dict = field(default_factory=dict)
    # simulator identity of the recording (fba/semantics.py)
    semantics: dict = field(default_factory=dict)


def _parse_synthetic_version(version: str | None) -> dict:
    text = version or ""
    base_seed = None
    m = _BASE_SEED_SUFFIX.match(text)
    if m:
        base_seed, text = int(m.group("s")), m.group("head")
    m = _SYNTH_VERSION.match(text)
    if m:
        return {"n": int(m.group("n")), "p": float(m.group("p")),
                "base_seed": base_seed}
    m = _SYNTH_EDGES_VERSION.match(text)
    if m:
        return {"n": int(m.group("n")), "e": int(m.group("e")),
                "base_seed": base_seed}
    raise ReplayUnavailable(
        f"cannot reconstruct synthetic network from dataset version "
        f"{version!r}")


def compare_device_identity(recorded: dict, current: dict) -> tuple[list[str],
                                                                     list[str]]:
    """(fatal_in_strict, informational). GPU model / compute capability
    drift is fatal in strict mode; UUID / index / torch / CUDA version
    drift is informational (same GPU class, may still differ bitwise)."""
    fatal, info = [], []
    rec_model, cur_model = recorded.get("gpu_model"), current.get("gpu_model")
    rec_cc, cur_cc = (recorded.get("compute_capability"),
                      current.get("compute_capability"))
    if rec_model or cur_model:
        if rec_model != cur_model:
            fatal.append(f"GPU model differs: recorded {rec_model!r}, "
                         f"replaying on {cur_model!r}")
        if rec_cc != cur_cc:
            fatal.append(f"compute capability differs: recorded {rec_cc!r}, "
                         f"replaying on {cur_cc!r}")
    rec_dev = recorded.get("device")
    cur_dev = current.get("device")
    if rec_dev and cur_dev and rec_dev.split(":")[0] != cur_dev.split(":")[0]:
        fatal.append(f"device type differs: recorded {rec_dev}, "
                     f"replaying on {cur_dev}; bit-identity not expected")
    for k, label in (("gpu_uuid", "GPU UUID"), ("gpu_index", "GPU index"),
                     ("torch_version", "torch"), ("cuda_runtime", "CUDA")):
        a, b = recorded.get(k), current.get(k)
        if a is not None and b is not None and str(a) != str(b):
            info.append(f"{label} differs: recorded {a}, now {b}")
    return fatal, info


def _dataset_kwargs(dataset: dict, backend: str, data_dir: str | None,
                    run_dir: Path) -> dict:
    ds_id = dataset.get("dataset_id")
    if not ds_id:
        raise ReplayUnavailable(
            "evaluation has no recorded dataset identity; it predates "
            "schema v2 and cannot be replayed faithfully")
    if ds_id == "mock-fba":
        v = _parse_synthetic_version(dataset.get("version"))
        return {"n_neurons": v["n"], "connectivity": v.get("p", 0.01)}
    if ds_id == "synthetic-fba":
        v = _parse_synthetic_version(dataset.get("version"))
        kw = {"data_dir": None, "synthetic": True, "synthetic_neurons": v["n"],
              "runs_dir": str(run_dir),
              "region_mode": dataset.get("region_mode")}
        if "e" in v:
            kw["synthetic_edges"] = v["e"]
        else:
            kw["connectivity"] = v["p"]
        if v.get("base_seed") is not None:
            kw["base_seed"] = v["base_seed"]
        return kw
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
               strict: bool = False, allow_device_drift: bool = False,
               execution_batch: int | None = None,
               current_device_identity: dict | None = None) -> ReplayPlan:
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
    if strict and warnings:
        raise ReplayConfigMismatch("; ".join(warnings))

    # -- device identity (R8) --
    try:
        rec_ri = json.loads(ev.get("runtime_info_json") or "{}")
    except ValueError:
        rec_ri = {}
    recorded_device = {k: rec_ri.get(k) for k in DEVICE_IDENTITY_KEYS}
    recorded_device["device"] = ev.get("device") or rec_ri.get("device")
    current_device = dict(current_device_identity
                          if current_device_identity is not None
                          else gpu_identity(device))
    current_device.setdefault("device", device)
    dev_fatal, dev_info = compare_device_identity(recorded_device,
                                                  current_device)
    device_warnings = dev_fatal + dev_info
    if dev_fatal and strict and not allow_device_drift:
        raise ReplayConfigMismatch(
            "; ".join(dev_fatal) + " (pass --allow-device-drift for an "
            "explicit cross-GPU parity check)")
    if dev_fatal and allow_device_drift:
        device_warnings.insert(0, "device drift allowed explicitly: this is "
                               "a cross-GPU parity check, not a faithful "
                               "replay")

    # -- replicates (R1) --
    seed = int(ev["seed"] if ev.get("seed") is not None else job["seed"])
    try:
        rec_seeds = [int(s) for s in
                     json.loads(ev.get("replicate_seeds_json") or "[]")]
    except (ValueError, TypeError):
        rec_seeds = []
    n_rep = int(ev.get("requested_replicates") or job.get("replicates")
                or len(rec_seeds) or ev.get("batch_size") or 1)
    if not rec_seeds:
        rec_seeds = replicate_seeds(seed, n_rep)
    if len(rec_seeds) != n_rep:
        raise ReplayUnavailable(
            f"recorded {len(rec_seeds)} replicate seeds for "
            f"{n_rep} replicates")
    exec_batch = int(execution_batch or ev.get("execution_batch_size")
                     or ev.get("batch_size") or 1)

    # simulator semantics of the recording; compared against the backend
    # that will actually run the replay, in run_plan (M1 2.4-2)
    recorded_semantics = {
        k: ev.get(k) for k in ("simulator_semantics_version",
                               "rng_protocol_version", "propagation_backend")
        if ev.get(k) is not None}

    kwargs = _dataset_kwargs(dataset, use_backend,
                             data_dir or (config.get("fba") or {}).get(
                                 "data_dir"), exp_dir)
    return ReplayPlan(
        evaluation_id=evaluation_id, genome=genome, backend=use_backend,
        seed=seed,
        batch_size=exec_batch,
        environment_id=ev.get("environment_id") or job["environment_id"],
        duration_ms=float(ev.get("duration_ms") or job["duration_ms"]),
        device=device, dataset=dataset, backend_kwargs=kwargs, config=config,
        original_summary=json.loads(ev["summary_json"] or "{}"),
        warnings=warnings + device_warnings,
        identity={"genome_hash": genome.genome_id,
                  "scientific_config_hash": rec_sci,
                  "config_hash": ev.get("config_hash"),
                  "git_commit": ev.get("git_commit"),
                  "recorded_device": recorded_device,
                  "current_device": current_device,
                  "recorded_backend": rec_backend,
                  "allow_device_drift": allow_device_drift},
        replicates=n_rep, replicate_seeds=rec_seeds,
        execution_batch=exec_batch, device_warnings=device_warnings,
        recorded_device=recorded_device, current_device=current_device,
        semantics=recorded_semantics)


def run_plan(plan: ReplayPlan) -> dict:
    from ..workers.worker import evaluate_replicates
    backend = get_backend(plan.backend, **plan.backend_kwargs)
    phen = develop(plan.genome)
    # dataset identity is checked before any simulation
    backend.initialize(phen, batch_size=1, seed=plan.seed, device=plan.device,
                       replicate_seeds=plan.replicate_seeds[:1])
    # a recording made by different equations, a different RNG protocol
    # or a different propagation path is not reproducible by this build,
    # whatever the config says (M1 2.4-2)
    problems = check_replayable(plan.semantics, backend.semantics())
    if problems:
        raise ReplayUnavailable(
            "simulator semantics mismatch: " + "; ".join(problems)
            + " - replay this evaluation with the commit that produced it")
    ident = backend.dataset_identity()
    for k in ("dataset_id", "version", "manifest_hash"):
        if plan.dataset.get(k) is not None and ident.get(k) != plan.dataset[k]:
            raise ReplayUnavailable(
                f"dataset {k} mismatch: recorded {plan.dataset[k]!r}, "
                f"available {ident.get(k)!r}")
    job = {"seed": plan.seed, "replicates": plan.replicates,
           "replicate_seeds": plan.replicate_seeds,
           "duration_ms": plan.duration_ms,
           "environment_id": plan.environment_id, "config": plan.config}
    rep = evaluate_replicates(backend, phen, job, plan.device,
                              plan.execution_batch)
    summary = rep["summary"]
    original = plan.original_summary
    orig_counts = list(original.get("per_replicate_spike_counts")
                       or original.get("per_batch_spike_counts") or [])
    diff = {
        "original_mean_rate_hz": original.get("mean_rate_hz"),
        "replay_mean_rate_hz": summary.get("mean_rate_hz"),
        "original_spikes": sum(orig_counts),
        "replay_spikes": summary["spikes_total"],
        "identical_spike_counts": (
            orig_counts == list(summary.get("per_replicate_spike_counts")
                                or [])),
        "identical_replicate_seeds": (
            [int(s) for s in original.get("replicate_seeds") or
             plan.replicate_seeds] == summary.get("replicate_seeds")),
    }
    return {"identity": plan.identity, "dataset": ident,
            "conditions": {"backend": plan.backend, "seed": plan.seed,
                           "replicates": plan.replicates,
                           "execution_batch": plan.execution_batch,
                           "batch_size": plan.execution_batch,
                           "environment_id": plan.environment_id,
                           "duration_ms": plan.duration_ms,
                           "device": plan.device},
            "warnings": plan.warnings,
            "device_warnings": plan.device_warnings,
            "original": original, "replay": summary, "diff": diff}
