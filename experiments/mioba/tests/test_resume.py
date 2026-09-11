"""Smoke E: subprocess coordinator + worker, checkpoint, SIGKILL, resume."""
import json
import signal
import socket
import subprocess
import sys
import time
from pathlib import Path

import httpx
import pytest
import yaml

CFG = Path(__file__).resolve().parents[1] / "configs" / "smoke_mock.yaml"


def _free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


def _cli(runs_dir, *cli_args, port=None):
    cmd = [sys.executable, "-m", "experiments.mioba.cli",
           "--config", str(CFG), "--runs-dir", str(runs_dir)]
    return cmd + [str(a) for a in cli_args]


@pytest.mark.slow
def test_resume_after_kill(tmp_path):
    runs = tmp_path / "runs"
    port = _free_port()
    base = f"http://127.0.0.1:{port}"
    coord = subprocess.Popen(
        _cli(runs, "start", "--port", port), cwd=str(CFG.parents[3]))
    try:
        # wait for status
        exp_id = None
        for _ in range(60):
            try:
                exp_id = httpx.get(base + "/api/status", timeout=2
                                   ).json()["experiment_id"]
                break
            except Exception:
                if coord.poll() is not None:
                    raise AssertionError("coordinator exited early")
                time.sleep(0.5)
        assert exp_id
        worker = subprocess.Popen(
            [sys.executable, "-m", "experiments.mioba.workers.worker",
             "--coordinator", base, "--backend", "mock", "--batch", "1",
             "--worker-id", "w1", "--heartbeat-s", "1"],
            cwd=str(CFG.parents[3]))
        try:
            ok = False
            for _ in range(120):
                try:
                    evs = httpx.get(base + "/api/evaluations", timeout=2
                                    ).json()["evaluations"]
                    if evs:
                        ok = True
                        break
                except Exception:
                    pass
                time.sleep(0.5)
            assert ok, "no evaluation completed"
            r = subprocess.run(_cli(runs, "checkpoint"),
                               cwd=str(CFG.parents[3]), timeout=15)
            assert r.returncode == 0
        finally:
            worker.kill()
            worker.wait()
        coord.send_signal(signal.SIGKILL)
        coord.wait()

        port2 = _free_port()
        base2 = f"http://127.0.0.1:{port2}"
        coord2 = subprocess.Popen(
            _cli(runs, "start", "--resume", exp_id, "--port", port2),
            cwd=str(CFG.parents[3]))
        try:
            for _ in range(60):
                try:
                    s = httpx.get(base2 + "/api/status", timeout=2).json()
                    break
                except Exception:
                    time.sleep(0.5)
            assert s["experiment_id"] == exp_id
            assert s["status"] == "running"
            jobs = httpx.get(base2 + "/api/jobs",
                             params={"limit": 10000}).json()["jobs"]
            assert not [j for j in jobs if j["status"] == "RUNNING"]
            assert s["population_size"] >= 4
            genomes = httpx.get(base2 + "/api/genomes",
                                params={"limit": 10000}).json()["genomes"]
            assert len(genomes) >= 4
            evs = httpx.get(base2 + "/api/evaluations").json()["evaluations"]
            assert len(evs) >= 1
            events = httpx.get(
                base2 + "/api/events",
                params={"type": "experiment_resumed_from_checkpoint"}
            ).json()["events"]
            assert events
        finally:
            coord2.kill()
            coord2.wait()
    finally:
        if coord.poll() is None:
            coord.kill()
