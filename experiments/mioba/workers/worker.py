"""MIOBA worker process.

    python -m experiments.mioba.workers.worker \
        --coordinator http://host:8765 --device cuda:0|cpu \
        --backend torch|mock|genn --execution-batch auto|N [--bench] \
        [--worker-id ...] [--heartbeat-s 5]

Loop: GET /api/worker/profile -> (bench on that profile if
--execution-batch auto or --bench) -> register -> claim -> develop ->
evaluate replicates in chunks of execution_batch -> deliver result until
acknowledged -> claim next. A heartbeat thread runs the whole time and
reports the job this worker currently holds (idle -> null, running /
delivering -> job id, acknowledged -> null) and only *this* worker's GPU.
SIGTERM: finish the current job if it has been running for < grace_s,
else report FAILED("interrupted") and exit.
The worker never marks anything succeeded on its own.

Scientific vs operational: ``job["replicates"]`` (evaluation.replicates)
is how many independent lanes are averaged for fitness; the worker's
``execution_batch`` only decides how many of them run at once. Lane i is
always seeded with ``replicate_seed(job seed, i)`` — see
fba/replicates.py — so a P100 running 4x8 and a 3060 running 16+16 produce
the same replicate results and the same fitness.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import signal
import socket
import sys
import threading
import time
import uuid
from pathlib import Path

from ..development.phenotype import develop
from ..fba.registry import get_backend
from ..fba.replicates import chunk_indices, replicate_seeds
from ..genome.schema import Genome, utcnow
from ..mie.environments import make_drive
from ..storage import models as M
from . import gpu_info
from .bench import (DEFAULT_CANDIDATES, DEFAULT_VRAM_HEADROOM, bench_phenotype,
                    choose_batch, profile_backend_kwargs, startup_benchmark)

_stop = threading.Event()
_current_job_started = None


class WorkerState:
    """Shared between the job loop and the heartbeat thread."""

    def __init__(self):
        self._lock = threading.Lock()
        self._job_id: str | None = None
        self.phase = "idle"

    def set_job(self, job_id: str | None, phase: str = "running") -> None:
        with self._lock:
            self._job_id = job_id
            self.phase = phase if job_id else "idle"

    @property
    def current_job_id(self) -> str | None:
        with self._lock:
            return self._job_id


STATE = WorkerState()


def _sigterm(signum, frame):
    _stop.set()


def default_worker_id(device: str = "cpu") -> str:
    dev = device.replace(":", "")
    return f"{socket.gethostname()}-{dev}-{uuid.uuid4().hex[:8]}"


def backend_kwargs(config: dict, run_dir: str | None,
                   data_dir: str | None = None) -> dict:
    fba = (config or {}).get("fba", {})
    kw = {"data_dir": (data_dir if data_dir is not None
                       else fba.get("data_dir")),
          "synthetic": bool(fba.get("synthetic", True)),
          "synthetic_neurons": int(fba.get("synthetic_neurons", 2000)),
          "runs_dir": run_dir}
    if fba.get("synthetic_edges"):
        kw["synthetic_edges"] = int(fba["synthetic_edges"])
    if fba.get("connectivity") is not None:
        kw["connectivity"] = float(fba["connectivity"])
    return kw


def result_id_for(job_id: str, worker_id: str, attempt: int) -> str:
    """Deterministic per (job, worker, attempt): the same worker
    re-delivering the same result reuses the id; a reclaim (new attempt
    or other worker) gets a different one."""
    h = hashlib.blake2b(f"{job_id}:{worker_id}:{int(attempt)}".encode(),
                        digest_size=16).hexdigest()
    return f"res_{h}"


def _stack(per_lane: dict[str, list], key: str, values) -> None:
    per_lane.setdefault(key, []).extend(values)


def evaluate_replicates(backend, phenotype: dict, job: dict, device: str,
                        execution_batch: int, grace_s: float = 30.0,
                        deadline_started: float | None = None) -> dict:
    """Run every scientific replicate of ``job`` in chunks of
    ``execution_batch`` lanes and aggregate per-replicate results.

    Returned ``summary`` is ordered by replicate index; ``mean_rate_hz``
    is the mean over replicates. Independent of ``execution_batch``.
    """
    n_rep = int(job.get("replicates") or 1)
    seeds = list(job.get("replicate_seeds") or replicate_seeds(job["seed"],
                                                                n_rep))
    if len(seeds) != n_rep:
        raise ValueError(f"replicate_seeds has {len(seeds)} entries for "
                         f"{n_rep} replicates")
    config = job.get("config") or {}
    duration = float(job["duration_ms"])
    per_rep: dict[str, list] = {}
    activity: dict[str, list] = {}
    chunks = chunk_indices(n_rep, execution_batch)
    exec_batch_used = max(len(c) for c in chunks)
    total_wall = 0.0
    t_ms = 0.0
    completed = 0
    for lanes in chunks:
        backend.initialize(phenotype, batch_size=len(lanes), seed=job["seed"],
                           device=device,
                           replicate_seeds=[seeds[i] for i in lanes])
        n_drive = backend.n_base if hasattr(backend, "n_base") else 512
        backend.set_inputs(make_drive(job["environment_id"], n_drive, config))
        remaining = duration
        while remaining > 1e-9:
            if _stop.is_set() and deadline_started is not None and \
                    time.time() - deadline_started > grace_s:
                raise RuntimeError("interrupted")
            chunk = min(50.0, remaining)
            stats = backend.run(chunk)
            total_wall += float(stats.get("wall_s", 0.0))
            remaining -= chunk
        s = backend.get_state_summary()
        t_ms = s.get("t_ms", t_ms)
        _stack(per_rep, "mean_rate_hz", s["per_batch_mean_rate_hz"])
        _stack(per_rep, "spike_counts", s["per_batch_spike_counts"])
        _stack(per_rep, "seeds", list(s.get("replicate_seeds") or
                                      [seeds[i] for i in lanes]))
        _stack(per_rep, "index", lanes)
        for g, vals in backend.get_population_activity(["all", "fba0"]).items():
            _stack(activity, g, vals)
        completed += len(lanes)
    rates = per_rep["mean_rate_hz"]
    counts = per_rep["spike_counts"]
    mean = sum(rates) / len(rates)
    var = sum((r - mean) ** 2 for r in rates) / len(rates)
    summary = {
        "t_ms": t_ms,
        "mean_rate_hz": float(mean),
        "rate_std_hz": float(var ** 0.5),
        "active_fraction": float(sum(1 for c in counts if c > 0) / len(counts)),
        "spikes_total": int(sum(counts)),
        "per_replicate_mean_rate_hz": [float(r) for r in rates],
        "per_replicate_spike_counts": [int(c) for c in counts],
        "replicate_seeds": [int(x) for x in per_rep["seeds"]],
        "replicate_index": list(per_rep["index"]),
        "activity": {g: [float(v) for v in vals] for g, vals in activity.items()},
        "wall_s": total_wall,
        "sim_seconds_per_wall_second": ((duration / 1000.0) * n_rep / total_wall
                                        if total_wall > 0 else None),
        "vram": (backend.get_state_summary().get("vram")
                 if hasattr(backend, "get_state_summary") else None),
    }
    return {"summary": summary, "requested_replicates": n_rep,
            "completed_replicates": completed,
            "execution_batch_size": exec_batch_used,
            "replicate_seeds": [int(x) for x in seeds]}


def _is_oom(exc: BaseException) -> bool:
    name = type(exc).__name__.lower()
    msg = str(exc).lower()
    return "outofmemory" in name or "out of memory" in msg or "cuda oom" in msg


def _is_retryable_infra(exc: BaseException) -> bool:
    if _is_oom(exc) or isinstance(exc, (OSError, TimeoutError, ConnectionError)):
        return True
    if type(exc).__name__ == "BackendUnavailable":
        return True
    msg = str(exc).lower()
    return any(x in msg for x in (
        "interrupted", "cuda error", "cuda driver", "device-side",
        "cublas", "cusparse", "driver shutting down", "device unavailable"))


def _clear_cuda_cache(device: str) -> None:
    if not str(device).startswith("cuda"):
        return
    try:
        import torch
        torch.cuda.empty_cache()
    except Exception:
        pass


def run_job(client, worker_id: str, job: dict, device: str,
            execution_batch: int, grace_s: float = 30.0,
            runtime_info: dict | None = None, state: WorkerState = STATE,
            deliver: bool = True, data_dir: str | None = None) -> dict:
    """Execute one claimed job and deliver its result until acknowledged.
    Returns the result body (with ``delivery`` = accepted|duplicate|
    rejected|gone|abandoned when delivered).

    On SIGTERM (_stop set): the job keeps running while it is younger
    than grace_s; once older, it reports FAILED("interrupted") — it is
    never left RUNNING.
    """
    global _current_job_started
    state.set_job(job["job_id"], "running")
    genome = Genome.from_json(job["genome_json"])
    phenotype = develop(genome)
    config = job.get("config") or {}
    backend = None
    started = utcnow()
    _current_job_started = time.time()
    result_id = result_id_for(job["job_id"], worker_id,
                              int(job.get("attempt") or 0))
    try:
        backend = get_backend(
            job["backend"],
            **backend_kwargs(config, job.get("run_dir"), data_dir=data_dir))
        used_batch = max(1, int(execution_batch))
        while True:
            try:
                rep = evaluate_replicates(
                    backend, phenotype, job, device, used_batch,
                    grace_s=grace_s, deadline_started=_current_job_started)
                break
            except Exception as exc:
                if _is_oom(exc) and used_batch > 1:
                    used_batch = max(1, used_batch // 2)
                    _clear_cuda_cache(device)
                    continue
                raise
        summary = rep["summary"]
        trace_path = None
        if job.get("requested_traces"):
            traces_dir = Path(job["run_dir"]) / "traces"
            traces_dir.mkdir(parents=True, exist_ok=True)
            trace_path = str(traces_dir / f"{result_id}.json")
            Path(trace_path).write_text(json.dumps(summary, default=str))
        evaluation = {
            "evaluation_id": f"eval_{result_id[4:16]}",
            "result_id": result_id,
            "backend": job["backend"],
            "seed": job["seed"],
            "batch_size": rep["execution_batch_size"],
            "execution_batch_size": rep["execution_batch_size"],
            "requested_replicates": rep["requested_replicates"],
            "completed_replicates": rep["completed_replicates"],
            "replicate_seeds": rep["replicate_seeds"],
            "device": device,
            "environment_id": job["environment_id"],
            "duration_ms": float(job["duration_ms"]),
            "dataset": backend.dataset_identity(),
            "summary": summary,
            "started_at": started,
            "finished_at": utcnow(),
            "trace_path": trace_path,
        }
        if runtime_info:
            evaluation["runtime_info"] = runtime_info
        status, error = M.JOB_SUCCEEDED, None
    except Exception as exc:
        evaluation = None
        status = "RETRY" if _is_retryable_infra(exc) else M.JOB_FAILED
        error = f"{type(exc).__name__}: {exc}"
    finally:
        _current_job_started = None
    body = {"job_id": job["job_id"], "worker_id": worker_id,
            "result_id": result_id, "status": status,
            "evaluation": evaluation, "error": error}
    if deliver:
        state.set_job(job["job_id"], "delivering")
        body["delivery"] = deliver_result(client, body)
    state.set_job(None)
    return body


def deliver_result(client, body: dict, max_wait_s: float | None = None,
                   sleep=time.sleep) -> str:
    """POST /api/worker/result until the coordinator acknowledges.

    accepted  - 200 with ok (first commit)
    duplicate - 200 with duplicate=True (already committed; response of an
                earlier attempt was lost)
    rejected  - 409: job no longer RUNNING under us (reclaimed) -> stop
    gone      - 404: job unknown to the coordinator -> stop
    abandoned - max_wait_s exceeded (only when given) or worker stopping
                and nobody reachable
    Connection errors and 5xx retry with capped exponential backoff.
    """
    delay = 1.0
    t0 = time.time()
    while True:
        try:
            resp = client.post("/api/worker/result", json=body)
        except Exception:
            resp = None
        if resp is not None:
            if resp.status_code == 200:
                try:
                    data = resp.json()
                except ValueError:
                    data = {}
                return "duplicate" if data.get("duplicate") else "accepted"
            if resp.status_code == 409:
                return "rejected"
            if resp.status_code == 404:
                return "gone"
            if 400 <= resp.status_code < 500:
                return "rejected"
        if max_wait_s is not None and time.time() - t0 >= max_wait_s:
            return "abandoned"
        if _stop.is_set() and time.time() - t0 > 60:
            return "abandoned"
        sleep(delay)
        delay = min(delay * 2, 30.0)


def heartbeat_payload(worker_id: str, current_job_id=None,
                      device: str | None = None,
                      identity: dict | None = None) -> dict:
    g = gpu_info.query_device_gpu(device, identity) or {} if device else {}
    cpu = ram = None
    try:
        import psutil
        cpu = psutil.cpu_percent(interval=None)
        ram = psutil.virtual_memory().percent
    except ImportError:
        pass
    mie = []
    if g:
        mie.append({"timestamp": utcnow(), "source": f"gpu:{g.get('uuid')}",
                    "domain": "machine_interoception",
                    "signal_type": "temperature_c",
                    "value": g.get("temperature_c"), "confidence": 1.0,
                    "metadata": {"worker_id": worker_id, "device": device}})
    if cpu is not None:
        mie.append({"timestamp": utcnow(), "source": "host:cpu",
                    "domain": "machine_interoception",
                    "signal_type": "cpu_percent", "value": cpu,
                    "confidence": 1.0, "metadata": {}})
    return {
        "worker_id": worker_id,
        "device": device,
        "gpu_uuid": g.get("uuid") or (identity or {}).get("gpu_uuid"),
        "temperature_c": g.get("temperature_c"),
        "utilization_pct": g.get("utilization_pct"),
        "vram_used_mb": g.get("memory_used_mb"),
        "vram_total_mb": g.get("memory_total_mb"),
        "cpu_percent": cpu, "ram_percent": ram,
        "current_job_id": current_job_id,
        "mie_samples": mie,
    }


def fetch_profile(client, retries: int = 5) -> dict:
    delay = 1.0
    for _ in range(retries):
        try:
            r = client.get("/api/worker/profile")
            if r.status_code == 200:
                return r.json()
        except Exception:
            pass
        time.sleep(delay)
        delay = min(delay * 2, 30)
    raise RuntimeError("could not fetch /api/worker/profile")


def bench_on_profile(profile: dict, backend_name: str, device: str,
                     data_dir: str | None = None):
    """Benchmark on the experiment's own conditions (R2) and pick the
    execution batch under the configured VRAM headroom (R3)."""
    wk = profile.get("worker") or {}
    backend = get_backend(backend_name,
                          **profile_backend_kwargs(profile, data_dir=data_dir))
    phen = bench_phenotype(profile)
    n_drive = backend.n_base if hasattr(backend, "n_base") else \
        int((profile.get("fba") or {}).get("synthetic_neurons", 512))
    drive = make_drive((profile.get("evaluation") or {})
                       .get("environment_id", "synthetic-quiet-v0"),
                       n_drive, {"env": profile.get("env") or {}})
    rows = startup_benchmark(
        backend, phen, device=device,
        candidates=tuple(wk.get("candidates") or DEFAULT_CANDIDATES),
        duration_ms=float(wk.get("bench_duration_ms", 200)), drive=drive)
    headroom = float(wk.get("vram_headroom", DEFAULT_VRAM_HEADROOM))
    max_rep = int((profile.get("evaluation") or {}).get("replicates", 1))
    return rows, choose_batch(rows, vram_headroom=headroom,
                              max_batch=max(1, max_rep)), headroom


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(prog="mioba-worker")
    ap.add_argument("--coordinator", required=True)
    ap.add_argument("--device", default="cpu")
    ap.add_argument("--backend", default="mock")
    ap.add_argument("--execution-batch", "--batch", dest="batch",
                    default="auto",
                    help="GPU lanes per chunk (operational); auto = bench")
    ap.add_argument("--bench", action="store_true")
    ap.add_argument("--worker-id", default=None)
    ap.add_argument("--heartbeat-s", type=float, default=5.0)
    ap.add_argument("--grace-s", type=float, default=30.0)
    ap.add_argument("--data-dir", default=None,
                    help="local FBA dataset dir (real backend)")
    args = ap.parse_args(argv)

    import httpx
    signal.signal(signal.SIGTERM, _sigterm)
    worker_id = args.worker_id or default_worker_id(args.device)
    client = httpx.Client(base_url=args.coordinator, timeout=30.0)

    from ..fba.runtime_info import collect_runtime_info
    identity = gpu_info.gpu_identity(args.device)
    runtime_info = collect_runtime_info(backend=args.backend,
                                        device=args.device)
    runtime_info.update({k: v for k, v in identity.items() if v is not None})

    profile = fetch_profile(client)
    bench_rows, batch_size, headroom = [], None, None
    if args.bench or args.batch == "auto":
        rows, batch_size, headroom = bench_on_profile(
            profile, args.backend, args.device, data_dir=args.data_dir)
        bench_rows = [r.to_dict() for r in rows]
    if args.batch != "auto":
        batch_size = int(args.batch)

    payload = {"worker_id": worker_id, "hostname": socket.gethostname(),
               "device": args.device,
               "gpu": [g for g in [gpu_info.query_device_gpu(args.device,
                                                             identity)] if g],
               "gpu_identity": identity,
               "runtime_info": runtime_info,
               "bench": bench_rows, "batch_size": batch_size,
               "vram_headroom": headroom,
               "profile_scientific_config_hash":
                   profile.get("scientific_config_hash")}
    delay = 1.0
    while True:
        try:
            client.post("/api/worker/register", json=payload)
            break
        except Exception:
            if _stop.is_set():
                return 2
            time.sleep(delay)
            delay = min(delay * 2, 30)

    def hb_loop():
        while not _stop.is_set():
            try:
                client.post("/api/worker/heartbeat",
                            json=heartbeat_payload(worker_id,
                                                   STATE.current_job_id,
                                                   device=args.device,
                                                   identity=identity))
            except Exception:
                pass
            _stop.wait(args.heartbeat_s)

    threading.Thread(target=hb_loop, daemon=True).start()

    while not _stop.is_set():
        try:
            resp = client.post("/api/worker/claim",
                               json={"worker_id": worker_id,
                                     "batch_size": batch_size})
        except Exception:
            time.sleep(2.0)
            continue
        if resp.status_code == 204:
            time.sleep(1.0)
            continue
        job = resp.json()
        run_job(client, worker_id, job, args.device, batch_size or 1,
                grace_s=args.grace_s, runtime_info=runtime_info,
                data_dir=args.data_dir)
        if _stop.is_set():
            break
    return 0


if __name__ == "__main__":
    sys.exit(main())
