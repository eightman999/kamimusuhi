"""Evaluation-throughput benchmark (M1 §2.2, §2.3).

Two questions this answers, both on the experiment's *own* conditions
(config backend / dataset / duration / replicates), never on a toy:

1. **How many concurrent evaluation slots should one GPU run?** M0 pinned
   one job per GPU. The figure of merit here is deliberately not per-job
   latency but ``successful evaluations per minute`` — a slot count that
   makes each evaluation 40% slower while running three at once is a win.
   OOM at a slot count is a result, not a crash: the sweep records it and
   moves on.

2. **What does one evaluation actually spend its time on?** Every
   evaluation is run under a :class:`~experiments.mioba.perf.PhaseTimer`,
   and the report carries both the per-evaluation breakdowns and their
   merge.

Slots are threads, not processes: the whole point of §2.1 is that the
139k/14M base graph is resident *once*, and separate processes would each
need their own copy. torch releases the GIL inside its kernels, so
threads overlap real work; the per-slot Python overhead shows up as
``unaccounted_s`` in the report if it ever stops being true.
"""
from __future__ import annotations

import threading
import time

from ..development.phenotype import develop
from ..fba.registry import get_backend
from ..fba.replicates import replicate_seeds
from ..genome.schema import Genome, fba0_genome
from .profile import PhaseTimer, merge_timings


def job_for(genome: Genome, config: dict, seed: int | None = None,
            duration_ms: float | None = None,
            replicates: int | None = None) -> dict:
    """A job payload shaped like the coordinator's ``claim`` response, so
    the benchmark exercises exactly the worker's evaluation path."""
    ev = dict(config.get("evaluation") or {})
    n_rep = int(replicates if replicates is not None
                else ev.get("replicates", 1))
    s = int(seed if seed is not None else genome.random_seed)
    return {
        "job_id": f"bench-{genome.genome_id[-8:] or 'root'}-{s}",
        "genome_json": genome.to_json(),
        "seed": s,
        "duration_ms": float(duration_ms if duration_ms is not None
                             else ev.get("duration_ms", 500)),
        "backend": ev.get("backend", "mock"),
        "environment_id": ev.get("environment_id", "synthetic-quiet-v0"),
        "requested_traces": [],
        "attempt": 0,
        "replicates": n_rep,
        "replicate_seeds": replicate_seeds(s, n_rep),
        "config": config,
    }


def _backend_kwargs(config: dict, run_dir: str | None = None,
                    data_dir: str | None = None) -> dict:
    from ..workers.worker import backend_kwargs
    return backend_kwargs(config, run_dir, data_dir=data_dir)


def run_one(genome: Genome, config: dict, device: str,
            execution_batch: int = 1, seed: int | None = None,
            duration_ms: float | None = None, replicates: int | None = None,
            data_dir: str | None = None, run_dir: str | None = None,
            profile: bool = True) -> dict:
    """One evaluation on this process/device. Returns
    ``{"summary", "timing", "wall_s", "ok", "error"}``."""
    from ..workers.worker import evaluate_replicates, make_timer
    job = job_for(genome, config, seed=seed, duration_ms=duration_ms,
                  replicates=replicates)
    timer = make_timer(device, enabled=profile)
    backend = get_backend(job["backend"],
                          **_backend_kwargs(config, run_dir, data_dir))
    with timer.phase("genome_decode"):
        decoded = Genome.from_json(job["genome_json"])
    with timer.phase("mutation_resolve"):
        phenotype = develop(decoded)
    t0 = time.perf_counter()
    try:
        rep = evaluate_replicates(backend, phenotype, job, device,
                                  max(1, int(execution_batch)), timer=timer)
    except Exception as exc:
        return {"ok": False, "error": f"{type(exc).__name__}: {exc}",
                "wall_s": time.perf_counter() - t0,
                "timing": timer.to_dict(), "summary": None,
                "job_id": job["job_id"]}
    return {"ok": True, "error": None, "wall_s": time.perf_counter() - t0,
            "timing": timer.to_dict(), "summary": rep["summary"],
            "execution_batch_size": rep["execution_batch_size"],
            "job_id": job["job_id"]}


def run_slots(genomes: list[Genome], config: dict, device: str, slots: int,
              execution_batch: int = 1, duration_ms: float | None = None,
              replicates: int | None = None, data_dir: str | None = None,
              run_dir: str | None = None) -> dict:
    """Run every genome once with ``slots`` concurrent evaluations.

    The figure of merit is successful evaluations per minute for the whole
    batch, not the latency of any one of them.
    """
    queue = list(enumerate(genomes))
    lock = threading.Lock()
    results: list[dict] = []

    def worker():
        while True:
            with lock:
                if not queue:
                    return
                i, g = queue.pop(0)
            r = run_one(g, config, device, execution_batch=execution_batch,
                        seed=g.random_seed or i, duration_ms=duration_ms,
                        replicates=replicates, data_dir=data_dir,
                        run_dir=run_dir)
            r["index"] = i
            with lock:
                results.append(r)

    t0 = time.perf_counter()
    threads = [threading.Thread(target=worker, name=f"slot{i}")
               for i in range(max(1, int(slots)))]
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    wall = time.perf_counter() - t0

    ok = [r for r in results if r["ok"]]
    failed = [r for r in results if not r["ok"]]
    oom = [r for r in failed if "out of memory" in (r["error"] or "").lower()]
    lat = sorted(r["wall_s"] for r in ok)
    return {
        "slots": int(slots),
        "execution_batch": int(execution_batch),
        "evaluations": len(results),
        "succeeded": len(ok),
        "failed": len(failed),
        "oom": len(oom),
        "wall_s": round(wall, 4),
        # the M1 §2.2 decision criterion
        "successful_evaluations_per_minute":
            (round(len(ok) * 60.0 / wall, 3) if wall > 0 else None),
        "mean_latency_s": (round(sum(lat) / len(lat), 4) if lat else None),
        "median_latency_s": (round(lat[len(lat) // 2], 4) if lat else None),
        "errors": sorted({r["error"] for r in failed if r["error"]}),
        "timing": merge_timings([r["timing"] for r in ok]),
        "per_evaluation": [
            {"index": r["index"], "job_id": r["job_id"], "ok": r["ok"],
             "wall_s": round(r["wall_s"], 4),
             "mean_rate_hz": ((r["summary"] or {}).get("mean_rate_hz")
                              if r["ok"] else None),
             "timing": r["timing"]}
            for r in sorted(results, key=lambda r: r["index"])],
    }


def slot_sweep(genomes: list[Genome], config: dict, device: str,
               slot_candidates: tuple[int, ...] = (1, 2, 3),
               execution_batch: int = 1, duration_ms: float | None = None,
               replicates: int | None = None, data_dir: str | None = None,
               run_dir: str | None = None) -> dict:
    """Run the same set of genomes at each slot count and pick the one
    with the highest successful evaluations/minute."""
    rows = []
    for slots in slot_candidates:
        rows.append(run_slots(genomes, config, device, slots,
                              execution_batch=execution_batch,
                              duration_ms=duration_ms, replicates=replicates,
                              data_dir=data_dir, run_dir=run_dir))
    usable = [r for r in rows
              if r["succeeded"] == r["evaluations"] and r["evaluations"]]
    best = max(usable, key=lambda r: r["successful_evaluations_per_minute"],
               default=None)
    return {"device": device, "rows": rows,
            "selected_slots": (best or {}).get("slots", 1),
            "selected_evaluations_per_minute":
                (best or {}).get("successful_evaluations_per_minute")}


def founder_population(n: int, base_seed: int = 0) -> list[Genome]:
    """``n`` distinct founder genomes for a benchmark: the pure FBA0 root
    plus parameter-mutated variants, so the run exercises the
    parameter-only cache path the real loop uses."""
    from ..genome.mutation import mutate
    import random
    rng = random.Random(base_seed)
    base = fba0_genome(seed=base_seed)
    out = [base]
    for i in range(1, max(1, n)):
        out.append(mutate(base, rng, birth_index=i, generation=0))
    return out


def activity_break_even(config: dict, device: str,
                        stim_rates_hz: tuple[float, ...] = (
                            5.0, 50.0, 200.0, 500.0, 1000.0, 2000.0),
                        stim_fraction: float = 0.05,
                        duration_ms: float = 50.0,
                        data_dir: str | None = None) -> dict:
    """Where does event-driven propagation stop paying off? (M1 §2.4-4)

    Event cost scales with the edges leaving neurons that spiked; the
    dense product costs ``nnz`` whatever happens. Somewhere between the
    two there is a crossover, and M1 can produce individuals on either
    side of it — a hyperactive mutant, or an evaluation under a severe
    disturbance. This drives the *same* network at increasing input rates
    and times both paths at each resulting activity level, so the
    crossover is measured on the run's own conditions rather than
    guessed.

    Returns one row per rate with the realised activity and both wall
    times, plus the lowest ``active_edge_ratio`` at which the dense path
    won.
    """
    import time as _time

    from ..fba.registry import get_backend
    from ..fba.semantics import (PROPAGATION_EVENT_CSC,
                                 PROPAGATION_SPARSE_CSR)
    from ..genome.schema import fba0_genome
    from ..mie.environments import make_drive

    rows = []
    for rate in stim_rates_hz:
        cfg = dict(config)
        cfg["env"] = dict(cfg.get("env") or {},
                          stim_fraction=stim_fraction, stim_rate_hz=rate)
        measured = {}
        for mode in (PROPAGATION_EVENT_CSC, PROPAGATION_SPARSE_CSR):
            kw = dict(_backend_kwargs(cfg, None, data_dir))
            kw["propagation_backend"] = mode
            backend = get_backend((cfg.get("evaluation") or {})
                                  .get("backend", "torch"), **kw)
            phen = develop(fba0_genome())
            backend.initialize(phen, batch_size=1, seed=0, device=device)
            n_drive = getattr(backend, "n_base", 512)
            backend.set_inputs(make_drive(
                (cfg.get("evaluation") or {}).get("environment_id",
                                                  "synthetic-quiet-v0"),
                n_drive, cfg))
            t0 = _time.perf_counter()
            backend.run(duration_ms)
            measured[mode] = {
                "wall_s": round(_time.perf_counter() - t0, 4),
                "activity": backend.activity_stats(),
                "mean_rate_hz": backend.get_state_summary()["mean_rate_hz"],
            }
        ev = measured[PROPAGATION_EVENT_CSC]
        dn = measured[PROPAGATION_SPARSE_CSR]
        rows.append({
            "stim_rate_hz": rate,
            "mean_rate_hz": ev["mean_rate_hz"],
            "active_neurons_per_step":
                ev["activity"]["active_neurons_per_step"],
            "active_edges_per_step": ev["activity"]["active_edges_per_step"],
            "active_edge_ratio": ev["activity"]["active_edge_ratio"],
            "event_wall_s": ev["wall_s"],
            "dense_wall_s": dn["wall_s"],
            "event_faster": ev["wall_s"] < dn["wall_s"],
            "speedup": (round(dn["wall_s"] / ev["wall_s"], 3)
                        if ev["wall_s"] > 0 else None),
        })
    losing = [r for r in rows if not r["event_faster"]]
    return {
        "device": device,
        "duration_ms": duration_ms,
        "stim_fraction": stim_fraction,
        "rows": rows,
        # the activity level at which the dense path first wins: the
        # right value for fba.dense_above on this device
        "break_even_active_edge_ratio":
            (min(r["active_edge_ratio"] for r in losing) if losing else None),
        "note": ("event-driven propagation won at every sampled rate"
                 if not losing else
                 "dense propagation won above the break-even ratio"),
    }
