"""Loopback-only monitoring and cooperative run control; trainers are detached."""
import argparse
import asyncio
import json
import os
import sqlite3
import subprocess
import sys
import threading
import time
import uuid
from pathlib import Path

from fastapi import FastAPI, HTTPException, Request, WebSocket, WebSocketDisconnect
from fastapi.responses import JSONResponse
from .protocol import StartRequest, ControlRequest
from ..telemetry.store import RUN_ID
from ..telemetry.system_stats import snapshot

PACKAGE_ROOT = Path(__file__).resolve().parents[1]
REPO_ROOT = PACKAGE_ROOT.parents[1]


def read_json(path, default=None):
    try:
        return json.loads(path.read_text())
    except (OSError, ValueError):
        return default


def atomic_json(path, data):
    temporary = path.with_name(path.name + "." + uuid.uuid4().hex + ".tmp")
    temporary.write_text(json.dumps(data, allow_nan=False))
    os.replace(temporary, path)


def tail_jsonl(path, limit=1000):
    from collections import deque
    try:
        with path.open() as stream:
            rows = deque(stream, maxlen=limit)
        result = []
        for row in rows:
            try:
                result.append(json.loads(row))
            except ValueError:
                pass  # A trainer may currently be appending the final line.
        return result
    except OSError:
        return []


def create_app(artifacts=None):
    root = Path(artifacts or PACKAGE_ROOT / "artifacts").resolve()
    root.mkdir(parents=True, exist_ok=True)
    runs_root = root / "runs"
    runs_root.mkdir(exist_ok=True)
    app = FastAPI(title="K0 Brainstem Monitor")
    lock = threading.Lock()

    @app.middleware("http")
    async def no_cross_origin_control(request: Request, call_next):
        # Native Qt and CLI clients send no Origin. Reject browser-origin writes.
        if request.method == "POST" and request.headers.get("origin"):
            return JSONResponse({"detail": "browser-origin controls disabled"}, status_code=403)
        return await call_next(request)

    def run_path(run_id, must_exist=True):
        if not RUN_ID.fullmatch(run_id):
            raise HTTPException(400, "invalid run ID")
        run = (runs_root / run_id).resolve()
        if run.parent != runs_root or (must_exist and not run.is_dir()):
            raise HTTPException(404, "unknown run")
        return run

    def history(run_id, limit=2000):
        run = run_path(run_id)
        db_path = root / "metrics.sqlite"
        if db_path.exists():
            try:
                with sqlite3.connect(db_path.as_uri() + "?mode=ro", uri=True, timeout=2) as db:
                    rows = db.execute("SELECT payload FROM metrics WHERE run_id=? ORDER BY id DESC LIMIT ?", (run_id, limit)).fetchall()
                return [json.loads(row[0]) for row in reversed(rows)]
            except (sqlite3.Error, ValueError):
                pass
        return tail_jsonl(run / "metrics.jsonl", limit)

    def run_record(run_id):
        run = run_path(run_id)
        config = read_json(run / "config.json", {})
        status = read_json(run / "status.json", {})
        metrics = history(run_id, 1)
        return {**config, **status, "run_id": run_id, "physical_gpu": read_json(run / "gpu.json", {}), "latest": metrics[-1] if metrics else {}}

    def list_runs():
        return [run_record(path.name) for path in sorted(runs_root.iterdir())
                if path.is_dir() and not path.is_symlink() and RUN_ID.fullmatch(path.name)]

    def spawn(run, config_path, device, resume=False):
        queue = read_json(root / "queue.json", {})
        if queue.get("pending") or queue.get("active"):
            raise HTTPException(409, "experiment queue owns scheduling; wait until the queue finishes")
        active = [r for r in list_runs() if r.get("status") in ("queued", "running", "paused") and r["run_id"] != run.name]
        gpu = None
        environment = dict(os.environ)
        launch_device = device
        if device.startswith("cuda:"):
            hardware = snapshot()
            if hardware.get("gpu_error"):
                raise HTTPException(409, "GPU process inventory unavailable")
            gpu = next((g for g in hardware["gpus"] if g["index"] == int(device.split(":")[1])), None)
            if gpu is None:
                raise HTTPException(409, "requested GPU unavailable")
            if any(p["gpu_uuid"] == gpu["uuid"] for p in hardware.get("gpu_processes", [])):
                raise HTTPException(409, "GPU occupied by an existing compute process")
            if any(r.get("physical_gpu", {}).get("uuid") == gpu["uuid"] or (not r.get("physical_gpu") and r.get("device") == device) for r in active):
                raise HTTPException(409, "GPU already assigned to an active run")
            environment["CUDA_VISIBLE_DEVICES"] = gpu["uuid"]
            launch_device = "cuda:0"
        run.mkdir(exist_ok=True)
        command = [sys.executable, "-m", "experiments.k0_brainstem.train.trainer", "--config", str(config_path), "--artifacts", str(root), "--run-id", run.name, "--device", launch_device]
        if resume:
            command.append("--resume")
        atomic_json(run / "status.json", {"run_id": run.name, "status": "queued", "device": device, "timestamp": time.time()})
        if gpu:
            atomic_json(run / "gpu.json", gpu)
        atomic_json(run / "launch.json", {"config": config_path.name, "device": device})
        try:
            with (run / "trainer.log").open("a") as log:
                process = subprocess.Popen(command, cwd=REPO_ROOT, env=environment, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        except OSError:
            atomic_json(run / "status.json", {"status": "failed", "device": device})
            raise HTTPException(500, "trainer failed to launch")
        return {"run_id": run.name, "status": "queued", "pid": process.pid}

    @app.get("/api/status")
    def status():
        return {"status": "online", "timestamp": time.time(), "runs": len(list_runs()), "protocol_version": 1}

    @app.get("/api/configs")
    def configs():
        return [path.name for path in sorted((PACKAGE_ROOT / "configs").glob("*.yaml"))]

    @app.get("/api/runs")
    def runs():
        return list_runs()

    @app.get("/api/runs/{run_id}")
    def run(run_id: str, limit: int = 2000):
        record = run_record(run_id)
        record["metrics"] = history(run_id, max(1, min(limit, 20000)))
        return record

    @app.get("/api/runs/{run_id}/episodes")
    def episodes(run_id: str):
        return read_json(run_path(run_id) / "episodes.json", [])

    @app.get("/api/runs/{run_id}/language-events")
    def events(run_id: str):
        return tail_jsonl(run_path(run_id) / "language_events.jsonl", 500)

    @app.get("/api/system")
    def system():
        return snapshot()

    @app.get("/api/gpus")
    def gpus():
        return snapshot()["gpus"]

    @app.post("/api/run/start")
    def start(body: StartRequest):
        config = (PACKAGE_ROOT / "configs" / body.config).resolve()
        if config.parent != PACKAGE_ROOT / "configs" or not config.is_file():
            raise HTTPException(400, "unknown configuration")
        with lock:
            run_id = body.run_id or f"gui-{int(time.time())}-{uuid.uuid4().hex[:8]}"
            run = run_path(run_id, must_exist=False)
            if run.exists():
                raise HTTPException(409, "run ID already exists")
            return spawn(run, config, body.device)

    def control(body, command):
        with lock:
            run = run_path(body.run_id)
            status = read_json(run / "status.json", {}).get("status")
            if command == "resume" and status == "stopped":
                config = run / "config.json"
                if not config.is_file() or not (run / "checkpoint.pt").is_file():
                    raise HTTPException(409, "saved configuration or checkpoint unavailable")
                stored = read_json(config, {})
                gpu = read_json(run / "gpu.json", {})
                device = f"cuda:{gpu['index']}" if gpu else stored.get("device", "cpu")
                atomic_json(run / "control.json", {"command": "resume"})
                return spawn(run, config, device, resume=True)
            if status not in ("running", "paused", "queued"):
                raise HTTPException(409, "run is not active")
            atomic_json(run / "control.json", {"command": command, "timestamp": time.time()})
            return {"run_id": body.run_id, "requested": command, "status": "pending"}

    @app.post("/api/run/pause")
    def pause(body: ControlRequest):
        return control(body, "pause")

    @app.post("/api/run/resume")
    def resume(body: ControlRequest):
        return control(body, "resume")

    @app.post("/api/run/stop")
    def stop(body: ControlRequest):
        return control(body, "stop")

    @app.websocket("/ws/metrics")
    async def websocket(websocket: WebSocket):
        if websocket.headers.get("origin"):
            await websocket.close(code=1008)
            return
        await websocket.accept()
        try:
            while True:
                payload = {"type": "snapshot", "timestamp": time.time(),
                           "runs": await asyncio.to_thread(list_runs), "system": await asyncio.to_thread(snapshot)}
                await websocket.send_json(payload)
                await asyncio.sleep(1)
        except (WebSocketDisconnect, RuntimeError, OSError):
            return

    return app


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--artifacts", default=str(PACKAGE_ROOT / "artifacts"))
    parser.add_argument("--port", type=int, default=8097)
    args = parser.parse_args()
    import uvicorn
    uvicorn.run(create_app(args.artifacts), host="127.0.0.1", port=args.port, timeout_graceful_shutdown=2)


if __name__ == "__main__":
    main()
