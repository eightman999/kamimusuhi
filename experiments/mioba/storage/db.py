"""SQLite lineage store. Single writer = the coordinator process."""
from __future__ import annotations

import hashlib
import json
from contextlib import contextmanager
import sqlite3
import threading
import uuid
from pathlib import Path

from . import models as M

SCHEMA_VERSION = 4
_SCHEMA_SQL = (Path(__file__).parent / "schema.sql").read_text()

# columns added after v2; applied with ALTER TABLE when opening an older DB.
# Adding columns only: an older DB keeps every row it had, and rows written
# before a column existed read back as NULL rather than being rewritten.
_ADDED_COLUMNS = (
    ("evaluation_jobs", "replicates", "INTEGER NOT NULL DEFAULT 1"),
    ("evaluation_jobs", "result_id", "TEXT"),
    ("evaluations", "requested_replicates", "INTEGER"),
    ("evaluations", "completed_replicates", "INTEGER"),
    ("evaluations", "execution_batch_size", "INTEGER"),
    ("evaluations", "replicate_seeds_json", "TEXT NOT NULL DEFAULT '[]'"),
    ("evaluations", "result_id", "TEXT"),
    ("worker_runs", "device", "TEXT"),
    # v4
    ("evaluations", "simulator_semantics_version", "INTEGER"),
    ("evaluations", "rng_protocol_version", "INTEGER"),
    ("evaluations", "propagation_backend", "TEXT"),
    ("evaluations", "selection_score", "REAL"),
    ("evaluations", "metrics_json", "TEXT NOT NULL DEFAULT '{}'"),
    ("evaluations", "resource_json", "TEXT NOT NULL DEFAULT '{}'"),
    ("evaluations", "activity_json", "TEXT NOT NULL DEFAULT '{}'"),
    ("worker_runs", "slots", "INTEGER"),
)


def _uid(prefix: str) -> str:
    return f"{prefix}_{uuid.uuid4().hex[:12]}"


class InvalidTransition(RuntimeError):
    pass


class Database:
    def __init__(self, path: str | Path):
        self.path = Path(path)
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self._lock = threading.RLock()
        self._tx_depth = 0
        self.conn = sqlite3.connect(str(self.path), check_same_thread=False,
                                    timeout=30)
        self.conn.row_factory = sqlite3.Row
        with self._lock:
            self.conn.execute("PRAGMA journal_mode=WAL")
            self.conn.execute("PRAGMA busy_timeout=30000")
            self.conn.execute("PRAGMA foreign_keys=ON")
            self.conn.executescript(_SCHEMA_SQL)
            row = self.conn.execute("SELECT version FROM schema_version").fetchone()
            if row is None:
                self.conn.execute("INSERT INTO schema_version(version) VALUES (?)",
                                  (SCHEMA_VERSION,))
            elif row["version"] < SCHEMA_VERSION:
                self._migrate(row["version"])
            self._commit()

    def _migrate(self, from_version: int) -> None:
        for table, col, decl in _ADDED_COLUMNS:
            have = {r["name"] for r in
                    self.conn.execute(f"PRAGMA table_info({table})")}
            if col not in have:
                self.conn.execute(f"ALTER TABLE {table} ADD COLUMN {col} {decl}")
        self.conn.execute("UPDATE schema_version SET version=?",
                          (SCHEMA_VERSION,))

    def close(self):
        with self._lock:
            self.conn.close()

    # ------------------------------------------------------------ util
    def _q(self, sql, params=()):
        return self.conn.execute(sql, params)

    def _commit(self) -> None:
        if self._tx_depth == 0:
            self.conn.commit()

    @contextmanager
    def transaction(self):
        """Group several write methods into one SQLite transaction. Nested
        method-level commits are suppressed; the outermost block commits,
        any exception rolls everything back."""
        with self._lock:
            self._tx_depth += 1
            try:
                yield self
            except BaseException:
                self._tx_depth -= 1
                if self._tx_depth == 0:
                    self.conn.rollback()
                raise
            else:
                self._tx_depth -= 1
                if self._tx_depth == 0:
                    self.conn.commit()

    @staticmethod
    def _now() -> str:
        from ..genome.schema import utcnow
        return utcnow()

    # ------------------------------------------------------------ experiments
    def create_experiment(self, experiment_id: str, config: dict,
                          config_hash: str, git_commit: str | None,
                          rng_state: str | None = None,
                          scientific_config_hash: str | None = None,
                          runtime_config_hash: str | None = None) -> None:
        with self._lock:
            self._q("INSERT INTO experiments(experiment_id,created_at,config_json,"
                    "config_hash,scientific_config_hash,runtime_config_hash,"
                    "git_commit,status,rng_state_json,counters_json)"
                    " VALUES(?,?,?,?,?,?,?,?,?,?)",
                    (experiment_id, self._now(), json.dumps(config), config_hash,
                     scientific_config_hash, runtime_config_hash,
                     git_commit, M.EXP_CREATED, rng_state, "{}"))
            self._commit()

    def get_experiment(self, experiment_id: str) -> dict | None:
        with self._lock:
            row = self._q("SELECT * FROM experiments WHERE experiment_id=?",
                          (experiment_id,)).fetchone()
        return dict(row) if row else None

    def set_experiment_status(self, experiment_id: str, status: str) -> None:
        with self._lock:
            self._q("UPDATE experiments SET status=? WHERE experiment_id=?",
                    (status, experiment_id))
            self._commit()

    def set_rng_state(self, experiment_id: str, rng_state_json: str) -> None:
        with self._lock:
            self._q("UPDATE experiments SET rng_state_json=? WHERE experiment_id=?",
                    (rng_state_json, experiment_id))
            self._commit()

    def set_counters(self, experiment_id: str, counters: dict) -> None:
        with self._lock:
            self._q("UPDATE experiments SET counters_json=? WHERE experiment_id=?",
                    (json.dumps(counters), experiment_id))
            self._commit()

    # ------------------------------------------------------------ genomes
    def insert_genome(self, experiment_id: str, genome, birth_kind: str,
                      clade_id: str | None = None,
                      default_clade_id: str | None = None) -> str:
        """Insert genome + parents + birth + mutation rows. Returns genome_id."""
        g = genome
        gj = g.to_json()
        with self._lock:
            self._q("INSERT OR IGNORE INTO genomes(experiment_id,genome_id,"
                    "parent_ids_json,species_base,generation,birth_index,"
                    "random_seed,genome_json,content_hash,created_at)"
                    " VALUES(?,?,?,?,?,?,?,?,?,?)",
                    (experiment_id, g.genome_id, json.dumps(g.parent_ids),
                     g.species_base, g.generation, g.birth_index,
                     g.random_seed, gj, g.genome_id, g.created_at))
            for pid in g.parent_ids:
                self._q("INSERT OR IGNORE INTO parents(experiment_id,child_id,"
                        "parent_id) VALUES(?,?,?)",
                        (experiment_id, g.genome_id, pid))
            mutation_ids = []
            for pm in g.parameter_mutations:
                mutation_ids.append(pm.mutation_id)
                self._q("INSERT OR IGNORE INTO mutations(experiment_id,"
                        "mutation_id,genome_id,kind,path,op,value,scope,"
                        "provenance_json) VALUES(?,?,?,?,?,?,?,?,?)",
                        (experiment_id, pm.mutation_id, g.genome_id,
                         "parameter", pm.path, pm.op, pm.value, pm.scope,
                         "{}"))
            for organ in g.artificial_organs:
                if organ.provenance.birth_mutation_id:
                    mutation_ids.append(organ.provenance.birth_mutation_id)
                    self._q("INSERT OR IGNORE INTO mutations(experiment_id,"
                            "mutation_id,genome_id,kind,path,op,value,scope,"
                            "provenance_json) VALUES(?,?,?,?,?,?,?,?,?)",
                            (experiment_id, organ.provenance.birth_mutation_id,
                             g.genome_id, "organ", organ.organ_id, "add",
                             float(organ.size), f"organ:{organ.organ_id}",
                             json.dumps(organ.provenance.__dict__)))
            self._q("INSERT INTO births(experiment_id,birth_id,genome_id,"
                    "birth_index,generation,mutation_ids_json,created_at,kind)"
                    " VALUES(?,?,?,?,?,?,?,?)",
                    (experiment_id, _uid("birth"), g.genome_id, g.birth_index,
                     g.generation, json.dumps(mutation_ids), self._now(),
                     birth_kind))
            cid = clade_id or default_clade_id
            if cid:
                self._q("INSERT OR IGNORE INTO genome_clades(experiment_id,"
                        "genome_id,clade_id) VALUES(?,?,?)",
                        (experiment_id, g.genome_id, cid))
            self._commit()
        return g.genome_id

    def get_genome(self, genome_id: str) -> dict | None:
        with self._lock:
            row = self._q("SELECT * FROM genomes WHERE genome_id=?",
                          (genome_id,)).fetchone()
        return dict(row) if row else None

    def list_genomes(self, experiment_id: str, limit=100, offset=0) -> list[dict]:
        with self._lock:
            rows = self._q("SELECT * FROM genomes WHERE experiment_id=?"
                           " ORDER BY generation,birth_index LIMIT ? OFFSET ?",
                           (experiment_id, limit, offset)).fetchall()
        return [dict(r) for r in rows]

    def genome_children(self, genome_id: str) -> list[str]:
        with self._lock:
            rows = self._q("SELECT child_id FROM parents WHERE parent_id=?",
                           (genome_id,)).fetchall()
        return [r["child_id"] for r in rows]

    def genome_parents(self, genome_id: str) -> list[str]:
        with self._lock:
            rows = self._q("SELECT parent_id FROM parents WHERE child_id=?",
                           (genome_id,)).fetchall()
        return [r["parent_id"] for r in rows]

    def genome_mutations(self, genome_id: str) -> list[dict]:
        with self._lock:
            rows = self._q("SELECT * FROM mutations WHERE genome_id=?",
                           (genome_id,)).fetchall()
        return [dict(r) for r in rows]

    def genome_clades(self, genome_id: str) -> list[str]:
        with self._lock:
            rows = self._q("SELECT clade_id FROM genome_clades WHERE genome_id=?",
                           (genome_id,)).fetchall()
        return [r["clade_id"] for r in rows]

    def ancestry(self, genome_id: str) -> list[str]:
        """Walk parents to the FBA0 root; returns ids root-first."""
        seen, order = set(), []

        def walk(gid):
            if gid in seen:
                return
            seen.add(gid)
            for pid in self.genome_parents(gid):
                walk(pid)
            order.append(gid)

        walk(genome_id)
        return order

    def count_genomes(self, experiment_id: str) -> int:
        with self._lock:
            return self._q("SELECT COUNT(*) c FROM genomes WHERE experiment_id=?",
                           (experiment_id,)).fetchone()["c"]

    def count_evaluations(self, experiment_id: str) -> int:
        with self._lock:
            return self._q("SELECT COUNT(*) c FROM evaluations WHERE "
                           "experiment_id=?",
                           (experiment_id,)).fetchone()["c"]

    def list_genomes_view(self, experiment_id: str, limit=100,
                          offset=0, base_neurons: int | None = None
                          ) -> list[dict]:
        """GUI list view: one row per genome with clade, organ count,
        ancestry fraction, best fitness and children count.

        `base_neurons`: synthetic-N override for ancestry_fraction (see
        develop())."""
        from ..development.phenotype import develop
        from ..genome.schema import Genome
        rows = self.list_genomes(experiment_id, limit, offset)
        out = []
        with self._lock:
            for r in rows:
                gid = r["genome_id"]
                genome = Genome.from_json(r["genome_json"])
                clade = self._q(
                    "SELECT c.clade_id, c.name FROM genome_clades gc JOIN "
                    "clades c ON c.clade_id=gc.clade_id WHERE gc.genome_id=? "
                    "LIMIT 1", (gid,)).fetchone()
                fit = self._q("SELECT MAX(fitness) f FROM evaluations WHERE "
                              "experiment_id=? AND genome_id=?",
                              (experiment_id, gid)).fetchone()["f"]
                children = self._q("SELECT COUNT(*) c FROM parents WHERE "
                                   "parent_id=?", (gid,)).fetchone()["c"]
                out.append({
                    "genome_id": gid,
                    "parent_ids": json.loads(r["parent_ids_json"]),
                    "generation": r["generation"],
                    "birth_index": r["birth_index"],
                    "species_base": r["species_base"],
                    "clade_id": clade["clade_id"] if clade else None,
                    "clade_name": clade["name"] if clade else None,
                    "organ_count": len(genome.artificial_organs),
                    "mutation_count": len(genome.parameter_mutations),
                    "ancestry_fraction":
                        develop(genome, base_neurons)["ancestry_fraction"],
                    "best_fitness": fit,
                    "children_count": children,
                    "created_at": r["created_at"],
                })
        return out

    def children_of(self, genome_id: str) -> list[str]:
        return self.genome_children(genome_id)

    def ancestry_chain(self, genome_id: str) -> list[dict]:
        """Walk first-parent links to the FBA0 root; returns root-first
        list of {genome_id, generation, birth_index}."""
        chain, seen = [], set()
        gid = genome_id
        while gid and gid not in seen:
            seen.add(gid)
            row = self.get_genome(gid)
            if row is None:
                break
            chain.append({"genome_id": gid, "generation": row["generation"],
                          "birth_index": row["birth_index"]})
            parents = self.genome_parents(gid)
            gid = parents[0] if parents else None
        return list(reversed(chain))

    # ------------------------------------------------------------ clades
    def create_clade(self, experiment_id: str, name: str,
                     founder_genome_id: str | None) -> str:
        cid = _uid("clade")
        with self._lock:
            self._q("INSERT INTO clades(experiment_id,clade_id,name,"
                    "founder_genome_id,created_at) VALUES(?,?,?,?,?)",
                    (experiment_id, cid, name, founder_genome_id, self._now()))
            self._commit()
        return cid

    def get_clade_by_name(self, experiment_id: str, name: str) -> dict | None:
        with self._lock:
            row = self._q("SELECT * FROM clades WHERE experiment_id=? AND name=?",
                          (experiment_id, name)).fetchone()
        return dict(row) if row else None

    def assign_clade(self, experiment_id: str, genome_id: str,
                     clade_id: str) -> None:
        with self._lock:
            self._q("INSERT OR IGNORE INTO genome_clades(experiment_id,"
                    "genome_id,clade_id) VALUES(?,?,?)",
                    (experiment_id, genome_id, clade_id))
            self._commit()

    # ------------------------------------------------------------ jobs
    def enqueue_job(self, experiment_id: str, genome_id: str, environment_id: str,
                    seed: int, tier: str, backend: str, duration_ms: float,
                    requested_traces: list[str], priority: int = 0,
                    job_id: str | None = None, replicates: int = 1) -> str:
        jid = job_id or _uid("job")
        with self._lock:
            self._q("INSERT INTO evaluation_jobs(experiment_id,job_id,genome_id,"
                    "environment_id,seed,evaluation_tier,backend,duration_ms,"
                    "requested_traces_json,status,priority,created_at,attempt,"
                    "replicates)"
                    " VALUES(?,?,?,?,?,?,?,?,?,'QUEUED',?,?,0,?)",
                    (experiment_id, jid, genome_id, environment_id, seed, tier,
                     backend, duration_ms, json.dumps(requested_traces),
                     priority, self._now(), int(replicates)))
            self._commit()
        return jid

    def claim_job(self, experiment_id: str, worker_id: str) -> dict | None:
        """Atomically claim the highest-priority QUEUED job."""
        with self._lock:
            row = self._q("SELECT job_id FROM evaluation_jobs WHERE "
                          "experiment_id=? AND status='QUEUED' AND "
                          "(last_error IS NULL OR last_error NOT LIKE "
                          "'retryable:%' OR claimed_by_worker IS NULL OR "
                          "claimed_by_worker<>?) "
                          "ORDER BY priority DESC, created_at LIMIT 1",
                          (experiment_id, worker_id)).fetchone()
            if row is None:
                return None
            cur = self._q("UPDATE evaluation_jobs SET status='RUNNING',"
                          " claimed_at=?, claimed_by_worker=? "
                          "WHERE job_id=? AND status='QUEUED'",
                          (self._now(), worker_id, row["job_id"]))
            if cur.rowcount != 1:
                self.conn.rollback()
                return None
            self._commit()
            return self.get_job(row["job_id"])

    def get_job(self, job_id: str) -> dict | None:
        with self._lock:
            row = self._q("SELECT * FROM evaluation_jobs WHERE job_id=?",
                          (job_id,)).fetchone()
        return dict(row) if row else None

    def list_jobs(self, experiment_id: str, status: str | None = None,
                  limit: int = 100) -> list[dict]:
        with self._lock:
            if status:
                rows = self._q("SELECT * FROM evaluation_jobs WHERE "
                               "experiment_id=? AND status=? ORDER BY created_at "
                               "DESC LIMIT ?", (experiment_id, status, limit)).fetchall()
            else:
                rows = self._q("SELECT * FROM evaluation_jobs WHERE "
                               "experiment_id=? ORDER BY created_at DESC LIMIT ?",
                               (experiment_id, limit)).fetchall()
        return [dict(r) for r in rows]

    def finish_job(self, job_id: str, status: str, worker_id: str,
                   error: str | None = None,
                   result_id: str | None = None) -> None:
        if status not in (M.JOB_SUCCEEDED, M.JOB_FAILED):
            raise InvalidTransition(f"finish_job: bad status {status}")
        with self._lock:
            job = self.get_job(job_id)
            if job is None:
                raise InvalidTransition("no such job")
            if job["status"] != M.JOB_RUNNING:
                raise InvalidTransition(
                    f"job {job_id} is {job['status']}, expected RUNNING")
            if job["claimed_by_worker"] != worker_id:
                raise InvalidTransition(
                    f"job {job_id} claimed by {job['claimed_by_worker']}, "
                    f"not {worker_id}")
            self._q("UPDATE evaluation_jobs SET status=?, finished_at=?,"
                    " last_error=?, result_id=? WHERE job_id=?",
                    (status, self._now(), error, result_id, job_id))
            self._commit()

    def release_job_for_retry(self, job_id: str, worker_id: str,
                              error: str | None = None,
                              result_id: str | None = None,
                              allow_same_worker: bool = False) -> None:
        """RUNNING -> UNKNOWN for an infrastructure/resource failure.

        ``claimed_by_worker`` is deliberately retained while UNKNOWN/QUEUED
        so the same worker does not immediately reclaim a job it just proved
        unable to execute.  A different worker may claim it.

        ``allow_same_worker`` clears that hold. It is used for device
        resource limits (M1 2.4-7): the worker has already narrowed its
        concurrency, so retrying there is a *different* configuration, not
        a repeat of the one that failed — and on a single-worker run
        holding the job back would strand it forever.
        """
        with self._lock:
            job = self.get_job(job_id)
            if job is None:
                raise InvalidTransition("no such job")
            if job["status"] != M.JOB_RUNNING:
                raise InvalidTransition(
                    f"job {job_id} is {job['status']}, expected RUNNING")
            if job["claimed_by_worker"] != worker_id:
                raise InvalidTransition(
                    f"job {job_id} claimed by {job['claimed_by_worker']}, "
                    f"not {worker_id}")
            detail = f"retryable:{worker_id}:{error or 'infrastructure failure'}"
            if allow_same_worker:
                self._q("UPDATE evaluation_jobs SET status='UNKNOWN', "
                        "last_error=?, result_id=?, claimed_by_worker=NULL "
                        "WHERE job_id=?", (detail, result_id, job_id))
            else:
                self._q("UPDATE evaluation_jobs SET status='UNKNOWN', "
                        "last_error=?, result_id=? WHERE job_id=?",
                        (detail, result_id, job_id))
            self._commit()

    def cancel_job(self, job_id: str) -> None:
        with self._lock:
            job = self.get_job(job_id)
            if job is None:
                raise InvalidTransition("no such job")
            if job["status"] in M.JOB_TERMINAL:
                raise InvalidTransition(f"job {job_id} already {job['status']}")
            self._q("UPDATE evaluation_jobs SET status='CANCELLED',"
                    " finished_at=? WHERE job_id=?",
                    (self._now(), job_id))
            self._commit()

    def mark_running_unknown(self, experiment_id: str,
                             job_ids: list[str] | None = None) -> list[str]:
        """RUNNING -> UNKNOWN (coordinator restart or stale worker)."""
        with self._lock:
            if job_ids is None:
                rows = self._q("SELECT job_id FROM evaluation_jobs WHERE "
                               "experiment_id=? AND status='RUNNING'",
                               (experiment_id,)).fetchall()
            else:
                marks = ",".join("?" * len(job_ids)) or "''"
                rows = self._q(f"SELECT job_id FROM evaluation_jobs WHERE "
                               f"experiment_id=? AND status='RUNNING' AND "
                               f"job_id IN ({marks})",
                               (experiment_id, *job_ids)).fetchall()
            ids = [r["job_id"] for r in rows]
            for jid in ids:
                self._q("UPDATE evaluation_jobs SET status='UNKNOWN' WHERE "
                        "job_id=? AND status='RUNNING'", (jid,))
            self._commit()
        return ids

    def requeue_unknown(self, experiment_id: str,
                        max_attempts: int) -> list[tuple[str, str]]:
        """UNKNOWN -> QUEUED (attempt+1) or FAILED at max_attempts.

        Returns [(job_id, 'requeued'|'failed'), ...]."""
        out = []
        with self._lock:
            rows = self._q("SELECT job_id,attempt,last_error FROM "
                           "evaluation_jobs WHERE experiment_id=? AND "
                           "status='UNKNOWN'",
                           (experiment_id,)).fetchall()
            for r in rows:
                retryable = str(r["last_error"] or "").startswith("retryable:")
                if not retryable and r["attempt"] + 1 >= max_attempts:
                    self._q("UPDATE evaluation_jobs SET status='FAILED',"
                            " finished_at=?, last_error='max_attempts' "
                            "WHERE job_id=?", (self._now(), r["job_id"]))
                    out.append((r["job_id"], "failed"))
                elif retryable:
                    # Keep claimed_by_worker as an avoid-worker hint.  This
                    # job remains scientifically unresolved until another
                    # worker can execute it; do not convert infra trouble to
                    # selection fitness.
                    self._q("UPDATE evaluation_jobs SET status='QUEUED',"
                            " attempt=attempt+1, claimed_at=NULL WHERE job_id=?",
                            (r["job_id"],))
                    out.append((r["job_id"], "requeued"))
                else:
                    self._q("UPDATE evaluation_jobs SET status='QUEUED',"
                            " attempt=attempt+1, claimed_at=NULL,"
                            " claimed_by_worker=NULL WHERE job_id=?",
                            (r["job_id"],))
                    out.append((r["job_id"], "requeued"))
            self._commit()
        return out

    def job_counts(self, experiment_id: str) -> dict:
        with self._lock:
            rows = self._q("SELECT status, COUNT(*) c FROM evaluation_jobs "
                           "WHERE experiment_id=? GROUP BY status",
                           (experiment_id,)).fetchall()
        return {r["status"]: r["c"] for r in rows}

    def jobs_for_worker(self, experiment_id: str, worker_id: str) -> list[dict]:
        with self._lock:
            rows = self._q("SELECT job_id FROM evaluation_jobs WHERE "
                           "experiment_id=? AND claimed_by_worker=? AND "
                           "status='RUNNING'", (experiment_id, worker_id)).fetchall()
        return [dict(r) for r in rows]

    # ------------------------------------------------------------ evaluations
    def insert_evaluation(self, experiment_id: str, job_id: str,
                          genome_id: str, evaluation: dict) -> str:
        eid = evaluation.get("evaluation_id") or _uid("eval")
        sem = evaluation.get("semantics") or {}
        with self._lock:
            self._q("INSERT INTO evaluations(experiment_id,evaluation_id,job_id,"
                    "genome_id,worker_id,backend,seed,batch_size,fitness,"
                    "summary_json,runtime_info_json,config_hash,"
                    "scientific_config_hash,genome_hash,"
                    "git_commit,started_at,finished_at,trace_path,"
                    "environment_id,duration_ms,dataset_json,device,"
                    "requested_replicates,completed_replicates,"
                    "execution_batch_size,replicate_seeds_json,result_id,"
                    "simulator_semantics_version,rng_protocol_version,"
                    "propagation_backend,selection_score,metrics_json,"
                    "resource_json,activity_json)"
                    " VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,"
                    "?,?,?,?,?,?,?,?,?,?,?,?)",
                    (experiment_id, eid, job_id, genome_id,
                     evaluation.get("worker_id"), evaluation.get("backend"),
                     evaluation.get("seed"), evaluation.get("batch_size"),
                     evaluation.get("fitness"),
                     json.dumps(evaluation.get("summary") or {}),
                     json.dumps(evaluation.get("runtime_info") or {}),
                     evaluation.get("config_hash"),
                     evaluation.get("scientific_config_hash"),
                     evaluation.get("genome_hash"),
                     evaluation.get("git_commit"),
                     evaluation.get("started_at"), evaluation.get("finished_at"),
                     evaluation.get("trace_path"),
                     evaluation.get("environment_id"),
                     evaluation.get("duration_ms"),
                     json.dumps(evaluation.get("dataset") or {}),
                     evaluation.get("device"),
                     evaluation.get("requested_replicates"),
                     evaluation.get("completed_replicates"),
                     evaluation.get("execution_batch_size"),
                     json.dumps(evaluation.get("replicate_seeds") or []),
                     evaluation.get("result_id"),
                     sem.get("simulator_semantics_version"),
                     sem.get("rng_protocol_version"),
                     sem.get("propagation_backend"),
                     evaluation.get("selection_score"),
                     json.dumps(evaluation.get("metrics") or {}),
                     json.dumps(evaluation.get("resource") or {}),
                     json.dumps(evaluation.get("activity") or {})))
            self._commit()
        return eid

    def get_evaluation(self, evaluation_id: str) -> dict | None:
        with self._lock:
            row = self._q("SELECT * FROM evaluations WHERE evaluation_id=?",
                          (evaluation_id,)).fetchone()
        return dict(row) if row else None

    def list_evaluations(self, experiment_id: str,
                         genome_id: str | None = None,
                         limit: int = 200) -> list[dict]:
        with self._lock:
            if genome_id:
                rows = self._q("SELECT * FROM evaluations WHERE experiment_id=?"
                               " AND genome_id=? ORDER BY finished_at DESC LIMIT ?",
                               (experiment_id, genome_id, limit)).fetchall()
            else:
                rows = self._q("SELECT * FROM evaluations WHERE experiment_id=?"
                               " ORDER BY finished_at DESC LIMIT ?",
                               (experiment_id, limit)).fetchall()
        return [dict(r) for r in rows]

    # ------------------------------------------------------------ workers
    def register_worker(self, experiment_id: str, worker_id: str,
                        hostname: str, gpu: list, runtime_info: dict,
                        bench: list, batch_size: int | None,
                        device: str | None = None) -> str:
        wrid = _uid("wrun")
        with self._lock:
            self._q("UPDATE worker_runs SET status='offline', "
                    "current_job_id=NULL WHERE experiment_id=? AND "
                    "worker_id=? AND status='online'",
                    (experiment_id, worker_id))
            self._q("INSERT INTO worker_runs(experiment_id,worker_run_id,"
                    "worker_id,hostname,gpu_json,runtime_info_json,bench_json,"
                    "started_at,last_heartbeat_at,status,batch_size,device)"
                    " VALUES(?,?,?,?,?,?,?,?,?,'online',?,?)",
                    (experiment_id, wrid, worker_id, hostname,
                     json.dumps(gpu or []), json.dumps(runtime_info or {}),
                     json.dumps(bench or []), self._now(), self._now(),
                     batch_size, device))
            self._commit()
        return wrid

    def heartbeat(self, experiment_id: str, worker_id: str, sample: dict) -> None:
        with self._lock:
            now = self._now()
            self._q("UPDATE worker_runs SET last_heartbeat_at=?, status='online',"
                    " current_job_id=? WHERE worker_run_id=(SELECT worker_run_id "
                    "FROM worker_runs WHERE experiment_id=? AND worker_id=? "
                    "ORDER BY started_at DESC, rowid DESC LIMIT 1) AND "
                    "status != 'lost'",
                    (now, sample.get("current_job_id"), experiment_id, worker_id))
            self._q("INSERT INTO worker_heartbeats(experiment_id,worker_id,at,"
                    "temperature_c,utilization_pct,vram_used_mb,vram_total_mb,"
                    "current_job_id) VALUES(?,?,?,?,?,?,?,?)",
                    (experiment_id, worker_id, now,
                     sample.get("temperature_c"), sample.get("utilization_pct"),
                     sample.get("vram_used_mb"), sample.get("vram_total_mb"),
                     sample.get("current_job_id")))
            # prune >24h
            self._q("DELETE FROM worker_heartbeats WHERE experiment_id=? AND "
                    "at < datetime('now','-1 day')", (experiment_id,))
            self._commit()

    def worker_finished_job(self, experiment_id: str, worker_id: str,
                            ok: bool) -> None:
        col = "completed_jobs" if ok else "failed_jobs"
        with self._lock:
            self._q(f"UPDATE worker_runs SET {col}={col}+1, current_job_id=NULL "
                    f"WHERE worker_run_id=(SELECT worker_run_id FROM worker_runs "
                    f"WHERE experiment_id=? AND worker_id=? ORDER BY started_at "
                    f"DESC, rowid DESC LIMIT 1)",
                    (experiment_id, worker_id))
            self._commit()

    def list_workers(self, experiment_id: str) -> list[dict]:
        with self._lock:
            rows = self._q("SELECT * FROM worker_runs WHERE experiment_id=? "
                           "ORDER BY started_at", (experiment_id,)).fetchall()
        return [dict(r) for r in rows]

    def get_worker(self, experiment_id: str, worker_id: str) -> dict | None:
        with self._lock:
            row = self._q("SELECT * FROM worker_runs WHERE experiment_id=? AND "
                          "worker_id=? ORDER BY started_at DESC LIMIT 1",
                          (experiment_id, worker_id)).fetchone()
        return dict(row) if row else None

    def mark_worker_status(self, experiment_id: str, worker_id: str,
                           status: str) -> None:
        with self._lock:
            self._q("UPDATE worker_runs SET status=? WHERE experiment_id=? AND "
                    "worker_id=?", (status, experiment_id, worker_id))
            self._commit()

    # ------------------------------------------------------------ checkpoints
    def insert_checkpoint(self, experiment_id: str, reason: str,
                          manifest: dict, path: str | None) -> str:
        cid = _uid("ckpt")
        with self._lock:
            self._q("INSERT INTO checkpoints(experiment_id,checkpoint_id,"
                    "created_at,reason,manifest_json,path) VALUES(?,?,?,?,?,?)",
                    (experiment_id, cid, self._now(), reason,
                     json.dumps(manifest), path))
            self._commit()
        return cid

    def list_checkpoints(self, experiment_id: str) -> list[dict]:
        with self._lock:
            rows = self._q("SELECT * FROM checkpoints WHERE experiment_id=? "
                           "ORDER BY created_at", (experiment_id,)).fetchall()
        return [dict(r) for r in rows]

    def last_event_id(self, experiment_id: str) -> int:
        with self._lock:
            row = self._q("SELECT MAX(event_id) m FROM experiment_events WHERE "
                          "experiment_id=?", (experiment_id,)).fetchone()
        return int(row["m"] or 0)

    # ------------------------------------------------------------ events
    def emit(self, experiment_id: str, type_: str, severity: str = "info",
             payload: dict | None = None, source: str | None = None) -> int:
        with self._lock:
            cur = self._q("INSERT INTO experiment_events(experiment_id,at,type,"
                          "severity,payload_json,source) VALUES(?,?,?,?,?,?)",
                          (experiment_id, self._now(), type_, severity,
                           json.dumps(payload or {}), source))
            self._commit()
            return int(cur.lastrowid)

    def list_events(self, experiment_id: str, since_id: int = 0,
                    limit: int = 200, type_: str | None = None) -> list[dict]:
        with self._lock:
            if type_:
                rows = self._q("SELECT * FROM experiment_events WHERE "
                               "experiment_id=? AND event_id>? AND type=? "
                               "ORDER BY event_id LIMIT ?",
                               (experiment_id, since_id, type_, limit)).fetchall()
            else:
                rows = self._q("SELECT * FROM experiment_events WHERE "
                               "experiment_id=? AND event_id>? ORDER BY event_id "
                               "LIMIT ?", (experiment_id, since_id, limit)).fetchall()
        return [dict(r) for r in rows]

    # ------------------------------------------------------------ telemetry
    def insert_telemetry(self, experiment_id: str,
                         samples: list[dict]) -> int:
        with self._lock:
            for s in samples:
                self._q("INSERT INTO telemetry_samples(experiment_id,at,source,"
                        "domain,signal_type,value,normalized_value,delta,"
                        "confidence,metadata_json) VALUES(?,?,?,?,?,?,?,?,?,?)",
                        (experiment_id, s.get("timestamp") or s.get("at"),
                         s["source"], s["domain"], s["signal_type"],
                         s.get("value"), s.get("normalized_value"),
                         s.get("delta"), s.get("confidence", 1.0),
                         json.dumps(s.get("metadata") or {})))
            self._commit()
        return len(samples)

    def query_telemetry(self, experiment_id: str, domain=None,
                        signal_type=None, since=None, limit=200) -> list[dict]:
        sql = ("SELECT * FROM telemetry_samples WHERE experiment_id=?")
        params: list = [experiment_id]
        if domain:
            sql += " AND domain=?"; params.append(domain)
        if signal_type:
            sql += " AND signal_type=?"; params.append(signal_type)
        if since:
            sql += " AND at>?"; params.append(since)
        sql += " ORDER BY id DESC LIMIT ?"; params.append(limit)
        with self._lock:
            rows = self._q(sql, params).fetchall()
        return [dict(r) for r in rows]

    def telemetry_series(self, experiment_id: str, since_iso: str | None,
                         signal_types: list[str] | None, limit: int = 1000
                         ) -> list[dict]:
        sql = "SELECT * FROM telemetry_samples WHERE experiment_id=?"
        params: list = [experiment_id]
        if since_iso:
            sql += " AND at>=?"; params.append(since_iso)
        if signal_types:
            marks = ",".join("?" * len(signal_types))
            sql += f" AND signal_type IN ({marks})"; params.extend(signal_types)
        sql += " ORDER BY at ASC, id ASC LIMIT ?"; params.append(limit)
        with self._lock:
            rows = self._q(sql, params).fetchall()
        return [dict(r) for r in rows]

    def worker_heartbeat_series(self, experiment_id: str, worker_id: str,
                                since_iso: str | None,
                                limit: int = 1000) -> list[dict]:
        sql = ("SELECT * FROM worker_heartbeats WHERE experiment_id=? AND "
               "worker_id=?")
        params: list = [experiment_id, worker_id]
        if since_iso:
            sql += " AND at>=?"; params.append(since_iso)
        sql += " ORDER BY at ASC LIMIT ?"; params.append(limit)
        with self._lock:
            rows = self._q(sql, params).fetchall()
        return [dict(r) for r in rows]

    def latest_telemetry(self, experiment_id: str, limit=200) -> list[dict]:
        with self._lock:
            rows = self._q(
                "SELECT t.* FROM telemetry_samples t JOIN (SELECT source,"
                "signal_type, MAX(id) m FROM telemetry_samples WHERE "
                "experiment_id=? GROUP BY source,signal_type) x ON t.id=x.m "
                "LIMIT ?", (experiment_id, limit)).fetchall()
        return [dict(r) for r in rows]

    # ------------------------------------------------------------ maintenance
    def wal_checkpoint_truncate(self) -> None:
        with self._lock:
            self.conn.execute("PRAGMA wal_checkpoint(TRUNCATE)")

    def db_sha256(self) -> str:
        self.wal_checkpoint_truncate()
        return hashlib.sha256(self.path.read_bytes()).hexdigest()
