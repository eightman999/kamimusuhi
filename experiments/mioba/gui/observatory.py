"""Live Observatory data (M1 §11-§19).

M0's GUI answered "is the GPU busy?". That is an operations question, and
it still has a page — Infrastructure. The top page now answers the
research question instead: *what is happening to the population?* Who was
born, what grew, which crisis is running, who came back from it, which
branch just died.

Three rules shape everything here.

**The simulation is never slowed down for the display (§19).** Every
payload is computed from the lineage DB and the event log on request.
Nothing in this module is called from a simulation loop, and the page
polls on a 1-2 s timer rather than streaming per step.

**One canonical firing rate (§18).** M0's GUI showed 0.50 Hz while
fitness saw 9.5 Hz, because the two were computed from different things.
:func:`canonical_rate` is now the single source: the same evaluation row,
the same field, for the fitness input and for the display. Anything the
GUI shows that is *not* that number carries a different name.

**Size is not achievement (§14).** The "notable organism" list is
deliberately not a leaderboard of one metric. It surfaces the largest
circuit *and* the smallest successful one, the cheapest survivor, the
strangest mutation, the fastest recovery — the individuals worth looking
at, which are rarely the individuals at the top of any single column.
"""
from __future__ import annotations

import json

from ..storage import models as M


def _loads(text, default=None):
    try:
        return json.loads(text or "")
    except (ValueError, TypeError):
        return default if default is not None else {}


# ------------------------------------------------------------------ rates
def canonical_rate(evaluation: dict) -> dict:
    """The one firing-rate pipeline (M1 §18).

    ``spike_count -> population size -> simulated duration -> mean_rate_hz``
    is computed once, in the backend, and stored in the evaluation
    summary. Both the fitness input and every display read *this* field of
    *this* row. The derived values are returned alongside it so a reader
    can check the arithmetic rather than trust it.
    """
    summary = _loads(evaluation.get("summary_json"), {}) \
        if "summary_json" in evaluation else (evaluation.get("summary") or {})
    circuit = summary.get("circuit") or {}
    spikes = summary.get("spikes_total")
    n_neurons = circuit.get("n_neurons")
    t_ms = summary.get("t_ms")
    replicates = (evaluation.get("completed_replicates")
                  or summary.get("replicate_index")
                  and len(summary["replicate_index"]) or 1)
    derived = None
    if spikes is not None and n_neurons and t_ms:
        derived = spikes / (n_neurons * (t_ms / 1000.0) * max(1, replicates))
    return {
        # what fitness consumed and what the GUI must display
        "mean_rate_hz": summary.get("mean_rate_hz"),
        "rate_std_hz": summary.get("rate_std_hz"),
        "source": "evaluations.summary_json:mean_rate_hz",
        "evaluation_id": evaluation.get("evaluation_id"),
        # the inputs, so the number can be checked
        "spikes_total": spikes,
        "population_size": n_neurons,
        "simulated_ms": t_ms,
        "replicates": replicates,
        "recomputed_mean_rate_hz": (round(derived, 6) if derived is not None
                                    else None),
    }


# ------------------------------------------------------------ population
def _latest_evaluations(db, experiment_id: str) -> dict:
    """genome_id -> its most recent evaluation row."""
    out: dict[str, dict] = {}
    for ev in db.list_evaluations(experiment_id, limit=100000):
        gid = ev["genome_id"]
        if gid not in out or (ev.get("finished_at") or "") > \
                (out[gid].get("finished_at") or ""):
            out[gid] = ev
    return out


def _individual(row: dict, ev: dict | None) -> dict:
    structure = _loads(row.get("structure_json"), {})
    metrics = _loads((ev or {}).get("metrics_json"), {}) if ev else {}
    summary = _loads((ev or {}).get("summary_json"), {}) if ev else {}
    resource = metrics.get("resource") or summary.get("resource") or {}
    counts = structure.get("counts") or {}
    return {
        "genome_id": row["genome_id"],
        "generation": row["generation"],
        "birth_index": row["birth_index"],
        "parent_ids": _loads(row.get("parent_ids_json"), []),
        "evaluated": ev is not None,
        "evaluation_id": (ev or {}).get("evaluation_id"),
        "fitness": (ev or {}).get("fitness"),
        "selection_score": (ev or {}).get("selection_score"),
        "mean_rate_hz": canonical_rate(ev)["mean_rate_hz"] if ev else None,
        "artificial_neurons": structure.get("n_artificial_neurons") or 0,
        "artificial_organs": structure.get("n_enabled_organs") or 0,
        "attachments": structure.get("n_enabled_attachments") or 0,
        "functional_organs": counts.get("functional", 0),
        "neutral_organs": counts.get("neutral_structure", 0),
        "invalid_organs": counts.get("invalid_structure", 0),
        "disabled_organs": counts.get("disabled", 0),
        "marginal_memory_bytes": resource.get("marginal_memory_cost_bytes"),
        "marginal_compute": resource.get("marginal_compute_cost"),
        "task_score": metrics.get("task_score"),
        "homeostasis_score": metrics.get("homeostasis_score"),
        "recovery_score": metrics.get("disturbance_recovery_score"),
        "efficiency_score": metrics.get("resource_efficiency_score"),
        "novelty": metrics.get("novelty"),
        "peak_debt": metrics.get("peak_debt"),
        "terminated_fraction": metrics.get("terminated_fraction"),
        "disturbances": metrics.get("n_disturbances"),
        "structure": structure,
    }


def population_view(db, experiment_id: str, generation: int | None = None
                    ) -> dict:
    """The individuals currently on stage, plus the generation summary."""
    genomes = db.list_genomes(experiment_id, limit=100000)
    if not genomes:
        return {"generation": None, "individuals": [], "counts": {}}
    current = generation if generation is not None else \
        max(g["generation"] for g in genomes)
    evals = _latest_evaluations(db, experiment_id)
    rows = [_individual(g, evals.get(g["genome_id"]))
            for g in genomes if g["generation"] == current]
    rows.sort(key=lambda r: (r["birth_index"],))
    evaluated = [r for r in rows if r["evaluated"]]
    return {
        "generation": current,
        "population": len(rows),
        "evaluated": len(evaluated),
        "pending": len(rows) - len(evaluated),
        "individuals": rows,
        "neuron_histogram": _histogram(
            [r["artificial_neurons"] for r in rows]),
        "counts": {
            "functional_organs": sum(r["functional_organs"] for r in rows),
            "neutral_organs": sum(r["neutral_organs"] for r in rows),
            "invalid_organs": sum(r["invalid_organs"] for r in rows),
            "disabled_organs": sum(r["disabled_organs"] for r in rows),
        },
    }


def _histogram(values: list[int], bins: int = 8) -> dict:
    vals = [v for v in values if v is not None]
    if not vals:
        return {"bins": [], "counts": []}
    lo, hi = min(vals), max(vals)
    if hi == lo:
        return {"bins": [lo], "counts": [len(vals)]}
    width = (hi - lo) / bins
    counts = [0] * bins
    for v in vals:
        idx = min(bins - 1, int((v - lo) / width))
        counts[idx] += 1
    return {"bins": [round(lo + i * width, 2) for i in range(bins)],
            "counts": counts, "min": lo, "max": hi}


# ------------------------------------------------------------ extinction
def lineage_view(db, experiment_id: str, limit: int = 4000) -> dict:
    """Parent -> child edges, with each branch's fate.

    "Extinct" here means exactly one thing: this lineage has no member in
    the newest generation. It is a statement about the run, not a guess
    about the future.
    """
    genomes = db.list_genomes(experiment_id, limit=limit)
    if not genomes:
        return {"nodes": [], "edges": [], "generations": 0, "extinct": 0}
    newest = max(g["generation"] for g in genomes)
    evals = _latest_evaluations(db, experiment_id)
    by_id = {g["genome_id"]: g for g in genomes}

    children: dict[str, list[str]] = {}
    for g in genomes:
        for pid in _loads(g.get("parent_ids_json"), []):
            children.setdefault(pid, []).append(g["genome_id"])

    # a genome is "alive" if it or a descendant reached the newest gen
    alive: set[str] = set()
    for g in genomes:
        if g["generation"] != newest:
            continue
        node = g["genome_id"]
        while node:
            if node in alive:
                break
            alive.add(node)
            parents = _loads(by_id.get(node, {}).get("parent_ids_json"), [])
            node = parents[0] if parents else None

    best = None
    nodes, edges = [], []
    for g in genomes:
        ev = evals.get(g["genome_id"])
        score = (ev or {}).get("selection_score")
        if score is not None and (best is None or score > best[1]):
            best = (g["genome_id"], score)
        structure = _loads(g.get("structure_json"), {})
        parents = _loads(g.get("parent_ids_json"), [])
        parent_structure = _loads(
            by_id.get(parents[0], {}).get("structure_json"), {}) \
            if parents else {}
        nodes.append({
            "genome_id": g["genome_id"],
            "generation": g["generation"],
            "birth_index": g["birth_index"],
            "alive": g["genome_id"] in alive,
            "selection_score": score,
            "artificial_neurons": structure.get("n_artificial_neurons") or 0,
            "organs": structure.get("n_enabled_organs") or 0,
            # markers the lineage view highlights
            "first_organ": bool(
                (structure.get("n_enabled_organs") or 0) > 0
                and (parent_structure.get("n_enabled_organs") or 0) == 0),
            "structural_innovation": bool(
                (structure.get("n_enabled_organs") or 0)
                != (parent_structure.get("n_enabled_organs") or 0)),
        })
        for pid in parents:
            edges.append({"parent": pid, "child": g["genome_id"]})
    for node in nodes:
        node["champion"] = bool(best and node["genome_id"] == best[0])
    return {
        "nodes": nodes, "edges": edges,
        "generations": newest + 1,
        "alive": len(alive),
        "extinct": sum(1 for n in nodes if not n["alive"]),
        "champion": best[0] if best else None,
    }


def extinction_events(db, experiment_id: str) -> list[dict]:
    """Lineages whose last member was born in an earlier generation."""
    genomes = db.list_genomes(experiment_id, limit=100000)
    if not genomes:
        return []
    newest = max(g["generation"] for g in genomes)
    by_clade: dict[str, list[dict]] = {}
    for g in genomes:
        for clade in db.genome_clades(g["genome_id"]) or ["<root>"]:
            by_clade.setdefault(clade, []).append(g)
    out = []
    for clade, members in by_clade.items():
        last = max(m["generation"] for m in members)
        if last < newest:
            out.append({"clade_id": clade, "last_generation": last,
                        "members": len(members),
                        "generations_gone": newest - last})
    out.sort(key=lambda r: -r["last_generation"])
    return out


# --------------------------------------------------------------- notable
NOTABLE = (
    ("largest_circuit", "largest circuit", "artificial_neurons", True, False),
    ("smallest_successful", "smallest successful circuit",
     "artificial_neurons", False, True),
    ("lowest_memory", "lowest memory, still working",
     "marginal_memory_bytes", False, True),
    ("most_organs", "most artificial organs", "artificial_organs", True,
     False),
    ("most_novel", "highest structural novelty", "novelty", True, False),
    ("fastest_recovery", "fastest disturbance recovery", "recovery_score",
     True, False),
    ("most_efficient", "most efficient", "efficiency_score", True, False),
    ("steadiest", "steadiest under load", "homeostasis_score", True, False),
)


def notable_organisms(db, experiment_id: str, viable_quantile: float = 0.5
                      ) -> dict:
    """The current menagerie (M1 §14).

    "Successful" is defined once, here: at or above the median selection
    score of the individuals that have been evaluated. Without that gate,
    "smallest circuit" and "lowest memory" would always be won by whatever
    does nothing, which is the failure mode §9 exists to prevent.
    """
    genomes = db.list_genomes(experiment_id, limit=100000)
    evals = _latest_evaluations(db, experiment_id)
    rows = [_individual(g, evals.get(g["genome_id"])) for g in genomes]
    rows = [r for r in rows if r["evaluated"]]
    if not rows:
        return {"viability_threshold": None, "entries": []}

    scores = sorted(r["selection_score"] for r in rows
                    if r["selection_score"] is not None)
    threshold = (scores[int(len(scores) * viable_quantile)]
                 if scores else None)
    viable = [r for r in rows
              if threshold is None or (r["selection_score"] is not None
                                       and r["selection_score"] >= threshold)]

    entries = []
    for key, label, field, want_max, needs_viable in NOTABLE:
        pool = viable if needs_viable else rows
        pool = [r for r in pool if r.get(field) is not None]
        if not pool:
            continue
        pick = (max if want_max else min)(pool, key=lambda r: r[field])
        entries.append({"key": key, "label": label, "field": field,
                        "value": pick[field], "viable_only": needs_viable,
                        "individual": pick})

    jump = _largest_phenotype_jump(db, genomes, evals)
    if jump:
        entries.append(jump)
    return {"viability_threshold": threshold, "entries": entries,
            "viable": len(viable), "evaluated": len(rows)}


def _largest_phenotype_jump(db, genomes, evals) -> dict | None:
    """The child that came out least like its parent (M1 §14).

    Measured on the canonical firing rate, not on size: a large jump in
    behaviour is interesting, a large jump in neuron count is not.
    """
    by_id = {g["genome_id"]: g for g in genomes}
    best = None
    for g in genomes:
        ev = evals.get(g["genome_id"])
        if not ev:
            continue
        rate = canonical_rate(ev)["mean_rate_hz"]
        for pid in _loads(g.get("parent_ids_json"), []):
            pev = evals.get(pid)
            if not pev or pid not in by_id:
                continue
            prate = canonical_rate(pev)["mean_rate_hz"]
            if rate is None or prate is None:
                continue
            delta = abs(rate - prate)
            if best is None or delta > best[0]:
                best = (delta, g, pid, rate, prate)
    if not best:
        return None
    delta, g, pid, rate, prate = best
    return {"key": "largest_phenotype_jump",
            "label": "largest parent-child phenotype jump",
            "field": "mean_rate_hz", "value": round(delta, 4),
            "viable_only": False,
            "individual": _individual(g, evals.get(g["genome_id"])),
            "detail": {"parent_id": pid, "parent_rate_hz": prate,
                       "child_rate_hz": rate}}


# ----------------------------------------------------------------- events
LIFE_EVENTS = (M.EV_GENOME_BORN, M.EV_STRUCTURAL_MUTATION,
               M.EV_NEUTRAL_STRUCTURE, M.EV_INVALID_STRUCTURE,
               M.EV_NEW_CLADE, M.EV_NEW_BEST, M.EV_NEW_BEST_SELECTION,
               M.EV_EVALUATION_SUCCEEDED, M.EV_HIGH_ACTIVITY,
               "generation_advanced")

HIGHLIGHT = {M.EV_STRUCTURAL_MUTATION: "NEW ORGAN",
             M.EV_NEW_CLADE: "FIRST OF LINEAGE",
             M.EV_NEW_BEST_SELECTION: "FITNESS RECORD",
             M.EV_HIGH_ACTIVITY: "HYPERACTIVE"}


def event_feed(db, experiment_id: str, since_id: int = 0,
               limit: int = 200) -> dict:
    """Life events, newest last, with the ones worth shouting about
    marked (M1 §13)."""
    rows = db.list_events(experiment_id, since_id, 100000)
    life = [e for e in rows if e["type"] in LIFE_EVENTS]
    out = []
    for e in life[-limit:]:
        payload = _loads(e.get("payload_json"), {})
        label = HIGHLIGHT.get(e["type"])
        if e["type"] == M.EV_STRUCTURAL_MUTATION:
            op = payload.get("operator")
            label = op if op else label
        out.append({"event_id": e["event_id"], "at": e["at"],
                    "type": e["type"], "severity": e.get("severity"),
                    "highlight": label, "payload": payload})
    return {"events": out,
            "last_event_id": (out[-1]["event_id"] if out else since_id)}


def structural_innovations(db, experiment_id: str, limit: int = 50) -> list:
    rows = db.list_events(experiment_id, 0, 100000, M.EV_STRUCTURAL_MUTATION)
    out = []
    for e in rows[-limit:]:
        payload = _loads(e.get("payload_json"), {})
        if payload.get("operator") in ("NEW_ORGAN", "DUPLICATE_ORGAN",
                                       "GROW_ORGAN"):
            out.append({"at": e["at"], "event_id": e["event_id"], **payload})
    return out


# ------------------------------------------------------------ environment
def current_environment(db, experiment_id: str) -> dict:
    """What the most recently finished evaluations were living through.

    This is the *scientific* environment (the virtual one the organisms
    are in), not the machine's temperature — those are different pages on
    purpose (M1 §2.4-10).
    """
    evals = db.list_evaluations(experiment_id, limit=12)
    kinds: dict[str, int] = {}
    debts, disturbed, total = [], 0, 0
    for ev in evals:
        summary = _loads(ev.get("summary_json"), {})
        episode = summary.get("episode") or {}
        for env in episode.get("environments") or []:
            total += 1
            sched = env.get("schedule") or {}
            if sched.get("n_events"):
                disturbed += 1
            for kind in sched.get("kinds") or []:
                kinds[kind] = kinds.get(kind, 0) + 1
            if env.get("peak_debt") is not None:
                debts.append(env["peak_debt"])
    return {
        "kind": "RECORDED",
        "episodes_sampled": total,
        "disturbed_episodes": disturbed,
        "active_kinds": kinds,
        "mean_peak_debt": (round(sum(debts) / len(debts), 4) if debts
                           else None),
        "max_peak_debt": (max(debts) if debts else None),
    }


# -------------------------------------------------------------- inspector
def individual_view(db, experiment_id: str, genome_id: str) -> dict:
    """Everything about one organism (M1 §16)."""
    row = db.get_genome(genome_id)
    if row is None:
        return {}
    evals = db.list_evaluations(experiment_id, genome_id=genome_id, limit=20)
    latest = evals[0] if evals else None
    summary = _loads((latest or {}).get("summary_json"), {})
    metrics = _loads((latest or {}).get("metrics_json"), {})
    episode = summary.get("episode") or {}
    lanes = episode.get("per_lane") or []
    return {
        "individual": _individual(row, latest),
        "genome": _loads(row.get("genome_json"), {}),
        "mutations": db.genome_mutations(genome_id),
        "parents": db.genome_parents(genome_id),
        "children": db.children_of(genome_id),
        "clades": db.genome_clades(genome_id),
        # M1 §16: homeostasis timeline, per replicate
        "homeostasis": [
            {"trace": lane.get("trace") or [],
             "disturbances": lane.get("disturbances") or [],
             "peak_debt": lane.get("peak_debt"),
             "terminated_early": lane.get("terminated_early")}
            for lane in lanes],
        "environments": episode.get("environments") or [],
        "circuit": summary.get("circuit") or {},
        "region_activity": summary.get("activity") or {},
        "propagation_activity": summary.get("propagation_activity") or {},
        "resource": metrics.get("resource") or summary.get("resource") or {},
        "metrics": metrics,
        "rate": canonical_rate(latest) if latest else None,
        "evaluations": evals,
        "timing": summary.get("timing"),
    }


# ------------------------------------------------------------------ live
def live_view(db, experiment_id: str, service=None, since_id: int = 0
              ) -> dict:
    pop = population_view(db, experiment_id)
    lineage = lineage_view(db, experiment_id)
    return {
        "kind": "DERIVED",
        "status": service.status() if service is not None else None,
        "population": pop,
        "lineage_summary": {
            "generations": lineage["generations"],
            "alive": lineage["alive"],
            "extinct": lineage["extinct"],
            "champion": lineage["champion"],
        },
        "extinctions": extinction_events(db, experiment_id)[:10],
        "environment": current_environment(db, experiment_id),
        "notable": notable_organisms(db, experiment_id),
        "innovations": structural_innovations(db, experiment_id, limit=20),
        "feed": event_feed(db, experiment_id, since_id=since_id, limit=120),
    }
