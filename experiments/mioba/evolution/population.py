"""Trivial population controller (placeholder for real selection).

Seeds `population.target_size` genomes (genome 0 = pure FBA0, the rest
mutations of it), enqueues one smoke-tier evaluation job per genome, and
when all jobs of a generation are terminal produces the next generation
from the top-k by `fitness_placeholder` = -|mean_rate_hz - target_rate|.

New clade rule lives here: when a genome's first artificial organ
appears, a clade row is created (event `new_clade_detected`).

Atomicity: one generation (every child genome, its parents/mutations/
clade rows, birth events, evaluation jobs, and the post-mutation RNG
state written by the ``persist`` callback) is committed in a single
``db.transaction()``. A crash mid-generation rolls the whole generation
back, so resume re-derives exactly the same children from the stored
pre-generation RNG state instead of duplicating the ones already born.
"""
from __future__ import annotations

import json
import random
from typing import Callable

from ..genome.mutation import mutate
from ..genome.schema import Genome, fba0_genome
from ..storage import models as M

ROOT_CLADE = "fba0-root"


def fitness_placeholder(summary: dict, target_rate: float) -> float | None:
    rate = (summary or {}).get("mean_rate_hz")
    if rate is None:
        return None
    return -abs(rate - target_rate)


class PopulationController:
    def __init__(self, db, experiment_id: str, config: dict,
                 rng: random.Random,
                 persist: Callable[[int], None] | None = None):
        self.db = db
        self.experiment_id = experiment_id
        self.config = config
        self.rng = rng
        # persist(born) writes RNG state + counters; called inside the
        # generation transaction so they commit together with the children
        self._persist = persist or (lambda born: None)
        self._best_fitness: float | None = None
        clade = db.get_clade_by_name(experiment_id, ROOT_CLADE)
        self.root_clade_id = (clade["clade_id"] if clade else
                              db.create_clade(experiment_id, ROOT_CLADE, None))

    # ------------------------------------------------------------- seeding
    def seed_if_empty(self) -> int:
        """Create the initial generation if the experiment has no genomes."""
        if self.db.count_genomes(self.experiment_id):
            return 0
        target = int(self.config.get("population", {}).get("target_size", 8))
        eval_cfg = self.config.get("evaluation", {})
        seed = int(self.config.get("evolution", {}).get("mutation_seed", 0))
        base = fba0_genome(seed=seed)
        born = 0
        with self.db.transaction():
            for i in range(target):
                if i == 0:
                    g = base
                    kind = "initial"
                else:
                    g = mutate(base, self.rng, birth_index=i, generation=0)
                    kind = "mutation"
                self._birth(g, kind)
                self._enqueue_eval(g)
                born += 1
            self._persist(born)
        return born

    def _parent_clade(self, genome: Genome) -> str | None:
        """First parent's non-root clade, or root clade, or None."""
        for pid in genome.parent_ids:
            clades = self.db.genome_clades(pid)
            if clades:
                non_root = [c for c in clades if c != self.root_clade_id]
                return non_root[0] if non_root else clades[0]
        return None

    def _parent_has_organs(self, genome: Genome) -> bool:
        for pid in genome.parent_ids:
            row = self.db.get_genome(pid)
            if row is None:
                continue
            try:
                if json.loads(row["genome_json"]).get("artificial_organs"):
                    return True
            except ValueError:
                pass
        return False

    def _birth(self, genome: Genome, kind: str) -> str:
        """Clade rule: the genome in which an artificial organ first
        appears founds a new clade; descendants inherit the parent's
        clade unless they found a new one."""
        parent_clade = self._parent_clade(genome)
        if genome.artificial_organs and not self._parent_has_organs(genome):
            clade_id = self.db.create_clade(
                self.experiment_id, f"clade-{genome.genome_id[4:12]}",
                genome.genome_id)
            self.db.emit(self.experiment_id, M.EV_NEW_CLADE,
                         payload={"genome_id": genome.genome_id,
                                  "clade_id": clade_id},
                         source="population")
        else:
            clade_id = parent_clade or self.root_clade_id
        self.db.insert_genome(self.experiment_id, genome, kind,
                              clade_id=clade_id)
        self.db.emit(self.experiment_id, M.EV_GENOME_BORN,
                     payload={"genome_id": genome.genome_id,
                              "generation": genome.generation,
                              "birth_index": genome.birth_index,
                              "kind": kind},
                     source="population")
        return genome.genome_id

    def _enqueue_eval(self, genome: Genome) -> str:
        from ..coordinator import jobs
        eval_cfg = self.config.get("evaluation", {})
        return jobs.enqueue(
            self.db, self.experiment_id, genome.genome_id,
            environment_id=eval_cfg.get("environment_id",
                                        "synthetic-quiet-v0"),
            seed=genome.random_seed,
            tier=eval_cfg.get("tier", "smoke"),
            backend=eval_cfg.get("backend", "mock"),
            duration_ms=float(eval_cfg.get("duration_ms", 500)),
            requested_traces=[],
            replicates=int(eval_cfg.get("replicates", 1)))

    # ------------------------------------------------------------- fitness
    def record_fitness(self, genome_id: str, fitness: float | None) -> None:
        if fitness is not None and (self._best_fitness is None or
                                    fitness > self._best_fitness):
            self._best_fitness = fitness
            self.db.emit(self.experiment_id, M.EV_NEW_BEST,
                         payload={"genome_id": genome_id,
                                  "fitness_placeholder": fitness},
                         source="population")

    # ------------------------------------------------------------- advance
    def maybe_advance(self) -> int:
        """Spawn next generation when the current one is fully evaluated."""
        max_gen = int(self.config.get("evolution", {}).get("max_generations", 1))
        genomes = self.db.list_genomes(self.experiment_id, limit=10000)
        if not genomes:
            return 0
        current_gen = max(g["generation"] for g in genomes)
        if current_gen + 1 > max_gen:
            return 0
        current = [g for g in genomes if g["generation"] == current_gen]
        jobs_of_gen = []
        for g in current:
            # a generation is done when every genome has a terminal-or-done job
            js = [j for j in self.db.list_jobs(self.experiment_id, limit=10000)
                  if j["genome_id"] == g["genome_id"]]
            jobs_of_gen.extend(js)
            if not js or any(j["status"] not in M.JOB_TERMINAL for j in js):
                return 0
        # pick elite by placeholder fitness
        target_rate = float(self.config.get("evaluation", {})
                            .get("target_rate_hz", 5.0))
        elite_k = int(self.config.get("evolution", {}).get("elite_k", 4))
        scored = []
        for g in current:
            evs = self.db.list_evaluations(self.experiment_id,
                                           genome_id=g["genome_id"], limit=1)
            fit = None
            if evs:
                fit = fitness_placeholder(
                    json.loads(evs[0]["summary_json"]), target_rate)
            scored.append((fit if fit is not None else float("-inf"),
                           g["genome_id"]))
        scored.sort(key=lambda t: t[0], reverse=True)
        elites = [gid for _, gid in scored[:elite_k]] or [current[0]["genome_id"]]
        target = int(self.config.get("population", {}).get("target_size", 8))
        born = 0
        with self.db.transaction():
            self.db.emit(self.experiment_id, "generation_advanced",
                         payload={"from_generation": current_gen,
                                  "to_generation": current_gen + 1,
                                  "elites": elites, "target_size": target},
                         source="population")
            for i in range(target):
                parent_row = self.db.get_genome(self.rng.choice(elites))
                parent = Genome.from_json(parent_row["genome_json"])
                g = mutate(parent, self.rng, birth_index=i,
                           generation=current_gen + 1)
                self._birth(g, "mutation")
                self._enqueue_eval(g)
                born += 1
            self._persist(born)
        return born
