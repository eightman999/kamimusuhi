"""Separated selection metrics (M1 §8-§9).

M0 selected on one number, ``-|mean_rate_hz - target|``, and that number
barely moved. M1 does not fix this by inventing a better single number:
it keeps the components apart, stores them raw, and combines them only at
the last step.

    task_score                  can it do the job
    homeostasis_score           does it keep its own state in range
    disturbance_recovery_score  can it come back from a crisis
    resource_efficiency_score   how little does it need to do so
    structural_functionality    is the circuit it grew on a live path
    novelty                     is it doing something not already seen
    raw_firing_metrics          the unopinionated measurements

``selection_score`` is a weighted combination of the normalised
components. It never overwrites ``fitness``: the M0-comparable placeholder
stays in its own column so the two runs remain comparable, and every raw
component is persisted so weights can be changed and the whole run
re-scored without re-simulating anything.

Two rules protect the efficiency term from selecting for nothing:

**Performance gate (§9).** Below ``minimum_viable_task_score`` the
efficiency bonus is exactly zero. An organism that does nothing consumes
nothing, and without the gate it would win outright.

**Marginal cost only (§2.4-8).** The cost counted is what the genome
added — its own neurons, edges and traversed edges — not the worker's RSS
or the 139k/14M base every organism shares. Otherwise fitness would
depend on which GPU ran the job.

Structural functionality is a *share*, never a count: growing neurons
earns nothing, and growing neurons that sit on a path between FBA0 and
FBA0 earns something only because the share of dead tissue falls.
"""
from __future__ import annotations

import math

COMPONENTS = ("task_score", "homeostasis_score", "disturbance_recovery_score",
              "resource_efficiency_score", "structural_functionality",
              "novelty")

DEFAULTS = {
    "weights": {
        "task_score": 1.0,
        "homeostasis_score": 0.5,
        "disturbance_recovery_score": 0.8,
        "resource_efficiency_score": 0.4,
        "structural_functionality": 0.2,
        "novelty": 0.2,
    },
    # normalised task quality below which no efficiency bonus is given
    "minimum_viable_task_score": 0.35,
    # references that turn absolute costs into 0..1 (config, not magic:
    # they are the scale at which a circuit is considered "large")
    "memory_cost_reference_bytes": 512 * 1024,
    "compute_cost_reference_edges": 5000.0,
    "novelty_neighbours": 5,
}


def merged_config(config: dict | None) -> dict:
    cfg = {k: (dict(v) if isinstance(v, dict) else v)
           for k, v in DEFAULTS.items()}
    user = (config or {}).get("fitness") or {}
    for key, value in user.items():
        if isinstance(value, dict) and isinstance(cfg.get(key), dict):
            cfg[key].update(value)
        else:
            cfg[key] = value
    return cfg


def fitness_placeholder(summary: dict, target_rate: float) -> float | None:
    """The M0 metric, kept unchanged so M0 and M1 stay comparable."""
    rate = (summary or {}).get("mean_rate_hz")
    if rate is None:
        return None
    return -abs(rate - target_rate)


def _mean(values):
    vals = [v for v in values if v is not None]
    return (sum(vals) / len(vals)) if vals else None


def task_quality(task_score: float | None, target_rate: float) -> float | None:
    """Map ``-|rate - target|`` onto 0..1 so it can be combined with
    unitless components. 1 = on target, 0 = off by a whole target."""
    if task_score is None or target_rate <= 0:
        return None
    return max(0.0, 1.0 + task_score / target_rate)


def episode_metrics(summary: dict, target_rate: float) -> dict:
    """Pull the per-lane episode analyses out of an evaluation summary."""
    episode = (summary or {}).get("episode") or {}
    lanes = episode.get("per_lane") or []
    if not lanes:
        return {"task_score": fitness_placeholder(summary, target_rate),
                "mean_debt": None, "peak_debt": None,
                "recovery": None, "recovered_fraction": None,
                "disturbance_impact": None, "terminated_fraction": 0.0,
                "n_disturbances": 0, "has_disturbance": False}

    windows = [w for lane in lanes for w in (lane.get("disturbances") or [])]
    recoveries = [w for w in windows if w.get("performance_before") is not None]
    recovered = [w for w in recoveries if w.get("recovered")]
    slices = max(1, int(episode.get("slices") or 1))
    # recovery quality: 1 = back immediately, 0 = never came back
    recovery_scores = []
    for w in recoveries:
        if w.get("recovered"):
            recovery_scores.append(
                max(0.0, 1.0 - (w["recovery_slices"] or 0) / slices))
        else:
            recovery_scores.append(0.0)

    return {
        "task_score": _mean(l.get("task_score_clean")
                            if l.get("task_score_clean") is not None
                            else l.get("task_score_all") for l in lanes),
        "task_score_all": _mean(l.get("task_score_all") for l in lanes),
        "mean_debt": _mean(l.get("final_debt") for l in lanes),
        "peak_debt": _mean(l.get("peak_debt") for l in lanes),
        "recovery": _mean(recovery_scores) if recovery_scores else None,
        "recovered_fraction": (len(recovered) / len(recoveries)
                               if recoveries else None),
        "disturbance_impact": _mean(w.get("impact") for w in windows),
        "terminated_fraction": (sum(1 for l in lanes
                                    if l.get("terminated_early"))
                                / len(lanes)),
        "n_disturbances": len(windows),
        "has_disturbance": bool(windows),
    }


def resource_metrics(summary: dict) -> dict:
    res = (summary or {}).get("resource") or {}
    circuit = (summary or {}).get("circuit") or {}
    return {
        "marginal_memory_cost_bytes": res.get("marginal_memory_cost_bytes"),
        "marginal_compute_cost": res.get("marginal_compute_cost"),
        "artificial_neuron_count": (res.get("artificial_neuron_count")
                                    or circuit.get("n_artificial_neurons")),
        "artificial_edge_count": (res.get("artificial_edge_count")
                                  or circuit.get("nnz_artificial")),
        "genome_bytes": res.get("genome_bytes"),
        "active_propagated_edges": res.get("active_propagated_edges"),
        "estimated_operations": res.get("estimated_operations"),
    }


def structural_functionality(structure: dict | None) -> float | None:
    """Share of this organism's artificial neurons that sit on a live
    path. ``None`` when it has no artificial circuit at all — an organism
    with no organs is neither rewarded nor punished for it."""
    s = structure or {}
    total = s.get("n_artificial_neurons") or 0
    if not total:
        return None
    return (s.get("n_functional_neurons") or 0) / total


def descriptor(summary: dict, structure: dict | None) -> list[float]:
    """Behavioural descriptor for novelty (M1 §9).

    Deliberately behavioural plus coarse structure: novelty must not
    reduce to "has more neurons", or it would re-introduce the growth
    bonus §4 removes.
    """
    s = summary or {}
    st = structure or {}
    return [
        float(s.get("mean_rate_hz") or 0.0),
        float(s.get("rate_std_hz") or 0.0),
        float(s.get("active_fraction") or 0.0),
        float((s.get("propagation_activity") or {})
              .get("active_presynaptic_ratio") or 0.0) * 1000.0,
        float(st.get("counts", {}).get("functional", 0)),
    ]


def novelty(desc: list[float], archive: list[list[float]],
            k: int = 5) -> float | None:
    """Mean distance to the ``k`` nearest descriptors in the archive."""
    if not archive:
        return None
    dists = []
    for other in archive:
        n = min(len(desc), len(other))
        dists.append(math.sqrt(sum((desc[i] - other[i]) ** 2
                                   for i in range(n))))
    dists.sort()
    near = dists[:max(1, int(k))]
    return sum(near) / len(near)


def compute_metrics(summary: dict, structure: dict | None, config: dict | None,
                    target_rate: float, archive: list | None = None) -> dict:
    """Every raw component, plus the normalised values used for selection.

    Raw values are what gets persisted; normalisation is a presentation
    choice that a later re-analysis is free to redo.
    """
    cfg = merged_config(config)
    ep = episode_metrics(summary, target_rate)
    res = resource_metrics(summary)
    struct = structural_functionality(structure)

    quality = task_quality(ep["task_score"], target_rate)
    # homeostasis: nothing accumulated = 1, debt at the termination
    # threshold = 0
    terminate_at = float(((config or {}).get("environment") or {})
                         .get("virtual", {}).get("debt_terminate", 6.0))
    peak = ep.get("peak_debt")
    homeostasis = (None if peak is None
                   else max(0.0, 1.0 - peak / max(1e-9, terminate_at)))

    mem = res.get("marginal_memory_cost_bytes")
    comp = res.get("marginal_compute_cost")
    cost = 0.0
    if mem is not None:
        cost += mem / float(cfg["memory_cost_reference_bytes"])
    if comp is not None:
        cost += comp / float(cfg["compute_cost_reference_edges"])
    gate = float(cfg["minimum_viable_task_score"])
    if quality is None or quality < gate:
        efficiency = 0.0
        gated = True
    else:
        efficiency = quality / (cost + 1.0)
        gated = False

    desc = descriptor(summary, structure)
    nov = novelty(desc, archive or [], int(cfg["novelty_neighbours"]))

    raw = {
        "task_score": ep["task_score"],
        "task_score_all": ep.get("task_score_all"),
        "task_quality": quality,
        "mean_debt": ep["mean_debt"],
        "peak_debt": ep["peak_debt"],
        "homeostasis_score": homeostasis,
        "disturbance_recovery_score": ep["recovery"],
        "recovered_fraction": ep["recovered_fraction"],
        "disturbance_impact": ep["disturbance_impact"],
        "terminated_fraction": ep["terminated_fraction"],
        "n_disturbances": ep["n_disturbances"],
        "has_disturbance": ep["has_disturbance"],
        "resource_efficiency_score": efficiency,
        "resource_efficiency_gated": gated,
        "resource_cost_normalised": round(cost, 6),
        "structural_functionality": struct,
        "novelty": nov,
        "descriptor": desc,
        "raw_firing_metrics": {
            "mean_rate_hz": (summary or {}).get("mean_rate_hz"),
            "rate_std_hz": (summary or {}).get("rate_std_hz"),
            "active_fraction": (summary or {}).get("active_fraction"),
            "spikes_total": (summary or {}).get("spikes_total"),
        },
        "resource": res,
    }
    raw["selection_components"] = normalised_components(raw, cfg, target_rate)
    return raw


def normalised_components(raw: dict, cfg: dict, target_rate: float) -> dict:
    """Each component on a comparable 0..1-ish scale. ``None`` means "not
    applicable to this individual" and is dropped from the combination
    rather than counted as zero — a control episode with no disturbance
    must not be scored as having failed to recover from one."""
    nov = raw.get("novelty")
    return {
        "task_score": raw.get("task_quality"),
        "homeostasis_score": raw.get("homeostasis_score"),
        "disturbance_recovery_score": raw.get("disturbance_recovery_score"),
        "resource_efficiency_score": raw.get("resource_efficiency_score"),
        "structural_functionality": raw.get("structural_functionality"),
        # novelty is unbounded above; squash it so one outlier cannot
        # dominate the sum
        "novelty": (None if nov is None else nov / (1.0 + nov)),
    }


def selection_score(raw: dict, config: dict | None = None) -> float | None:
    """Weighted combination of the applicable components.

    Weights are renormalised over the components that apply, so an
    individual whose episode had no disturbance is compared on what it
    *was* tested on rather than being penalised for the draw.
    """
    cfg = merged_config(config)
    weights = cfg["weights"]
    comps = raw.get("selection_components") or {}
    used, total_w = 0.0, 0.0
    for name in COMPONENTS:
        value = comps.get(name)
        w = float(weights.get(name, 0.0))
        if value is None or w == 0.0:
            continue
        used += w * float(value)
        total_w += w
    if total_w == 0.0:
        return None
    return used / total_w
