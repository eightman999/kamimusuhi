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
from ..fba.replicates import replicate_seeds
from ..genome.hashing import (config_hash, runtime_config_hash,
                              scientific_config_hash)
from ..genome.schema import utcnow
from ..mie.registry import load_collectors
from ..storage import models as M
from ..storage.db import Database, InvalidTransition
from ..telemetry.recorder import TelemetryRecorder
from . import jobs, lifecycle


class ScientificConfigMismatch(RuntimeError):
    """Resume was attempted with a config whose result-affecting sections
    differ from the ones the experiment was created with. There is no
    override: a scientific change is a new experiment."""


DEFAULT_BATCH_CANDIDATES = (1, 2, 4, 8, 16, 32)
DEFAULT_VRAM_HEADROOM = 0.85


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
        self.scientific_config_hash = scientific_config_hash(config)
        self.runtime_config_hash = runtime_config_hash(config)
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
        self._stored_config_hash = None
        self._stored_scientific_hash = None
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
            exp = self.db.get_experiment(self.experiment_id) or {}
            self._stored_config_hash = exp.get("config_hash")
            self._stored_scientific_hash = exp.get("scientific_config_hash")
            self._check_config_on_resume(exp)
            if exp and exp.get("rng_state_json"):
                self.load_rng_state(exp["rng_state_json"])
            if exp and exp.get("counters_json"):
                self._counters.update(json.loads(exp["counters_json"]))
            lifecycle.resume_experiment(self)
        else:
            self.db.create_experiment(
                self.experiment_id, config, self.config_hash, self.git_commit,
                self.dump_rng_state(),
                scientific_config_hash=self.scientific_config_hash,
                runtime_config_hash=self.runtime_config_hash)
            self.db.set_experiment_status(self.experiment_id, M.EXP_RUNNING)
            self.db.emit(self.experiment_id, M.EV_STARTED,
                         payload={"experiment_id": self.experiment_id},
                         source="coordinator")

        self.population = PopulationController(self.db, self.experiment_id,
                                               config, self.rng,
                                               persist=self._on_generation)
        if not already:
            self.population.seed_if_empty()

    # ------------------------------------------------------------ helpers
    def _on_generation(self, born: int) -> None:
        """Runs inside the population's generation transaction: births
        counter + post-mutation RNG state commit with the children."""
        self._counters["births"] += born
        self.persist_state()

    def _check_config_on_resume(self, exp: dict):
        """Operational (runtime) changes are recorded and allowed;
        a scientific change refuses the resume (new experiment id)."""
        stored_sci = exp.get("scientific_config_hash")
        stored_run = exp.get("runtime_config_hash")
        stored_all = exp.get("config_hash")
        if stored_all and stored_all != self.config_hash:
            self.db.emit(self.experiment_id, "config_hash_mismatch", "warn",
                         {"stored": stored_all, "current": self.config_hash},
                         "coordinator")
        if stored_run and stored_run != self.runtime_config_hash:
            self.db.emit(self.experiment_id, M.EV_RUNTIME_CONFIG_CHANGED,
                         "info", {"stored": stored_run,
                                  "current": self.runtime_config_hash},
                         "coordinator")
        if stored_sci and stored_sci != self.scientific_config_hash:
            payload = {"stored": stored_sci,
                       "current": self.scientific_config_hash}
            self.db.emit(self.experiment_id, M.EV_SCIENTIFIC_CONFIG_MISMATCH,
                         "error", payload, "coordinator")
            self.db.close()
            raise ScientificConfigMismatch(
                f"experiment {self.experiment_id} was created with "
                f"scientific_config_hash {stored_sci[:12]} but the "
                f"current config hashes to "
                f"{self.scientific_config_hash[:12]}; scientific conditions "
                "are fixed per experiment - start a new experiment id")

    def fba_base_neurons(self) -> int | None:
        """FBA0 neuron count to use for ancestry_fraction: the synthetic-N
        when the backend runs synthetic, else the real data count."""
        fba = self.config.get("fba", {})
        if fba.get("synthetic"):
            return int(fba.get("synthetic_neurons", 2000))
        return None

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
    def worker_profile(self) -> dict:
        """Execution profile a worker must benchmark against: the exact
        backend / dataset / network size / duration / replicates that its
        jobs will use (plus the operational batch policy)."""
        ev = self.config.get("evaluation", {})
        fba = dict(self.config.get("fba", {}))
        wk = self.config.get("worker", {})
        return {
            "experiment_id": self.experiment_id,
            "scientific_config_hash": self.scientific_config_hash,
            "backend": ev.get("backend", "mock"),
            "fba": fba,
            "env": self.config.get("env", {}),
            "evaluation": {
                "replicates": int(ev.get("replicates", 1)),
                "duration_ms": float(ev.get("duration_ms", 500)),
                "environment_id": ev.get("environment_id",
                                         "synthetic-quiet-v0"),
                "tier": ev.get("tier", "smoke"),
            },
            "worker": {
                "execution_batch": wk.get("execution_batch",
                                          wk.get("batch_size", "auto")),
                "candidates": list(wk.get("candidates",
                                          DEFAULT_BATCH_CANDIDATES)),
                "vram_headroom": float(wk.get("vram_headroom",
                                              DEFAULT_VRAM_HEADROOM)),
                "bench_duration_ms": float(wk.get("bench_duration_ms",
                                                  ev.get("duration_ms", 500))),
                "bench_organ_overhead_neurons": int(
                    wk.get("bench_organ_overhead_neurons", 0)),
                # concurrent evaluations per worker process (M1 §2.2)
                "slots": int(wk.get("slots", 1)),
            },
        }

    def register_worker(self, worker_id, hostname, gpu, runtime_info,
                        bench, batch_size, device=None):
        self.db.register_worker(self.experiment_id, worker_id, hostname,
                                gpu, runtime_info, bench, batch_size,
                                device=device)
        self.db.emit(self.experiment_id, M.EV_WORKER_JOINED,
                     payload={"worker_id": worker_id, "hostname": hostname,
                              "device": device, "gpu": gpu,
                              "execution_batch": batch_size},
                     source="coordinator")

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
            "replicates": int(job.get("replicates") or 1),
            "replicate_seeds": replicate_seeds(job["seed"],
                                               int(job.get("replicates") or 1)),
            "config": self.config,
            "run_dir": str(self.run_dir),
            "experiment_id": self.experiment_id,
        }

    def _complete_evaluation(self, job: dict, worker_id: str,
                             evaluation: dict) -> dict:
        evaluation = dict(evaluation)
        evaluation["worker_id"] = worker_id
        target = float(self.config.get("evaluation", {})
                       .get("target_rate_hz", 5.0))
        evaluation["fitness"] = fitness_placeholder(
            evaluation.get("summary") or {}, target)
        if not evaluation.get("runtime_info"):
            wr = self.db.get_worker(self.experiment_id, worker_id) or {}
            try:
                ri = json.loads(wr.get("runtime_info_json") or "{}")
            except ValueError:
                ri = {}
            try:
                ri["gpu"] = json.loads(wr.get("gpu_json") or "[]")
            except ValueError:
                ri["gpu"] = []
            evaluation["runtime_info"] = ri
        evaluation.setdefault("config_hash", self.config_hash)
        evaluation.setdefault("scientific_config_hash",
                              self.scientific_config_hash)
        evaluation.setdefault("genome_hash", job["genome_id"])
        evaluation.setdefault("git_commit", self.git_commit)
        evaluation.setdefault("environment_id", job["environment_id"])
        evaluation.setdefault("duration_ms", job["duration_ms"])
        evaluation.setdefault("backend", job["backend"])
        evaluation.setdefault("seed", job["seed"])
        return evaluation

    def worker_result(self, job_id: str, worker_id: str, status: str,
                      evaluation: dict | None, error: str | None,
                      result_id: str | None = None) -> dict:
        """Accept a worker's result. The success path (ownership check,
        RUNNING->SUCCEEDED, evaluation row, worker counter, counters +
        RNG state, event) is one SQLite transaction: a crash or exception
        anywhere leaves the job RUNNING (recoverable), never
        SUCCEEDED-without-evaluation.

        Idempotent: ``result_id`` is the worker's deterministic id for this
        (job, worker, attempt). A re-delivery of an already committed
        result is acknowledged with ``duplicate=True`` and changes nothing;
        a result for a job that is no longer RUNNING under this worker
        (reclaimed after UNKNOWN) raises InvalidTransition (HTTP 409)."""
        job = self.db.get_job(job_id)
        if job is None:
            raise KeyError(f"no such job {job_id}")
        if result_id and job.get("result_id") == result_id:
            return {"ok": True, "duplicate": True, "job_status": job["status"]}
        if status == "RETRY":
            with self.db.transaction():
                self.db.release_job_for_retry(job_id, worker_id, error,
                                              result_id=result_id)
                self.db.emit(
                    self.experiment_id, M.EV_JOB_MARKED_UNKNOWN, "warn",
                    {"job_id": job_id, "worker_id": worker_id,
                     "reason": "retryable_infrastructure_failure",
                     "error": error, "result_id": result_id},
                    "coordinator")
            return {"ok": True, "duplicate": False,
                    "job_status": M.JOB_UNKNOWN, "retryable": True}
        ok = status == M.JOB_SUCCEEDED and evaluation is not None
        if status == M.JOB_SUCCEEDED and evaluation is None:
            status, error = M.JOB_FAILED, error or "success without evaluation"
        before = dict(self._counters)
        try:
            with self.db.transaction():
                jobs.finish(self.db, self.experiment_id, job, status,
                            worker_id, error, result_id=result_id)
                if ok:
                    evaluation = self._complete_evaluation(job, worker_id,
                                                           evaluation)
                    evaluation["result_id"] = result_id
                    self.db.insert_evaluation(self.experiment_id, job_id,
                                              job["genome_id"], evaluation)
                    self.db.emit(self.experiment_id, M.EV_EVALUATION_SUCCEEDED,
                                 payload={"job_id": job_id,
                                          "genome_id": job["genome_id"],
                                          "worker_id": worker_id},
                                 source="coordinator")
                else:
                    self.db.emit(self.experiment_id, M.EV_EVALUATION_FAILED,
                                 "warn", {"job_id": job_id,
                                          "worker_id": worker_id,
                                          "error": error}, "coordinator")
                self._counters["evaluations_succeeded" if ok
                               else "evaluations_failed"] += 1
                self.persist_state()
        except InvalidTransition:
            self._counters = before
            self.db.emit(self.experiment_id, "stale_result_rejected", "warn",
                         {"job_id": job_id, "worker_id": worker_id,
                          "status": status, "result_id": result_id},
                         "coordinator")
            raise
        except BaseException:
            self._counters = before
            raise
        if ok:
            self.population.record_fitness(job["genome_id"],
                                           evaluation["fitness"])
        return {"ok": True, "duplicate": False, "job_status": status,
                "evaluation_id": (evaluation or {}).get("evaluation_id")
                if ok else None}

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
            "archive_size": self.db.count_evaluations(self.experiment_id),
            "git_commit": self.git_commit,
            "config_hash": self.config_hash,
            "config_hash_stored": (self._stored_config_hash or
                                   self.config_hash),
            "scientific_config_hash": self.scientific_config_hash,
            "runtime_config_hash": self.runtime_config_hash,
            "kind": "LIVE",
        }
