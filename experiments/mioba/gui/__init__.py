"""Observatory GUI: static single-page app + read-only aggregate endpoints.

The GUI never owns state. Every payload it renders is fetched from the
coordinator and tagged ``kind: LIVE | RECORDED | DERIVED``:

- LIVE      current runtime view (workers, queue counts, status)
- RECORDED  rows persisted in the lineage / telemetry DB
- DERIVED   values computed on request from recorded rows (throughput, ranks)

M1 §11 split the GUI in two. ``/api/gui/dashboard`` is unchanged and now
backs the **Infrastructure** page — GPU cards, queue depth, throughput,
checkpoints. The top page is the **Live Observatory**
(``/api/gui/live`` and friends, see gui/observatory.py), which shows the
population rather than the machines: who was born, what grew, which
crisis is running, who recovered, which branch died.

Nothing here is called from a simulation loop (§19): every endpoint reads
the lineage DB on request, and the page polls on a timer.
"""
from __future__ import annotations

import json
from datetime import datetime, timedelta, timezone
from pathlib import Path

from fastapi import HTTPException
from fastapi.responses import FileResponse, HTMLResponse

STATIC = Path(__file__).parent / "static"


def _iso_minutes_ago(minutes: float) -> str:
    return (datetime.now(timezone.utc) - timedelta(minutes=minutes)).isoformat()


def _throughput(db, experiment_id, minutes: float) -> dict:
    since = _iso_minutes_ago(minutes)
    evals = db.list_evaluations(experiment_id, limit=100000)
    recent = [e for e in evals if (e.get("finished_at") or "") >= since]
    sim_ms = 0.0
    wall_s = 0.0
    for e in recent:
        try:
            s = json.loads(e.get("summary_json") or "{}")
        except ValueError:
            s = {}
        lanes = e.get("completed_replicates") or e.get("batch_size") or 1
        sim_ms += float(s.get("t_ms") or 0.0) * float(lanes)
        try:
            wall_s += (datetime.fromisoformat(e["finished_at"]) -
                       datetime.fromisoformat(e["started_at"])).total_seconds()
        except (KeyError, TypeError, ValueError):
            pass
    return {
        "kind": "DERIVED",
        "window_minutes": minutes,
        "evaluations": len(recent),
        "evaluations_per_minute": round(len(recent) / minutes, 3) if minutes else None,
        "simulated_seconds_per_wall_second":
            round((sim_ms / 1000.0) / wall_s, 3) if wall_s > 0 else None,
    }


def _physical_gpu_key(w: dict) -> str:
    """Identity of the physical device a worker run used. A restarted
    worker creates a new ``worker_runs`` row for the same GPU; the GUI
    groups by this key so two GPUs stay two cards however often the
    workers are restarted."""
    if w.get("gpu_uuid"):
        return f"uuid:{w['gpu_uuid']}"
    if w.get("device"):
        return f"host:{w.get('hostname') or '?'}|dev:{w['device']}"
    return f"worker:{w['worker_id']}"


def _group_by_physical_gpu(workers: list[dict]) -> list[dict]:
    """One entry per physical GPU: the current (or most recent) run, plus
    the superseded runs as history and job totals across all of them."""
    groups: dict[str, list[dict]] = {}
    for w in workers:
        groups.setdefault(_physical_gpu_key(w), []).append(w)
    out = []
    for key, runs in groups.items():
        ordered = sorted(runs, key=lambda r: (r.get("status") == "online",
                                              r.get("started_at") or "",
                                              r.get("worker_run_id") or ""))
        current = dict(ordered[-1])
        past = ordered[:-1]
        current["kind"] = "DERIVED"
        current["gpu_key"] = key
        current["run_count"] = len(ordered)
        current["completed_jobs_total"] = sum(r.get("completed_jobs") or 0
                                              for r in ordered)
        current["failed_jobs_total"] = sum(r.get("failed_jobs") or 0
                                           for r in ordered)
        current["past_runs"] = [{
            "worker_run_id": r.get("worker_run_id"),
            "worker_id": r.get("worker_id"),
            "status": r.get("status"),
            "started_at": r.get("started_at"),
            "last_heartbeat_at": r.get("last_heartbeat_at"),
            "completed_jobs": r.get("completed_jobs"),
            "failed_jobs": r.get("failed_jobs"),
        } for r in reversed(past)]
        out.append(current)
    out.sort(key=lambda g: (g.get("gpu_index") if g.get("gpu_index")
                            is not None else 99, g.get("worker_id") or ""))
    return out


def mount_gui(app, service) -> None:
    db = service.db
    exp = service.experiment_id
    from . import observatory as OBS

    @app.get("/", response_class=HTMLResponse, include_in_schema=False)
    def index():
        return (STATIC / "index.html").read_text(encoding="utf-8")

    @app.get("/static/{name}", include_in_schema=False)
    def static(name: str):
        path = (STATIC / name).resolve()
        if path.parent != STATIC.resolve() or not path.is_file():
            raise HTTPException(404)
        return FileResponse(path)

    @app.get("/api/gui/dashboard")
    def dashboard(minutes: float = 10.0):
        since = _iso_minutes_ago(minutes)
        workers = []
        for w in db.list_workers(exp):
            series = db.worker_heartbeat_series(exp, w["worker_id"], since, 600)
            latest = series[-1] if series else {}
            try:
                gpus = json.loads(w.get("gpu_json") or "[]")
            except ValueError:
                gpus = []
            try:
                ri = json.loads(w.get("runtime_info_json") or "{}")
            except ValueError:
                ri = {}
            try:
                bench = json.loads(w.get("bench_json") or "[]")
            except ValueError:
                bench = []
            selected = next((b for b in bench
                             if b.get("batch") == w.get("batch_size")
                             and b.get("ok")), None)
            vram_total_bytes = ri.get("vram_total_bytes")
            workers.append({
                "kind": "LIVE",
                "worker_id": w["worker_id"],
                "worker_run_id": w.get("worker_run_id"),
                "started_at": w.get("started_at"),
                "hostname": w.get("hostname"),
                "status": w.get("status"),
                "gpu_name": ri.get("gpu_model") or
                            (gpus[0].get("name") if gpus else None),
                "gpus": gpus,
                "device": w.get("device") or ri.get("device"),
                "gpu_index": ri.get("gpu_index"),
                "gpu_uuid": ri.get("gpu_uuid"),
                "gpu_model": ri.get("gpu_model"),
                "compute_capability": ri.get("compute_capability"),
                "vram_total_bytes": vram_total_bytes,
                "driver": ri.get("driver"),
                "cuda_runtime": ri.get("cuda_runtime"),
                "torch_version": ri.get("torch_version"),
                "bench": {"kind": "RECORDED", "rows": bench,
                          "selected": selected},
                "bench_sim_seconds_per_wall_second":
                    (selected or {}).get("sim_seconds_per_wall_second"),
                "bench_throughput": (selected or {}).get("throughput"),
                "batch_size": w.get("batch_size"),
                "current_job_id": w.get("current_job_id"),
                "completed_jobs": w.get("completed_jobs"),
                "failed_jobs": w.get("failed_jobs"),
                "last_heartbeat_at": w.get("last_heartbeat_at"),
                "temperature_c": latest.get("temperature_c"),
                "utilization_pct": latest.get("utilization_pct"),
                "vram_used_mb": latest.get("vram_used_mb"),
                "vram_total_mb": latest.get("vram_total_mb"),
                "heartbeats": {"kind": "RECORDED", "rows": series},
            })
        events = db.list_events(exp, 0, 100000)
        return {
            "status": service.status(),
            "workers": workers,
            "gpu_workers": _group_by_physical_gpu(workers),
            "throughput": _throughput(db, exp, minutes),
            "recent_events": {"kind": "RECORDED", "rows": events[-30:]},
            "runtime": dict(service.runtime_info, kind="LIVE"),
            "checkpoints": {"kind": "RECORDED",
                            "rows": db.list_checkpoints(exp)[-5:][::-1]},
        }

    @app.get("/api/gui/genomes")
    def genomes(limit: int = 500, offset: int = 0):
        return {"kind": "RECORDED",
                "genomes": db.list_genomes_view(
                    exp, limit, offset, service.fba_base_neurons())}

    @app.get("/api/gui/genomes/{genome_id}")
    def genome(genome_id: str):
        g = db.get_genome(genome_id)
        if g is None:
            raise HTTPException(404, "no such genome")
        from ..development.phenotype import develop
        from ..genome.schema import Genome
        genome_obj = Genome.from_json(g["genome_json"])
        phen = develop(genome_obj, base_neurons=service.fba_base_neurons())
        gj = json.loads(g["genome_json"])
        return {
            "kind": "RECORDED",
            "genome": g,
            "genome_doc": gj,
            "parents": db.genome_parents(genome_id),
            "children": db.children_of(genome_id),
            "clades": db.genome_clades(genome_id),
            "mutations": db.genome_mutations(genome_id),
            "evaluations": db.list_evaluations(exp, genome_id=genome_id),
            "ancestry_chain": {"kind": "DERIVED",
                               "rows": db.ancestry_chain(genome_id)},
            "phenotype": {"kind": "DERIVED",
                          "base": phen["base"],
                          "n_extra_neurons": phen["n_extra_neurons"],
                          "params": phen["params"],
                          "ancestry_fraction": phen["ancestry_fraction"]},
        }

    # ----------------------------------------------------- Live Observatory
    @app.get("/api/gui/live")
    def live(since_id: int = 0):
        """Everything the top page shows, in one poll (M1 §12)."""
        return OBS.live_view(db, exp, service=service, since_id=since_id)

    @app.get("/api/gui/population")
    def population(generation: int | None = None):
        return {"kind": "DERIVED",
                **OBS.population_view(db, exp, generation=generation)}

    @app.get("/api/gui/lineage")
    def lineage(limit: int = 4000):
        return {"kind": "DERIVED", **OBS.lineage_view(db, exp, limit=limit)}

    @app.get("/api/gui/feed")
    def feed(since_id: int = 0, limit: int = 200):
        return {"kind": "RECORDED",
                **OBS.event_feed(db, exp, since_id=since_id, limit=limit)}

    @app.get("/api/gui/notable")
    def notable():
        return {"kind": "DERIVED", **OBS.notable_organisms(db, exp)}

    @app.get("/api/gui/individual/{genome_id}")
    def individual(genome_id: str):
        view = OBS.individual_view(db, exp, genome_id)
        if not view:
            raise HTTPException(404, "no such genome")
        return {"kind": "DERIVED", **view}

    @app.get("/api/gui/rate/{evaluation_id}")
    def rate(evaluation_id: str):
        """The canonical firing rate of one evaluation (M1 §18).

        The GUI displays this; fitness consumed the same field of the same
        row. If they ever differ again, this endpoint is where it shows.
        """
        ev = db.get_evaluation(evaluation_id)
        if ev is None:
            raise HTTPException(404, "no such evaluation")
        return {"kind": "DERIVED", **OBS.canonical_rate(ev)}

    @app.get("/api/gui/telemetry")
    def telemetry(minutes: float = 30.0, limit: int = 5000):
        since = _iso_minutes_ago(minutes)
        rows = db.telemetry_series(exp, since, None, limit)
        series: dict[str, list] = {}
        for r in rows:
            key = f"{r['source']}|{r['signal_type']}"
            series.setdefault(key, []).append(
                {"at": r["at"], "value": r["value"],
                 "normalized_value": r.get("normalized_value"),
                 "domain": r["domain"]})
        workers = {}
        for w in db.list_workers(exp):
            workers[w["worker_id"]] = db.worker_heartbeat_series(
                exp, w["worker_id"], since, limit)
        events = [e for e in db.list_events(exp, 0, 100000)
                  if e["at"] >= since]
        return {
            "kind": "RECORDED",
            "since": since,
            "telemetry": series,
            "worker_heartbeats": workers,
            "events": events,
            "status": service.status(),
        }
