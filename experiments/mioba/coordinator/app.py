"""FastAPI coordinator app.

Control endpoints (/api/control/*) require header
``X-Mioba-Token: <token>``; the token is written to
``<runs>/<exp>/control.token`` at start and read by the CLI.
"""
from __future__ import annotations

import json
import threading

from fastapi import Depends, FastAPI, Header, HTTPException, Request
from fastapi.responses import JSONResponse, Response

from ..gui import mount_gui
from ..storage import models as M
from ..storage.db import InvalidTransition
from . import lifecycle


def create_app(service) -> FastAPI:
    app = FastAPI(title="mioba-coordinator")
    app.state.service = service

    def require_token(x_mioba_token: str | None = Header(default=None)):
        if x_mioba_token != service.token:
            raise HTTPException(status_code=401, detail="bad control token")

    # ------------------------------------------------------------ reads
    @app.get("/api/status")
    def status():
        return service.status()

    @app.get("/api/workers")
    def workers():
        return {"kind": "LIVE",
                "workers": service.db.list_workers(service.experiment_id)}

    @app.get("/api/workers/{worker_id}")
    def worker(worker_id: str):
        w = service.db.get_worker(service.experiment_id, worker_id)
        if w is None:
            raise HTTPException(404, "no such worker")
        return w

    @app.get("/api/jobs")
    def jobs_list(status: str | None = None, limit: int = 100):
        return {"kind": "LIVE",
                "jobs": service.db.list_jobs(service.experiment_id, status,
                                             limit)}

    @app.post("/api/jobs")
    def enqueue(body: dict):
        jid = service.db.enqueue_job(
            service.experiment_id, body["genome_id"],
            body.get("environment_id", "synthetic-quiet-v0"),
            int(body.get("seed", 0)), body.get("evaluation_tier", "smoke"),
            body.get("backend", "mock"),
            float(body.get("duration_ms", 500)),
            body.get("requested_traces", []),
            int(body.get("priority", 0)))
        return {"job_id": jid}

    @app.post("/api/jobs/{job_id}/cancel")
    def cancel(job_id: str):
        try:
            service.db.cancel_job(job_id)
        except InvalidTransition as exc:
            raise HTTPException(409, str(exc))
        return {"ok": True}

    @app.get("/api/genomes")
    def genomes(limit: int = 100, offset: int = 0):
        return {"kind": "RECORDED",
                "genomes": service.db.list_genomes(service.experiment_id,
                                                   limit, offset)}

    @app.get("/api/genomes/{genome_id}")
    def genome(genome_id: str):
        g = service.db.get_genome(genome_id)
        if g is None:
            raise HTTPException(404, "no such genome")
        from ..development.phenotype import develop
        from ..genome.schema import Genome
        phen = develop(Genome.from_json(g["genome_json"]),
                       base_neurons=service.fba_base_neurons())
        return {
            "kind": "RECORDED",
            "genome": g,
            "parents": service.db.genome_parents(genome_id),
            "children": service.db.genome_children(genome_id),
            "clades": service.db.genome_clades(genome_id),
            "mutations": service.db.genome_mutations(genome_id),
            "evaluations": service.db.list_evaluations(
                service.experiment_id, genome_id=genome_id),
            "phenotype": {k: phen[k] for k in
                          ("n_extra_neurons", "param_overrides",
                           "ancestry_fraction", "base")},
        }

    @app.get("/api/genomes/{genome_id}/ancestry")
    def ancestry(genome_id: str):
        return {"kind": "RECORDED",
                "ancestry": service.db.ancestry(genome_id)}

    @app.get("/api/evaluations")
    def evaluations(genome_id: str | None = None, limit: int = 200):
        return {"kind": "RECORDED",
                "evaluations": service.db.list_evaluations(
                    service.experiment_id, genome_id=genome_id, limit=limit)}

    @app.get("/api/evaluations/{evaluation_id}")
    def evaluation(evaluation_id: str):
        e = service.db.get_evaluation(evaluation_id)
        if e is None:
            raise HTTPException(404, "no such evaluation")
        e["kind"] = "RECORDED"
        return e

    @app.get("/api/events")
    def events(since_id: int = 0, limit: int = 200, type: str | None = None):
        return {"kind": "RECORDED",
                "events": service.db.list_events(service.experiment_id,
                                                 since_id, limit, type)}

    @app.get("/api/telemetry")
    def telemetry(domain: str | None = None, signal_type: str | None = None,
                  since: str | None = None, limit: int = 200):
        return {"kind": "RECORDED",
                "samples": service.db.query_telemetry(
                    service.experiment_id, domain, signal_type, since, limit)}

    @app.get("/api/telemetry/latest")
    def telemetry_latest(limit: int = 200):
        return {"kind": "DERIVED",
                "samples": service.db.latest_telemetry(service.experiment_id,
                                                       limit)}

    @app.get("/api/runtime")
    def runtime():
        return dict(service.runtime_info, kind="LIVE")

    # ------------------------------------------------------------ control
    @app.post("/api/control/pause", dependencies=[Depends(require_token)])
    def ctl_pause():
        lifecycle.pause(service)
        return {"ok": True}

    @app.post("/api/control/resume", dependencies=[Depends(require_token)])
    def ctl_resume():
        lifecycle.resume(service)
        return {"ok": True}

    @app.post("/api/control/checkpoint", dependencies=[Depends(require_token)])
    def ctl_checkpoint():
        service.persist_state()
        return {"manifest": lifecycle.checkpoint(service, "manual")}

    @app.post("/api/control/stop", dependencies=[Depends(require_token)])
    def ctl_stop(body: dict | None = None):
        timeout = (body or {}).get("timeout_s")
        threading.Thread(target=lifecycle.stop, args=(service, timeout),
                         daemon=True).start()
        return {"ok": True, "status": "stopping"}

    # ------------------------------------------------------------ worker protocol
    @app.post("/api/worker/register")
    def wregister(body: dict):
        service.register_worker(body["worker_id"], body.get("hostname"),
                                body.get("gpu") or [],
                                body.get("runtime_info") or {},
                                body.get("bench") or [],
                                body.get("batch_size"))
        return {"ok": True, "experiment_id": service.experiment_id}

    @app.post("/api/worker/heartbeat")
    def wheartbeat(body: dict):
        service.heartbeat(body["worker_id"], body,
                          body.get("mie_samples") or [])
        return {"ok": True}

    @app.post("/api/worker/claim")
    def wclaim(body: dict):
        job = service.claim(body["worker_id"], body.get("batch_size"))
        if job is None:
            return Response(status_code=204)
        return job

    @app.post("/api/worker/result")
    def wresult(body: dict):
        try:
            service.worker_result(body["job_id"], body["worker_id"],
                                  body["status"], body.get("evaluation"),
                                  body.get("error"))
        except KeyError as exc:
            raise HTTPException(404, str(exc))
        except InvalidTransition as exc:
            raise HTTPException(409, str(exc))
        return {"ok": True}

    @app.exception_handler(InvalidTransition)
    def _invalid(req: Request, exc: InvalidTransition):
        return JSONResponse(status_code=409, content={"detail": str(exc)})

    mount_gui(app, service)
    return app
