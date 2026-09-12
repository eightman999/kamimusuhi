"""Functional departure evaluation (M2 §16-18).

The question M1 could not ask: what does this individual actually *use*?
Intact performance conflates "the organ does the work" with "the organ
is along for the ride". Departure evaluation removes components and
re-measures, under the same seeds, on the same backend:

    intact              the individual as developed
    sham                the lesion procedure with an empty mask — must
                        equal intact exactly (the negative control)
    founder             the substrate alone (no organs, no attachments)
                        — "can the ancestor still do it?"
    organ_ablation      every artificial organ neuron silenced — "can
                        the substrate do it inside this individual?"
    <sid>_lesion_p      a deterministic severity-p lesion on substrate
                        ``sid`` — "how much function needs the
                        substrate?"

Scores use the same basis as the M-series task: ``task_score =
-|rate - target|`` (the M0-comparable score) and ``task_quality =
1 - |rate - target|/target`` normalised to [0, 1]. Every condition
runs the same replicate seeds, so a difference is a property of the
individual, not the draw. Everything is recorded raw — the derived
dependencies are statistics on top, never a rewrite.

Config (``functional_departure`` in the experiment config)::

    enabled: true
    substrate_id: fba0        # which substrate the lesion applies to
    severities: [0.1, 0.25, 0.5]
    controls: {founder: true, sham: true}
    duration_ms:              # default: the job's duration
    replicates:               # default: the job's replicate count
"""
from __future__ import annotations

from ..fba.seeds import lesion_seed
from ..mie.environments import make_drive
from ..substrate.registry import default_registry

_EPS = 1e-9


def _backend_base(backend) -> int:
    """Neuron index where artificial organs begin — ``n_base`` on the
    torch backend, ``n_neurons`` on the mock."""
    return int(getattr(backend, "n_base", None)
               or getattr(backend, "n_neurons", 0) or 0)


def _score(summary: dict, target_rate: float) -> dict:
    """The condition-level metrics, all raw."""
    rate = float(summary.get("mean_rate_hz") or 0.0)
    score = -abs(rate - target_rate)
    quality = (1.0 - abs(rate - target_rate) / target_rate
               if target_rate > _EPS else 0.0)
    return {
        "mean_rate_hz": rate,
        "task_score": score,
        "task_quality": max(0.0, quality),
        "spikes_total": int(summary.get("spikes_total") or 0),
        "per_replicate_mean_rate_hz":
            [float(r) for r in
             (summary.get("per_replicate_mean_rate_hz") or [])],
        "n_silenced": int(summary.get("n_silenced") or 0),
    }


def _run_condition(backend, phenotype, job, config, device, seeds,
                   silence=None, timer=None) -> dict:
    """Initialise the backend with ``phenotype`` and run one condition
    over all replicate lanes, chunked by the job's execution batch so
    memory pressure matches the main evaluation."""
    from ..fba.replicates import chunk_indices

    n_rep = len(seeds)
    exec_batch = max(1, int((config.get("functional_departure") or {})
                            .get("execution_batch")
                       or job.get("execution_batch_size")
                       or n_rep))
    rates, counts = [], []
    duration = float((config.get("functional_departure") or {})
                     .get("duration_ms") or job["duration_ms"])
    for lanes in chunk_indices(n_rep, exec_batch):
        backend.initialize(phenotype, batch_size=len(lanes),
                           seed=job["seed"], device=device,
                           replicate_seeds=[seeds[i] for i in lanes])
        n_base = _backend_base(backend)
        drive = make_drive(job["environment_id"], n_base, config)
        if silence:
            drive = dict(drive, silence=list(silence))
        backend.set_inputs(drive)
        backend.run(duration)
        s = backend.get_state_summary()
        rates += list(s.get("per_batch_mean_rate_hz") or [])
        counts += list(s.get("per_batch_spike_counts") or [])
    mean = sum(rates) / len(rates) if rates else 0.0
    return {"mean_rate_hz": float(mean),
            "per_replicate_mean_rate_hz": rates,
            "per_replicate_spike_counts": counts,
            "spikes_total": int(sum(counts)),
            "n_silenced": len(silence or [])}


def evaluate_departure(backend, phenotype, job, config, device="cpu",
                       seeds=None, timer=None) -> dict | None:
    """Run the departure battery for one evaluated individual.

    Returns the departure metrics dict, or ``None`` when the feature is
    not enabled in the config — an M1 job's summary is byte-identical to
    before.
    """
    dep = (config or {}).get("functional_departure") or {}
    if not dep.get("enabled"):
        return None

    n_rep = int(dep.get("replicates") or job.get("replicates") or 1)
    seeds = list(seeds or [])
    if len(seeds) < n_rep:
        from ..fba.replicates import replicate_seeds
        seeds = replicate_seeds(int(job["seed"]), n_rep)
    seeds = seeds[:n_rep]
    target = float((config.get("evaluation") or {})
                   .get("target_rate_hz", 5.0))
    sid = str(dep.get("substrate_id") or "fba0")
    severities = [float(s) for s in (dep.get("severities") or [0.1])]
    controls = dep.get("controls") or {}

    conditions: dict[str, dict] = {}

    def run(name, phen, silence=None):
        raw = _run_condition(backend, phen, job, config, device, seeds,
                             silence=silence, timer=timer)
        conditions[name] = dict(_score(raw, target),
                                n_silenced=raw["n_silenced"])

    run("intact", phenotype)
    if controls.get("sham", True):
        run("sham", phenotype, silence=[])
    if controls.get("founder", True):
        founder_phen = dict(phenotype, artificial_organs=[],
                            attachments=[], n_extra_neurons=0)
        run("founder", founder_phen)

    # organ ablation: silence every artificial neuron (indices past the
    # substrate base)
    backend.initialize(phenotype, batch_size=1, seed=job["seed"],
                       device=device, replicate_seeds=[seeds[0]])
    n_base = _backend_base(backend)
    n_extra = int(phenotype.get("n_extra_neurons") or 0)
    if n_extra:
        run("organ_ablation", phenotype,
            silence=list(range(n_base, n_base + n_extra)))
    else:
        conditions["organ_ablation"] = dict(conditions["intact"],
                                            n_silenced=0)

    # substrate lesions at each severity, masks from the substrate's own
    # adapter and the dedicated lesion seed stream
    try:
        adapter = default_registry().get(sid)
        has_lesion = hasattr(adapter, "lesion")
    except KeyError:
        has_lesion = False
    lesion_loss: dict[str, float] = {}
    for i, sev in enumerate(severities):
        name = f"{sid}_lesion_{sev:.2f}"
        if not has_lesion:
            conditions[name] = dict(conditions["intact"], n_silenced=0,
                                    lesion_error="no_adapter")
        else:
            mask = adapter.lesion(sev, n_base,
                                  lesion_seed(int(job["seed"]), i))
            run(name, phenotype, silence=list(mask.neuron_ids))
        lesion_loss[f"{sev:.2f}"] = round(
            conditions["intact"]["task_score"]
            - conditions[name]["task_score"], 6)

    intact = conditions["intact"]["task_score"]
    ablation_loss = round(
        intact - conditions["organ_ablation"]["task_score"], 6)
    dep_mean = (sum(lesion_loss.values()) / len(lesion_loss)
                if lesion_loss else 0.0)

    def _norm(loss: float) -> float | None:
        if abs(intact) < _EPS:
            return None
        return round(max(-1.0, min(1.0, loss / abs(intact))), 6)

    out = {
        "enabled": True,
        "substrate_id": sid,
        "severities": severities,
        "n_replicates": n_rep,
        "conditions": conditions,
        "intact_score": intact,
        "sham_score": (conditions.get("sham") or {}).get("task_score"),
        "founder_score": (conditions.get("founder") or {}).get(
            "task_score"),
        "organ_ablation_score": conditions["organ_ablation"]
        ["task_score"],
        f"{sid}_lesion_loss": lesion_loss,
        "organ_ablation_loss": ablation_loss,
        f"{sid}_dependency": round(dep_mean, 6),
        f"{sid}_dependency_normalized": _norm(dep_mean),
        "artificial_organ_dependency": ablation_loss,
        "artificial_organ_dependency_normalized": _norm(ablation_loss),
        "structural_ancestry_fraction":
            phenotype.get("structural_ancestry_fraction"),
    }
    # flat per-severity score keys (e.g. fba0_lesion_10_score)
    for sev in severities:
        pct = int(round(sev * 100))
        name = f"{sid}_lesion_{sev:.2f}"
        out[f"{sid}_lesion_{pct}_score"] = conditions[name]["task_score"]
    return out
