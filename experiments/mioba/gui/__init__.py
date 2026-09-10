"""Observatory GUI: static single-page app + read-only aggregate endpoints.

The GUI never owns state. Every payload it renders is fetched from the
coordinator and tagged ``kind: LIVE | RECORDED | DERIVED``:

- LIVE      current runtime view (workers, queue counts, status)
- RECORDED  rows persisted in the lineage / telemetry DB
- DERIVED   values computed on request from recorded rows (throughput, ranks)
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
        sim_ms += float(s.get("t_ms") or 0.0) * float(e.get("batch_size") or 1)
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


def mount_gui(app, service) -> None:
    db = service.db
    exp = service.experiment_id

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
            workers.append({
                "kind": "LIVE",
                "worker_id": w["worker_id"],
                "hostname": w.get("hostname"),
                "status": w.get("status"),
                "gpu_name": (gpus[0].get("name") if gpus else None),
                "gpus": gpus,
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
