"""Coordinator service: owns the DB, RNG, population controller, and the
1 Hz background loop. The GUI/CLI never own state — this process and its
DB are the truth."""
from __future__ import annotations

import json
import random
import secrets
import threading
import time
from datetime import datetime, timedelta, timezone
from pathlib import Path

from ..evolution.population import PopulationController, fitness_placeholder
from ..genome.hashing import config_hash
from ..genome.schema import utcnow
from ..mie.registry import load_collectors
from ..storage import models as M
from ..storage.db import Database
from ..telemetry.recorder import TelemetryRecorder
from . import jobs, lifecycle


class MiobaService:
    def __init__(self, config: dict, runs_dir: str | Path,
                 experiment_id: str | None = None, resume: str | None = None):
        self.config = config
        self.runs_dir = Path(runs_dir)
        exp_cfg = config.get("experiment", {})
        if resume:
            self.experiment_id = resume
        else:
            stamp = datetime.now(timezone.utc).strftime("%Y%m%d-%H%M%S")
            self.experiment_id = experiment_id or \
                f"{exp_cfg.get('id_prefix', 'M')}-{stamp}-{secrets.token_hex(3)}"
        self.run_dir = self.runs_dir / self.experiment_id
        self.db_path = self.run_dir / "lineage.sqlite"
        already = self.db_path.exists()
        if already and not resume:
            raise RuntimeError(
                f"runs dir {self.run_dir} already holds an experiment; "
                "use --resume")
        if resume and not already:
            raise RuntimeError(f"no experiment at {self.run_dir} to resume")

        self.run_dir.mkdir(parents=True, exist_ok=True)
        for sub in ("checkpoints", "logs", "telemetry", "traces", "cache",
                    "replays"):
            (self.run_dir / sub).mkdir(exist_ok=True)

        self.db = Database(self.db_path)
        self.config_hash = config_hash(config)
        from ..fba.runtime_info import collect_runtime_info
        self.runtime_info = collect_runtime_info(
            backend=config.get("evaluation", {}).get("backend"))
        self.git_commit = self.runtime_info.get("git_commit")

        seed = int(config.get("evolution", {}).get("mutation_seed", 0))
        self.rng = random.Random(seed)
        self.paused = False
        self.started_at = time.time()
        self._shutdown = threading.Event()
        self._bg_thread: threading.Thread | None = None
        self._counters = {"births": 0, "evaluations_succeeded": 0,
                          "evaluations_failed": 0}
        self.anomaly_threshold = float(
            config.get("mie", {}).get("anomaly", {}).get("gpu_temp_c", 85))

        # control token for POST /api/control/*
        tok_file = self.run_dir / "control.token"
        if tok_file.exists():
            self.token = tok_file.read_text().strip()
        else:
            self.token = secrets.token_hex(16)
            tok_file.write_text(self.token + "\n")

        self.collectors = load_collectors(config)
        self.recorder = TelemetryRecorder(self.db, self.experiment_id,
                                          self.run_dir / "telemetry")

        if already:
            exp = self.db.get_experiment(self.experiment_id)
            if exp and exp.get("rng_state_json"):
                self.load_rng_state(exp["rng_state_json"])
            if exp and exp.get("counters_json"):
                self._counters.update(json.loads(exp["counters_json"]))
            lifecycle.resume_experiment(self)
        else:
            self.db.create_experiment(self.experiment_id, config,
                                      self.config_hash, self.git_commit,
                                      self.dump_rng_state())
            self.db.set_experiment_status(self.experiment_id, M.EXP_RUNNING)
            self.db.emit(self.experiment_id, M.EV_STARTED,
                         payload={"experiment_id": self.experiment_id},
                         source="coordinator")

        self.population = PopulationController(self.db, self.experiment_id,
                                               config, self.rng)
        if not already:
            n = self.population.seed_if_empty()
            self._counters["births"] += n

    # ------------------------------------------------------------ rng / counters
    def dump_rng_state(self) -> str:
        return json.dumps(list(self.rng.getstate()[1]),
                          default=lambda o: list(o) if isinstance(o, tuple) else o)

    def load_rng_state(self, rng_state_json: str) -> None:
        try:
            state = json.loads(rng_state_json)
            self.rng.setstate((3, tuple(state), None))
        except (ValueError, TypeError):
            pass

    def counters(self) -> dict:
        return dict(self._counters)

    def persist_state(self) -> None:
        self.db.set_rng_state(self.experiment_id, self.dump_rng_state())
        self.db.set_counters(self.experiment_id, self._counters)

    # ------------------------------------------------------------ worker API
    def register_worker(self, worker_id, hostname, gpu, runtime_info,
                        bench, batch_size):
        self.db.register_worker(self.experiment_id, worker_id, hostname,
                                gpu, runtime_info, bench, batch_size)
        self.db.emit(self.experiment_id, M.EV_WORKER_JOINED,
                     payload={"worker_id": worker_id, "hostname": hostname,
                              "gpu": gpu}, source="coordinator")

    def heartbeat(self, worker_id, sample: dict, mie_samples: list[dict]):
        self.db.heartbeat(self.experiment_id, worker_id, sample)
        if mie_samples:
            self.recorder.record(mie_samples)

    def claim(self, worker_id: str, batch_size: int | None) -> dict | None:
        job = jobs.claim(self.db, self.experiment_id, worker_id, self.paused)
        if job is None:
            return None
        grow = self.db.get_genome(job["genome_id"])
        return {
            "job_id": job["job_id"],
            "genome_json": grow["genome_json"] if grow else None,
            "seed": job["seed"],
            "duration_ms": job["duration_ms"],
            "backend": job["backend"],
            "requested_traces": json.loads(job["requested_traces_json"]),
            "environment_id": job["environment_id"],
            "attempt": job["attempt"],
            "config": self.config,
            "run_dir": str(self.run_dir),
            "experiment_id": self.experiment_id,
        }

    def worker_result(self, job_id: str, worker_id: str, status: str,
                      evaluation: dict | None, error: str | None):
        job = self.db.get_job(job_id)
        if job is None:
            raise KeyError(f"no such job {job_id}")
        ok = status == M.JOB_SUCCEEDED
        jobs.finish(self.db, self.experiment_id, job, status, error)
        if ok and evaluation is not None:
            evaluation = dict(evaluation)
            evaluation["worker_id"] = worker_id
            target = float(self.config.get("evaluation", {})
                           .get("target_rate_hz", 5.0))
            evaluation["fitness"] = fitness_placeholder(
                evaluation.get("summary") or {}, target)
            evaluation.setdefault("config_hash", self.config_hash)
            evaluation.setdefault("genome_hash", job["genome_id"])
            evaluation.setdefault("git_commit", self.git_commit)
            self.db.insert_evaluation(self.experiment_id, job_id,
                                      job["genome_id"], evaluation)
            self._counters["evaluations_succeeded"] += 1
            self.db.emit(self.experiment_id, M.EV_EVALUATION_SUCCEEDED,
                         payload={"job_id": job_id,
                                  "genome_id": job["genome_id"],
                                  "worker_id": worker_id},
                         source="coordinator")
            self.population.record_fitness(job["genome_id"],
                                           evaluation["fitness"])
        else:
            self._counters["evaluations_failed"] += 1
            self.db.emit(self.experiment_id, M.EV_EVALUATION_FAILED, "warn",
                         {"job_id": job_id, "worker_id": worker_id,
                          "error": error}, "coordinator")
        self.persist_state()

    # ------------------------------------------------------------ background
    def request_shutdown(self):
        self._shutdown.set()

    @property
    def shutdown_requested(self) -> bool:
        return self._shutdown.is_set()

    def start_background(self):
        if self._bg_thread is None:
            self._bg_thread = threading.Thread(target=self._loop, daemon=True,
                                               name="mioba-bg")
            self._bg_thread.start()

    def stop_background(self):
        self._shutdown.set()
        if self._bg_thread:
            self._bg_thread.join(timeout=5)

    def _loop(self):
        last_ckpt = time.time()
        last_mie = 0.0
        ckpt_interval = float(self.config.get("checkpoint", {})
                              .get("interval_s", 600))
        mie_interval = float(self.config.get("mie", {}).get("interval_s", 5))
        while not self._shutdown.is_set():
            try:
                self._detect_stale_workers()
                for job_id, action in jobs.requeue(
                        self.db, self.experiment_id, self.config):
                    self.db.emit(self.experiment_id, M.EV_JOB_REQUEUED,
                                 payload={"job_id": job_id, "action": action},
                                 source="coordinator")
                self.population.maybe_advance()
                now = time.time()
                if now - last_mie >= mie_interval:
                    last_mie = now
                    self._collect_mie()
                if now - last_ckpt >= ckpt_interval:
                    last_ckpt = now
                    self.persist_state()
                    lifecycle.checkpoint(self, reason="periodic")
            except Exception as exc:  # keep the loop alive
                self.db.emit(self.experiment_id, "background_error", "error",
                             {"error": f"{type(exc).__name__}: {exc}"},
                             "coordinator")
            self._shutdown.wait(1.0)

    def _detect_stale_workers(self):
        lost_after = float(self.config.get("worker", {})
                           .get("lost_after_s", 90))
        cutoff = (datetime.now(timezone.utc)
                  - timedelta(seconds=lost_after)).isoformat()
        for w in self.db.list_workers(self.experiment_id):
            if w["status"] != "online":
                continue
            last = w.get("last_heartbeat_at") or ""
            if last and last < cutoff:
                self.db.mark_worker_status(self.experiment_id,
                                           w["worker_id"], "lost")
                running = self.db.jobs_for_worker(self.experiment_id,
                                                w["worker_id"])
                ids = [j["job_id"] for j in running]
                self.db.mark_running_unknown(self.experiment_id, ids)
                self.db.emit(self.experiment_id, M.EV_WORKER_LOST, "warn",
                             {"worker_id": w["worker_id"]}, "coordinator")
                if ids:
                    self.db.emit(self.experiment_id, M.EV_JOB_MARKED_UNKNOWN,
                                 "warn",
                                 {"job_ids": ids, "reason": "worker_lost"},
                                 "coordinator")

    def _collect_mie(self):
        samples = []
        for c in self.collectors:
            try:
                samples.extend(e.to_dict() for e in c.collect())
            except Exception as exc:
                samples.append({"timestamp": utcnow(), "source": "coordinator",
                                "domain": "machine_interoception",
                                "signal_type": "collector_error",
                                "value": None, "confidence": 0.0,
                                "metadata": {"collector": c.name,
                                             "error": str(exc)}})
        for s in samples:
            if (s.get("signal_type") == "temperature_c"
                    and s.get("value") is not None
                    and s["value"] > self.anomaly_threshold):
                self.db.emit(self.experiment_id, M.EV_SENSOR_ANOMALY, "warn",
                             {"source": s["source"], "value": s["value"]},
                             "mie")
        self.recorder.record(samples)

    # ------------------------------------------------------------ status
    def status(self) -> dict:
        counts = self.db.job_counts(self.experiment_id)
        exp = self.db.get_experiment(self.experiment_id) or {}
        counters = dict(self._counters)
        counters.update({
            "queued": counts.get(M.JOB_QUEUED, 0),
            "running": counts.get(M.JOB_RUNNING, 0),
            "unknown": counts.get(M.JOB_UNKNOWN, 0),
        })
        return {
            "experiment_id": self.experiment_id,
            "status": exp.get("status"),
            "uptime_s": round(time.time() - self.started_at, 1),
            "counters": counters,
            "population_size": self.db.count_genomes(self.experiment_id),
            "archive_size": len(self.db.list_evaluations(self.experiment_id,
                                                         limit=10**9)),
            "git_commit": self.git_commit,
            "config_hash": self.config_hash,
            "kind": "LIVE",
        }
