"""MIOBA worker process.

    python -m experiments.mioba.workers.worker \
        --coordinator http://host:8765 --device cuda:0|cpu \
        --backend torch|mock|genn --batch auto|N [--bench] \
        [--worker-id ...] [--heartbeat-s 5]

Loop: register -> (bench if --bench or batch=auto) -> claim -> develop ->
backend.initialize -> run -> summary -> result. A heartbeat thread runs
the whole time. SIGTERM: finish the current job if it has been running
for < grace_s, else report FAILED("interrupted") and exit.
The worker never marks anything succeeded on its own.
"""
from __future__ import annotations

import argparse
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
from ..genome.schema import Genome, utcnow
from ..mie.environments import make_drive
from ..storage import models as M
from . import gpu_info
from .bench import choose_batch, startup_benchmark

_stop = threading.Event()
_current_job_started = None


def _sigterm(signum, frame):
    _stop.set()


def default_worker_id() -> str:
    return f"{socket.gethostname()}-gpu0-{uuid.uuid4().hex[:8]}"


def backend_kwargs(config: dict, run_dir: str | None) -> dict:
    fba = (config or {}).get("fba", {})
    return {"data_dir": fba.get("data_dir"),
            "synthetic": bool(fba.get("synthetic", True)),
            "synthetic_neurons": int(fba.get("synthetic_neurons", 2000)),
            "runs_dir": run_dir}


def run_job(client, worker_id: str, job: dict, device: str,
            batch_size: int, grace_s: float = 30.0) -> dict:
    """Execute one claimed job and POST the result. Returns the result body.

    On SIGTERM (_stop set): the job keeps running while it is younger
    than grace_s; once older, it reports FAILED("interrupted") — it is
    never left RUNNING.
    """
    global _current_job_started
    genome = Genome.from_json(job["genome_json"])
    phenotype = develop(genome)
    config = job.get("config") or {}
    backend = get_backend(job["backend"],
                          **backend_kwargs(config, job.get("run_dir")))
    started = utcnow()
    _current_job_started = time.time()
    try:
        backend.initialize(phenotype, batch_size=batch_size,
                           seed=job["seed"], device=device)
        drive = make_drive(job["environment_id"], backend.n_base
                           if hasattr(backend, "n_base") else 512, config)
        backend.set_inputs(drive)
        remaining = float(job["duration_ms"])
        while remaining > 1e-9:
            if _stop.is_set() and \
                    time.time() - _current_job_started > grace_s:
                raise RuntimeError("interrupted")
            chunk = min(50.0, remaining)
            backend.run(chunk)
            remaining -= chunk
        summary = backend.get_state_summary()
        summary["activity"] = backend.get_population_activity(["all", "fba0"])
        trace_path = None
        if job.get("requested_traces"):
            import numpy as np
            traces_dir = Path(job["run_dir"]) / "traces"
            traces_dir.mkdir(parents=True, exist_ok=True)
            eval_stub = f"{job['job_id']}-{worker_id}"
            trace_path = str(traces_dir / f"{eval_stub}.npz")
            np.savez(trace_path,
                     spike_counts=backend.spike_counts
                     if hasattr(backend, "spike_counts") else [],
                     summary=json.dumps(summary, default=str))
        evaluation = {
            "backend": job["backend"],
            "seed": job["seed"],
            "batch_size": batch_size,
            "summary": summary,
            "started_at": started,
            "finished_at": utcnow(),
            "trace_path": trace_path,
        }
        status, error = M.JOB_SUCCEEDED, None
    except Exception as exc:
        evaluation = None
        status, error = M.JOB_FAILED, f"{type(exc).__name__}: {exc}"
    finally:
        _current_job_started = None
    body = {"job_id": job["job_id"], "worker_id": worker_id,
            "status": status, "evaluation": evaluation, "error": error}
    client.post("/api/worker/result", json=body)
    return body


def heartbeat_payload(worker_id: str, current_job_id=None) -> dict:
    gpus = gpu_info.query_gpus()
    g = gpus[0] if gpus else {}
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
                    "metadata": {}})
    if cpu is not None:
        mie.append({"timestamp": utcnow(), "source": "host:cpu",
                    "domain": "machine_interoception",
                    "signal_type": "cpu_percent", "value": cpu,
                    "confidence": 1.0, "metadata": {}})
    return {
        "worker_id": worker_id,
        "temperature_c": g.get("temperature_c"),
        "utilization_pct": g.get("utilization_pct"),
        "vram_used_mb": g.get("memory_used_mb"),
        "vram_total_mb": g.get("memory_total_mb"),
        "cpu_percent": cpu, "ram_percent": ram,
        "current_job_id": current_job_id,
        "mie_samples": mie,
    }


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(prog="mioba-worker")
    ap.add_argument("--coordinator", required=True)
    ap.add_argument("--device", default="cpu")
    ap.add_argument("--backend", default="mock")
    ap.add_argument("--batch", default="1")
    ap.add_argument("--bench", action="store_true")
    ap.add_argument("--worker-id", default=None)
    ap.add_argument("--heartbeat-s", type=float, default=5.0)
    ap.add_argument("--grace-s", type=float, default=30.0)
    args = ap.parse_args(argv)

    import httpx
    signal.signal(signal.SIGTERM, _sigterm)
    worker_id = args.worker_id or default_worker_id()
    client = httpx.Client(base_url=args.coordinator, timeout=30.0)

    bench_rows, batch_size = [], None
    if args.bench or args.batch == "auto":
        backend = get_backend(args.backend, **backend_kwargs({}, None))
        rows = startup_benchmark(backend, {}, device=args.device)
        bench_rows = [r.to_dict() for r in rows]
        batch_size = choose_batch(rows)
    elif args.batch != "auto":
        batch_size = int(args.batch)

    # register with retry/backoff
    from ..fba.runtime_info import collect_runtime_info
    payload = {"worker_id": worker_id, "hostname": socket.gethostname(),
               "gpu": gpu_info.query_gpus(),
               "runtime_info": collect_runtime_info(backend=args.backend),
               "bench": bench_rows, "batch_size": batch_size}
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
                            json=heartbeat_payload(worker_id))
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
        run_job(client, worker_id, job, args.device,
                batch_size or 1, grace_s=args.grace_s)
        if _stop.is_set():
            break
    return 0


if __name__ == "__main__":
    sys.exit(main())
