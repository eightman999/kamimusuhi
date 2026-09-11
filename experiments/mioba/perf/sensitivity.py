"""Mutation sensitivity sweep (M1 §10).

M0's post-mortem found that mutation barely moved the phenotype: every
individual sat at ~9.5 Hz and the within-generation fitness spread was
0.002-0.005. Selection was working; there was simply nothing to select
between. Before spending a real run on that again, M1 measures which
mutations actually change an organism.

Each mutable quantity is swept alone over a range of multipliers
(0.6 … 1.4 by default) from the same founder genome on the same base
graph, and the resulting phenotype is compared to the unmutated
baseline:

    mean firing rate, activity fraction, spike variance,
    region-wise activity, task score, disturbance response

The output is a sensitivity number per quantity — the mean relative
movement it causes — and a config fragment: draw weights proportional to
sensitivity, and wider scale ranges for the quantities that need one to
matter at all. A parameter that moves nothing is not removed (that would
be a judgement about biology); it is simply drawn less often, and the
report says why.

``organ size`` and ``attachment weight`` are swept the same way, because
"how much does a structural change matter" is the same question as "how
much does a parameter change matter", and M1's mutation mix is mostly
structural.
"""
from __future__ import annotations

import copy

from ..development.phenotype import develop
from ..genome.schema import (ArtificialOrgan, Attachment, Genome,
                             ParameterMutation, fba0_genome)

DEFAULT_FACTORS = (0.6, 0.8, 0.9, 1.1, 1.2, 1.4)

# what is swept: the LIF parameters plus the two structural quantities
DEFAULT_TARGETS = ("vThr", "tauMem", "wScale", "tauSyn", "tRefrac",
                   "attachment_weight", "organ_size")

# metrics compared against the baseline; each is a scalar the mutation
# either moves or does not
METRICS = ("mean_rate_hz", "active_fraction", "rate_std_hz",
           "region_spread", "task_score", "recovery")


def _organ_genome(size: int = 32, weight_scale: float = 1.0) -> Genome:
    """A founder with one properly wired organ, so the structural
    quantities have something to act on."""
    g = fba0_genome()
    organ = ArtificialOrgan(organ_id="org_sweep", kind="lif_cluster",
                            size=int(size))
    g.artificial_organs.append(organ)
    g.attachments.append(Attachment(attachment_id="att_in",
                                    source="fba0:medulla",
                                    target=organ.organ_id,
                                    weight_scale=float(weight_scale)))
    g.attachments.append(Attachment(attachment_id="att_out",
                                    source=organ.organ_id,
                                    target="fba0:central_complex",
                                    weight_scale=float(weight_scale)))
    return g.finalize()


def variant(target: str, factor: float, base_size: int = 32) -> Genome:
    """The founder with exactly one quantity scaled by ``factor``."""
    if target == "organ_size":
        return _organ_genome(size=max(1, int(round(base_size * factor))))
    if target == "attachment_weight":
        return _organ_genome(size=base_size, weight_scale=factor)
    g = _organ_genome(size=base_size)
    g.parameter_mutations.append(ParameterMutation(
        mutation_id=f"sweep_{target}", path=target, op="scale",
        value=float(factor), scope="global"))
    return g.finalize()


def _region_spread(summary: dict) -> float | None:
    """How unevenly the network is firing across its groups. A mutation
    that raises the mean rate everywhere is a different thing from one
    that redistributes activity, and fitness may care about the second."""
    act = (summary or {}).get("activity") or {}
    values = [v for vals in act.values() for v in (vals or [])
              if v is not None]
    if len(values) < 2:
        return None
    mean = sum(values) / len(values)
    var = sum((v - mean) ** 2 for v in values) / len(values)
    return var ** 0.5


def _metrics_of(summary: dict, target_rate: float) -> dict:
    from ..evolution import fitness as F

    ep = F.episode_metrics(summary, target_rate)
    return {
        "mean_rate_hz": (summary or {}).get("mean_rate_hz"),
        "active_fraction": (summary or {}).get("active_fraction"),
        "rate_std_hz": (summary or {}).get("rate_std_hz"),
        "region_spread": _region_spread(summary),
        "task_score": ep.get("task_score"),
        "recovery": ep.get("recovery"),
    }


def _relative_move(base, value) -> float | None:
    if base is None or value is None:
        return None
    scale = max(abs(base), 1e-6)
    return abs(value - base) / scale


def sweep(config: dict, device: str = "cpu",
          targets: tuple[str, ...] = DEFAULT_TARGETS,
          factors: tuple[float, ...] = DEFAULT_FACTORS,
          duration_ms: float | None = None, replicates: int | None = None,
          base_size: int = 32, data_dir: str | None = None) -> dict:
    """Run the sweep and return per-target sensitivity plus a config
    fragment for ``evolution.mutation``."""
    from .evalbench import run_one

    target_rate = float((config.get("evaluation") or {})
                        .get("target_rate_hz", 5.0))
    cfg = copy.deepcopy(config)

    def evaluate(genome: Genome) -> dict:
        r = run_one(genome, cfg, device, seed=genome.random_seed or 1,
                    duration_ms=duration_ms, replicates=replicates,
                    data_dir=data_dir, profile=False)
        if not r["ok"]:
            return {"ok": False, "error": r["error"]}
        m = _metrics_of(r["summary"], target_rate)
        m["ok"] = True
        m["wall_s"] = round(r["wall_s"], 4)
        return m

    baseline = evaluate(_organ_genome(size=base_size))
    rows = []
    for target in targets:
        points = []
        for factor in factors:
            measured = evaluate(variant(target, factor, base_size))
            moves = {k: _relative_move(baseline.get(k), measured.get(k))
                     for k in METRICS} if measured.get("ok") else {}
            applicable = [v for v in moves.values() if v is not None]
            points.append({
                "factor": factor,
                "metrics": {k: measured.get(k) for k in METRICS},
                "relative_move": {k: (None if v is None else round(v, 6))
                                  for k, v in moves.items()},
                "mean_move": (round(sum(applicable) / len(applicable), 6)
                              if applicable else None),
                "ok": bool(measured.get("ok")),
                "error": measured.get("error"),
            })
        moves = [p["mean_move"] for p in points if p["mean_move"] is not None]
        rows.append({
            "target": target,
            "points": points,
            # the headline number: how much this quantity moves the
            # organism, averaged over the sweep
            "sensitivity": (round(sum(moves) / len(moves), 6)
                            if moves else None),
            "max_move": (round(max(moves), 6) if moves else None),
        })

    return {
        "device": device,
        "factors": list(factors),
        "base_organ_size": base_size,
        "baseline": baseline,
        "rows": rows,
        "recommendation": recommend(rows),
    }


def recommend(rows: list[dict], floor: float = 0.2,
              inert_below: float = 0.01) -> dict:
    """Turn measured sensitivity into ``evolution.mutation`` settings.

    Draw weights are proportional to sensitivity with a floor, so an
    insensitive parameter is drawn *less*, never never: M1 §10 asks for
    lower probability, and silently deleting a parameter would be a claim
    about the model rather than a measurement of it. Quantities that
    barely move anything also get a wider scale range, since the sweep
    has just shown that a 5% change is beneath the noise.
    """
    param_rows = [r for r in rows
                  if r["target"] not in ("organ_size", "attachment_weight")]
    sens = {r["target"]: (r["sensitivity"] or 0.0) for r in param_rows}
    top = max(sens.values(), default=0.0)
    weights, scales, inert = {}, {}, []
    for name, value in sens.items():
        weights[name] = round(max(floor, value / top), 4) if top > 0 else 1.0
        if value < inert_below:
            inert.append(name)
            scales[name] = [0.6, 1.4]
    return {
        "parameter_weights": weights,
        "parameter_scale_by_path": scales,
        "insensitive_parameters": inert,
        "structural_sensitivity": {
            r["target"]: r["sensitivity"] for r in rows
            if r["target"] in ("organ_size", "attachment_weight")},
        "note": ("weights are relative to the most sensitive parameter, "
                 f"floored at {floor}; parameters moving the phenotype by "
                 f"less than {inert_below} get a wider scale range instead "
                 "of being removed"),
    }
