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
from ..perf import PhaseTimer
from ..fba.replicates import chunk_indices, replicate_seeds
from ..genome.schema import Genome, utcnow
from ..mie.environments import make_drive
from ..mie.episode import run_episode
from ..storage import models as M
from . import gpu_info
from .bench import (DEFAULT_CANDIDATES, DEFAULT_VRAM_HEADROOM, bench_phenotype,
                    choose_batch, profile_backend_kwargs, startup_benchmark)

_stop = threading.Event()
_current_job_started = None


class _SlotLocal(threading.local):
    """Which job the calling slot thread currently holds."""
    job_id: str | None = None


_SLOT = _SlotLocal()


class WorkerState:
    """Shared between the job loops and the heartbeat thread.

    With ``--slots N`` (M1 §2.2) several evaluations run concurrently in
    one process, so the state holds a set of in-flight jobs. The
    heartbeat still reports a single ``current_job_id`` (the schema's
    column) plus the full list, and every slot's job is reported, so a
    lost worker still requeues all of them.
    """

    def __init__(self):
        self._lock = threading.Lock()
        self._jobs: dict[str, str] = {}     # job_id -> phase
        self.phase = "idle"

    def set_job(self, job_id: str | None, phase: str = "running",
                slot: str | None = None) -> None:
        """Backwards-compatible single-slot API: ``None`` clears the job
        this thread was holding."""
        with self._lock:
            if job_id is None:
                held = slot or _SLOT.job_id
                if held:
                    self._jobs.pop(held, None)
                else:
                    self._jobs.clear()
            else:
                self._jobs[job_id] = phase
            _SLOT.job_id = job_id
            self.phase = (phase if self._jobs else "idle")

    @property
    def current_job_id(self) -> str | None:
        with self._lock:
            return next(iter(self._jobs), None)

    @property
    def current_job_ids(self) -> list[str]:
        with self._lock:
            return sorted(self._jobs)


STATE = WorkerState()


class SlotGovernor:
    """Caps how many evaluations this process runs at once, and backs off
    when the device says no (M1 §2.4-7).

    A GPU OOM at three concurrent slots is a statement about the
    scheduler, not about the organism: the same genome under the same
    evaluation seed fits perfectly well at two. So the governor
    permanently retires a permit on OOM, the job goes back to the queue
    unchanged, and the run continues narrower instead of recording a
    death that never happened.

    Backoff is one-way within a worker's life: re-widening would
    re-discover the same OOM, and a worker restart is the cheap way to
    try a wider setting again.
    """

    def __init__(self, slots: int = 1, minimum: int = 1):
        self.limit = max(1, int(slots))
        self.minimum = max(1, int(minimum))
        self._sem = threading.Semaphore(self.limit)
        self._lock = threading.Lock()
        self.backoffs: list[dict] = []

    def acquire(self) -> None:
        self._sem.acquire()

    def release(self) -> None:
        self._sem.release()

    def back_off(self, reason: str = "cuda_oom") -> int:
        """Retire one permit. Returns the new limit."""
        with self._lock:
            if self.limit <= self.minimum:
                self.backoffs.append({"at": utcnow(), "reason": reason,
                                      "limit": self.limit, "applied": False})
                return self.limit
            self.limit -= 1
            self.backoffs.append({"at": utcnow(), "reason": reason,
                                  "limit": self.limit, "applied": True})
        # consume a permit in the background so an in-flight slot is not
        # interrupted; the narrower limit takes effect as slots finish
        threading.Thread(target=self._sem.acquire, daemon=True).start()
        return self.limit


GOVERNOR = SlotGovernor(1)


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
    if fba.get("base_seed") is not None:
        kw["base_seed"] = int(fba["base_seed"])
    if fba.get("propagation_backend"):
        kw["propagation_backend"] = str(fba["propagation_backend"])
    if fba.get("dense_above") is not None:
        kw["dense_above"] = float(fba["dense_above"])
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


def environment_enabled(config: dict) -> bool:
    """M1 §5: run episodes against a virtual environment unless the
    experiment explicitly opts out (M0-style constant conditions)."""
    env = (config or {}).get("environment") or {}
    if not env:
        return False
    if env.get("enabled") is False:
        return False
    return bool((env.get("disturbance") or {}).get("enabled", True)
                or (env.get("virtual") or {}).get("enabled", True))


def task_rate_hz(config: dict) -> float:
    return float(((config or {}).get("env") or {}).get("stim_rate_hz", 50.0))


def task_range(config: dict, n_base: int) -> tuple[int, int]:
    frac = float(((config or {}).get("env") or {}).get("stim_fraction", 0.01))
    return 0, max(1, int(round(n_base * frac)))


def make_environment(job: dict, config: dict, n_base: int,
                     replicate_index: int):
    """One lane's environment, seeded from that replicate's disturbance
    stream so lanes sample different crises while any replay of this
    evaluation meets the same ones (fba/seeds.py)."""
    from ..fba.seeds import disturbance_seed
    from ..mie.virtual_env import VirtualEnvironment

    return VirtualEnvironment(
        seed=disturbance_seed(int(job["seed"]), int(replicate_index)),
        config=config, n_base=n_base,
        task_neurons=task_range(config, n_base))


def make_timer(device: str, enabled: bool = True) -> PhaseTimer:
    """Phase timer for one evaluation; on CUDA it synchronises at phase
    boundaries so asynchronous kernel time is attributed correctly."""
    sync = None
    if enabled and str(device).startswith("cuda"):
        try:
            import torch
            if torch.cuda.is_available():
                dev = torch.device(device)
                sync = lambda: torch.cuda.synchronize(dev)  # noqa: E731
        except Exception:
            sync = None
    return PhaseTimer(enabled=enabled, sync=sync, label=str(device))


def evaluate_replicates(backend, phenotype: dict, job: dict, device: str,
                        execution_batch: int, grace_s: float = 30.0,
                        deadline_started: float | None = None,
                        timer: PhaseTimer | None = None) -> dict:
    """Run every scientific replicate of ``job`` in chunks of
    ``execution_batch`` lanes and aggregate per-replicate results.

    Returned ``summary`` is ordered by replicate index; ``mean_rate_hz``
    is the mean over replicates. Independent of ``execution_batch``.
    ``timer`` (M1 §2) records the per-phase breakdown into
    ``summary["timing"]``; it only measures and never changes results.
    """
    timer = timer or PhaseTimer(enabled=False)
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
    state: dict = {}
    env_enabled = environment_enabled(config)
    target_rate = float((config.get("evaluation") or {})
                        .get("target_rate_hz", 5.0))
    episodes: list[dict] = []
    lane_reports: dict[int, dict] = {}
    for lanes in chunks:
        backend.initialize(phenotype, batch_size=len(lanes), seed=job["seed"],
                           device=device,
                           replicate_seeds=[seeds[i] for i in lanes],
                           timer=timer)
        n_drive = backend.n_base if hasattr(backend, "n_base") else 512
        t_chunk = time.perf_counter()
        if env_enabled:
            # M1 §5-§7: the episode is run slice by slice against a
            # seeded virtual environment, so crises land at reproducible
            # but non-fixed times and their consequences are felt
            envs = [make_environment(job, config, n_drive, i)
                    for i in lanes]
            with timer.phase("state_init"):
                pass
            ep = run_episode(backend, envs, duration, target_rate,
                             task_rate_hz(config), timer=timer)
            episodes.append(ep)
            for lane_idx, lane in enumerate(lanes):
                lane_reports[lane] = ep["per_lane"][lane_idx]
        else:
            with timer.phase("state_init"):
                backend.set_inputs(make_drive(job["environment_id"], n_drive,
                                              config))
            remaining = duration
            with timer.phase("simulation_loop"):
                while remaining > 1e-9:
                    if _stop.is_set() and deadline_started is not None and \
                            time.time() - deadline_started > grace_s:
                        raise RuntimeError("interrupted")
                    chunk = min(50.0, remaining)
                    backend.run(chunk)
                    remaining -= chunk
        total_wall += time.perf_counter() - t_chunk
        with timer.phase("metrics"):
            s = backend.get_state_summary()
            state = s
            t_ms = s.get("t_ms", t_ms)
            _stack(per_rep, "mean_rate_hz", s["per_batch_mean_rate_hz"])
            _stack(per_rep, "spike_counts", s["per_batch_spike_counts"])
            _stack(per_rep, "seeds", list(s.get("replicate_seeds") or
                                          [seeds[i] for i in lanes]))
            _stack(per_rep, "index", lanes)
            for g, vals in backend.get_population_activity(
                    ["all", "fba0"]).items():
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
        "vram": state.get("vram"),
        # circuit size of the evaluated organism (M1 §8 resource cost)
        "circuit": {
            "n_neurons": state.get("n_neurons"),
            "n_base": state.get("n_base"),
            "n_artificial_neurons": state.get("n_artificial_neurons"),
            "n_artificial_organs": state.get("n_artificial_organs"),
            "n_attachments": state.get("n_attachments"),
            "nnz": state.get("nnz"),
            "nnz_artificial": state.get("nnz_artificial"),
        },
        "topology": state.get("topology"),
        # M1 §5-§7 / §9.1: per-slice performance under the seeded virtual
        # environment, and the response to each disturbance
        "episode": ({"slices": episodes[0]["slices"],
                     "slice_ms": episodes[0]["slice_ms"],
                     "simulated_ms": sum(e["simulated_ms"] for e in episodes),
                     "per_lane": [lane_reports[i] for i in sorted(lane_reports)],
                     "environments": [env for e in episodes
                                      for env in e["environments"]]}
                    if episodes else None),
        # how much of the network was actually touched (M1 §2.4-4) and what
        # this individual cost on top of the shared base (M1 §8)
        "activity": state.get("activity"),
        "resource": state.get("resource"),
        "semantics": (state.get("semantics")
                      or (backend.semantics()
                          if hasattr(backend, "semantics")
                          else None)),
        "timing": timer.to_dict() if timer.enabled else None,
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
            deliver: bool = True, data_dir: str | None = None,
            profile: bool = True) -> dict:
    """Execute one claimed job and deliver its result until acknowledged.
    Returns the result body (with ``delivery`` = accepted|duplicate|
    rejected|gone|abandoned when delivered).

    On SIGTERM (_stop set): the job keeps running while it is younger
    than grace_s; once older, it reports FAILED("interrupted") — it is
    never left RUNNING.
    """
    global _current_job_started
    state.set_job(job["job_id"], "running")
    timer = make_timer(device, enabled=profile)
    with timer.phase("genome_decode"):
        genome = Genome.from_json(job["genome_json"])
    with timer.phase("mutation_resolve"):
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
                    grace_s=grace_s, deadline_started=_current_job_started,
                    timer=timer)
                break
            except Exception as exc:
                if _is_oom(exc) and used_batch > 1:
                    # halve the lanes first: cheaper than giving the job
                    # back, and the replicate set is unchanged
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
            "semantics": summary.get("semantics"),
            "activity": summary.get("activity"),
            "resource": summary.get("resource"),
            "summary": summary,
            "started_at": started,
            "finished_at": utcnow(),
            "trace_path": trace_path,
        }
        if runtime_info:
            evaluation["runtime_info"] = runtime_info
        if timer.enabled:
            # refresh: genome decode / trace write / teardown land after
            # evaluate_replicates took its snapshot
            summary["timing"] = timer.to_dict()
        status, error = M.JOB_SUCCEEDED, None
        retry_reason = None
    except Exception as exc:
        evaluation = None
        # A device resource limit is a statement about how this worker
        # scheduled the job, not about the organism: the job goes back to
        # the queue with its evaluation seed intact and is re-run
        # narrower. It is never a FAILED evaluation (M1 2.4-7).
        if _is_oom(exc):
            status, retry_reason = "RETRY", M.RETRY_RUNTIME_RESOURCE
        elif _is_retryable_infra(exc):
            status, retry_reason = "RETRY", M.RETRY_INFRASTRUCTURE
        else:
            status, retry_reason = M.JOB_FAILED, None
        error = f"{type(exc).__name__}: {exc}"
    finally:
        _current_job_started = None
    body = {"job_id": job["job_id"], "worker_id": worker_id,
            "result_id": result_id, "status": status,
            "evaluation": evaluation, "error": error,
            "retry_reason": retry_reason}
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
                      identity: dict | None = None,
                      current_job_ids: list[str] | None = None,
                      slots: int | None = None) -> dict:
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
        "current_job_ids": (list(current_job_ids)
                            if current_job_ids is not None
                            else ([current_job_id] if current_job_id else [])),
        "slots": slots,
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
    ap.add_argument("--slots", type=int, default=None,
                    help="concurrent evaluations on this device "
                         "(default: worker.slots from the experiment "
                         "profile, else 1). Operational, like "
                         "--execution-batch: it changes throughput, never "
                         "the replicate set or fitness.")
    ap.add_argument("--no-profile", action="store_true",
                    help="skip the per-evaluation phase breakdown")
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
    slots = int(args.slots if args.slots is not None
                else (profile.get("worker") or {}).get("slots", 1) or 1)
    global GOVERNOR
    GOVERNOR = SlotGovernor(slots)

    payload = {"worker_id": worker_id, "hostname": socket.gethostname(),
               "device": args.device,
               "gpu": [g for g in [gpu_info.query_device_gpu(args.device,
                                                             identity)] if g],
               "gpu_identity": identity,
               "runtime_info": runtime_info,
               "bench": bench_rows, "batch_size": batch_size,
               "slots": slots, "vram_headroom": headroom,
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
                            json=heartbeat_payload(
                                worker_id, STATE.current_job_id,
                                device=args.device, identity=identity,
                                current_job_ids=STATE.current_job_ids,
                                slots=slots))
            except Exception:
                pass
            _stop.wait(args.heartbeat_s)

    threading.Thread(target=hb_loop, daemon=True).start()

    def claim_loop():
        """One evaluation slot: claim -> evaluate -> deliver -> claim.

        Slots are independent; an empty queue backs one slot off without
        stalling the others, and each slot's backend shares the process's
        resident base topology (fba/topology.py).
        """
        while not _stop.is_set():
            try:
                resp = client.post("/api/worker/claim",
                                   json={"worker_id": worker_id,
                                         "batch_size": batch_size})
            except Exception:
                _stop.wait(2.0)
                continue
            if resp.status_code == 204:
                _stop.wait(1.0)
                continue
            job = resp.json()
            GOVERNOR.acquire()
            try:
                body = run_job(client, worker_id, job, args.device,
                               batch_size or 1, grace_s=args.grace_s,
                               runtime_info=runtime_info,
                               data_dir=args.data_dir,
                               profile=not args.no_profile)
            finally:
                GOVERNOR.release()
            if body.get("retry_reason") == M.RETRY_RUNTIME_RESOURCE:
                # the device ran out of room at this concurrency; run
                # narrower rather than letting the next job hit the same
                limit = GOVERNOR.back_off()
                print(f"[{worker_id}] device resource limit; "
                      f"concurrent slots -> {limit}", flush=True)

    if slots <= 1:
        claim_loop()
        return 0
    threads = [threading.Thread(target=claim_loop, name=f"slot{i}",
                                daemon=True) for i in range(slots)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    return 0


if __name__ == "__main__":
    sys.exit(main())
