"""Genome mutation operators (M1 §3).

M0 applied one mutation per child, structural with probability 0.2, and
the only structural move was "bolt a new organ onto one FBA0 region".
Over nine generations that moved the phenotype so little that selection
had nothing to act on: fitness spread within a generation was 0.002-0.005
and the best individual never improved on generation 0.

M1 changes three things:

**How many.** ``1 + Poisson(lambda)`` capped, so a child usually carries
two or three changes and occasionally four. One mutation per child on a
32-individual population is simply too slow a random walk.

**What kind.** Nine operators, weighted so that structure dominates:
parameter 30-40%, structural 50-60%, prune/disable ~10%. Pruning is not
an afterthought — without it circuits only ever grow, and "bigger"
becomes indistinguishable from "better".

**Whether it can matter.** Every organ this module creates is wired
``input -> organ -> output`` (M1 §13): NEW_ORGAN and DUPLICATE_ORGAN
never produce a dead-end. Pruning *can* orphan an organ, and that is left
to happen — it is a real evolutionary event, recorded as such by
``genome/structure.py`` — but it is never created on purpose.

Determinism is unchanged from M0 and load-bearing: every choice and every
identifier comes from the caller's ``random.Random``, so the same parent
plus the same RNG state plus the same (generation, birth_index) yields a
byte-identical child and genome hash. The coordinator persists that RNG
state inside the generation transaction.

Each call returns the child *and* a record per attempted mutation,
including the ones that hit a configured cap: "this lineage tried to grow
and was not allowed to" is data, not a non-event.
"""
from __future__ import annotations

import math
import random
from dataclasses import asdict, dataclass, field

from .schema import (SCHEMA_VERSION, ArtificialOrgan, Attachment, Genome,
                     OrganProvenance, ParameterMutation)
from .structure import analyse, is_fba0

# Parameters a mutation may scale (Shiu et al. 2024 LIF names).
MUTABLE_PARAMS = ("wScale", "tauMem", "tauSyn", "vThr", "tRefrac")
FBA0_REGIONS = ("medulla", "lobula", "lobula_plate", "central_complex",
                "mushroom_body", "optic_lobe", "antennal_lobe")

# Regions an organ is wired *from* and *to* by default. Keeping the two
# lists distinct biases new circuits towards sitting on a sensory ->
# motor-ish path rather than looping inside one region, which is what
# makes them able to matter (M1 §13).
UPSTREAM_REGIONS = ("medulla", "lobula", "lobula_plate", "antennal_lobe",
                    "optic_lobe")
DOWNSTREAM_REGIONS = ("central_complex", "mushroom_body", "lobula_plate")

STRUCTURAL_OPERATORS = ("NEW_ORGAN", "GROW_ORGAN", "DUPLICATE_ORGAN",
                        "ADD_ATTACHMENT", "REWIRE_ATTACHMENT",
                        "ADD_INTER_ORGAN_EDGE")
PRUNE_OPERATORS = ("PRUNE_EDGE", "PRUNE_ORGAN", "DISABLE_ORGAN")
PARAMETER_OPERATORS = ("SCALE_PARAMETER",)
ALL_OPERATORS = PARAMETER_OPERATORS + STRUCTURAL_OPERATORS + PRUNE_OPERATORS

DEFAULTS = {
    "count": {"base": 1, "poisson_lambda": 1.5, "cap": 4},
    "weights": {"parameter": 0.35, "structural": 0.55, "prune": 0.10},
    # relative weights inside each category
    "operator_weights": {
        "SCALE_PARAMETER": 1.0,
        "NEW_ORGAN": 1.0, "GROW_ORGAN": 1.0, "DUPLICATE_ORGAN": 0.6,
        "ADD_ATTACHMENT": 1.0, "REWIRE_ATTACHMENT": 0.8,
        "ADD_INTER_ORGAN_EDGE": 0.6,
        "PRUNE_EDGE": 1.0, "PRUNE_ORGAN": 0.5, "DISABLE_ORGAN": 0.5,
    },
    "limits": {"artificial_neurons": 512, "artificial_organs": 8,
               "attachments": 24},
    "new_organ_size": [8, 64],
    "grow_organ_delta": [4, 32],
    "parameter_scale": [0.95, 1.05],
    "attachment_weight_scale": [0.5, 1.5],
    # per-parameter scale ranges, filled in from the §10 sensitivity sweep
    "parameter_scale_by_path": {},
    # An operator drawn for a genome that has nothing to operate on (GROW
    # on an organism with no organs, PRUNE with no attachments) applies
    # nothing. Early generations are mostly such genomes, so without a
    # re-draw roughly half the mutation budget evaporates exactly when
    # structure needs to appear. Re-draws are bounded and every abandoned
    # attempt is still recorded, so the provenance stays complete.
    "retry_no_target": 3,
    # relative probability of picking each parameter; the sweep lowers the
    # ones that do not move the phenotype (M1 §10)
    "parameter_weights": {},
}

OUTCOME_APPLIED = "applied"
OUTCOME_NO_TARGET = "no_target"
OUTCOME_AT_LIMIT = "at_limit"


@dataclass
class MutationRecord:
    """One attempted mutation, applied or not."""
    mutation_id: str
    category: str            # parameter | structural | prune
    operator: str
    outcome: str             # applied | no_target | at_limit
    target: str | None = None
    op: str | None = None
    value: float | None = None
    scope: str = "global"
    detail: dict = field(default_factory=dict)

    def to_dict(self) -> dict:
        return asdict(self)


# see the note in mie/disturbance.py: weight maps are distributions and
# are replaced wholesale rather than merged key by key
_REPLACED_KEYS = ("weights", "parameter_weights")


def merged_config(config: dict | None) -> dict:
    """``evolution.mutation`` merged over the defaults, one level deep."""
    cfg = {k: (dict(v) if isinstance(v, dict) else v)
           for k, v in DEFAULTS.items()}
    user = ((config or {}).get("evolution") or {}).get("mutation") or {}
    for key, value in user.items():
        if (isinstance(value, dict) and isinstance(cfg.get(key), dict)
                and key not in _REPLACED_KEYS):
            cfg[key] = {**cfg[key], **value}
        else:
            cfg[key] = value
    return cfg


def _mid(rng: random.Random) -> str:
    """Deterministic 128-bit identifier derived from the mutation RNG."""
    return f"{rng.getrandbits(128):032x}"[:16]


def _poisson(rng: random.Random, lam: float) -> int:
    """Knuth's method, drawing from the caller's Random so the child stays
    a pure function of the RNG state."""
    if lam <= 0:
        return 0
    target, k, p = math.exp(-lam), 0, 1.0
    while True:
        p *= rng.random()
        if p <= target:
            return k
        k += 1
        if k > 1000:               # numerical guard, never reached in practice
            return k


def mutation_count(rng: random.Random, cfg: dict) -> int:
    c = cfg.get("count") or {}
    base = int(c.get("base", 1))
    lam = float(c.get("poisson_lambda", 1.5))
    cap = int(c.get("cap", 4))
    return max(1, min(cap, base + _poisson(rng, lam)))


def _weighted_choice(rng: random.Random, options: dict):
    """``options`` maps choice -> non-negative weight."""
    items = [(k, float(v)) for k, v in options.items() if float(v) > 0]
    if not items:
        return None
    total = sum(w for _, w in items)
    r = rng.random() * total
    upto = 0.0
    for key, weight in items:
        upto += weight
        if r <= upto:
            return key
    return items[-1][0]


def choose_operator(rng: random.Random, cfg: dict) -> tuple[str, str]:
    """(category, operator) under the configured category weights."""
    category = _weighted_choice(rng, cfg.get("weights") or {}) or "parameter"
    pool = {"parameter": PARAMETER_OPERATORS,
            "structural": STRUCTURAL_OPERATORS,
            "prune": PRUNE_OPERATORS}[category]
    weights = cfg.get("operator_weights") or {}
    operator = _weighted_choice(rng, {op: weights.get(op, 1.0)
                                      for op in pool}) or pool[0]
    return category, operator


# --------------------------------------------------------------- helpers
def _live_organs(genome: Genome):
    return [o for o in genome.artificial_organs if o.enabled]


def _live_attachments(genome: Genome):
    return [a for a in genome.attachments if a.enabled]


def _neuron_total(genome: Genome) -> int:
    return sum(int(o.size) for o in _live_organs(genome))


def _at_limit(genome: Genome, limits: dict, *, extra_neurons: int = 0,
              extra_organs: int = 0, extra_attachments: int = 0) -> str | None:
    if (_neuron_total(genome) + extra_neurons
            > int(limits.get("artificial_neurons", 512))):
        return "artificial_neurons"
    if (len(_live_organs(genome)) + extra_organs
            > int(limits.get("artificial_organs", 8))):
        return "artificial_organs"
    if (len(_live_attachments(genome)) + extra_attachments
            > int(limits.get("attachments", 24))):
        return "attachments"
    return None


def _attach(genome: Genome, rng: random.Random, prov: OrganProvenance,
            source: str, target: str, cfg: dict,
            direction: str = "forward") -> Attachment:
    lo, hi = cfg.get("attachment_weight_scale", [0.5, 1.5])
    att = Attachment(attachment_id=f"att_{_mid(rng)[:8]}", source=source,
                     target=target, direction=direction,
                     weight_scale=round(rng.uniform(float(lo), float(hi)), 4),
                     provenance=prov)
    genome.attachments.append(att)
    return att


def _endpoints(genome: Genome, rng: random.Random, exclude: str | None = None):
    """Every endpoint an attachment may name: FBA0 regions plus live
    organs (minus ``exclude``)."""
    names = [f"fba0:{r}" for r in FBA0_REGIONS]
    names += [o.organ_id for o in _live_organs(genome)
              if o.organ_id != exclude]
    return names


# ------------------------------------------------------------- operators
def _op_scale_parameter(genome, rng, cfg, prov, mid) -> MutationRecord:
    weights = cfg.get("parameter_weights") or {}
    path = (_weighted_choice(rng, {p: weights.get(p, 1.0)
                                   for p in MUTABLE_PARAMS})
            or rng.choice(MUTABLE_PARAMS))
    by_path = cfg.get("parameter_scale_by_path") or {}
    lo, hi = by_path.get(path, cfg.get("parameter_scale", [0.95, 1.05]))
    value = round(rng.uniform(float(lo), float(hi)), 4)
    genome.parameter_mutations.append(ParameterMutation(
        mutation_id=mid, path=path, op="scale", value=value, scope="global"))
    return MutationRecord(mid, "parameter", "SCALE_PARAMETER",
                          OUTCOME_APPLIED, target=path, op="scale",
                          value=value)


def _op_new_organ(genome, rng, cfg, prov, mid) -> MutationRecord:
    lo, hi = cfg.get("new_organ_size", [8, 64])
    size = rng.randint(int(lo), int(hi))
    limits = cfg.get("limits") or {}
    hit = _at_limit(genome, limits, extra_neurons=size, extra_organs=1,
                    extra_attachments=2)
    if hit:
        return MutationRecord(mid, "structural", "NEW_ORGAN", OUTCOME_AT_LIMIT,
                              op="add", value=float(size),
                              detail={"limit": hit})
    organ = ArtificialOrgan(organ_id=f"org_{_mid(rng)[:8]}", kind="lif_cluster",
                            size=size, params={}, provenance=prov)
    genome.artificial_organs.append(organ)
    # §13: wired in and out at birth, on an upstream -> downstream path
    src = f"fba0:{rng.choice(UPSTREAM_REGIONS)}"
    dst = f"fba0:{rng.choice(DOWNSTREAM_REGIONS)}"
    a_in = _attach(genome, rng, prov, src, organ.organ_id, cfg)
    a_out = _attach(genome, rng, prov, organ.organ_id, dst, cfg)
    return MutationRecord(mid, "structural", "NEW_ORGAN", OUTCOME_APPLIED,
                          target=organ.organ_id, op="add", value=float(size),
                          scope=f"organ:{organ.organ_id}",
                          detail={"size": size, "source": src, "target": dst,
                                  "attachments": [a_in.attachment_id,
                                                  a_out.attachment_id]})


def _op_grow_organ(genome, rng, cfg, prov, mid) -> MutationRecord:
    organs = _live_organs(genome)
    if not organs:
        return MutationRecord(mid, "structural", "GROW_ORGAN",
                              OUTCOME_NO_TARGET, op="grow")
    organ = rng.choice(organs)
    lo, hi = cfg.get("grow_organ_delta", [4, 32])
    delta = rng.randint(int(lo), int(hi))
    hit = _at_limit(genome, cfg.get("limits") or {}, extra_neurons=delta)
    if hit:
        return MutationRecord(mid, "structural", "GROW_ORGAN",
                              OUTCOME_AT_LIMIT, target=organ.organ_id,
                              op="grow", value=float(delta),
                              detail={"limit": hit})
    organ.size = int(organ.size) + delta
    return MutationRecord(mid, "structural", "GROW_ORGAN", OUTCOME_APPLIED,
                          target=organ.organ_id, op="grow", value=float(delta),
                          scope=f"organ:{organ.organ_id}",
                          detail={"delta": delta, "size": organ.size})


def _op_duplicate_organ(genome, rng, cfg, prov, mid) -> MutationRecord:
    organs = _live_organs(genome)
    if not organs:
        return MutationRecord(mid, "structural", "DUPLICATE_ORGAN",
                              OUTCOME_NO_TARGET, op="duplicate")
    src_organ = rng.choice(organs)
    size = int(src_organ.size)
    # the copy inherits its parent's attachment pattern, so it is wired
    # in and out exactly as the original is
    pattern = [a for a in _live_attachments(genome)
               if src_organ.organ_id in (a.source, a.target)]
    hit = _at_limit(genome, cfg.get("limits") or {}, extra_neurons=size,
                    extra_organs=1, extra_attachments=len(pattern))
    if hit:
        return MutationRecord(mid, "structural", "DUPLICATE_ORGAN",
                              OUTCOME_AT_LIMIT, target=src_organ.organ_id,
                              op="duplicate", value=float(size),
                              detail={"limit": hit})
    copy = ArtificialOrgan(organ_id=f"org_{_mid(rng)[:8]}",
                           kind=src_organ.kind, size=size,
                           params=dict(src_organ.params),
                           provenance=OrganProvenance(
                               origin="artificial",
                               parent_gene=prov.parent_gene,
                               birth_mutation_id=mid,
                               ancestry=list(prov.ancestry)
                               + [src_organ.organ_id]))
    genome.artificial_organs.append(copy)
    made = []
    for att in pattern:
        src = copy.organ_id if att.source == src_organ.organ_id else att.source
        tgt = copy.organ_id if att.target == src_organ.organ_id else att.target
        made.append(_attach(genome, rng, prov, src, tgt, cfg,
                            direction=att.direction).attachment_id)
    return MutationRecord(mid, "structural", "DUPLICATE_ORGAN",
                          OUTCOME_APPLIED, target=copy.organ_id,
                          op="duplicate", value=float(size),
                          scope=f"organ:{copy.organ_id}",
                          detail={"copied_from": src_organ.organ_id,
                                  "attachments": made})


def _op_add_attachment(genome, rng, cfg, prov, mid) -> MutationRecord:
    if not _live_organs(genome):
        return MutationRecord(mid, "structural", "ADD_ATTACHMENT",
                              OUTCOME_NO_TARGET, op="add")
    hit = _at_limit(genome, cfg.get("limits") or {}, extra_attachments=1)
    if hit:
        return MutationRecord(mid, "structural", "ADD_ATTACHMENT",
                              OUTCOME_AT_LIMIT, op="add", detail={"limit": hit})
    organ = rng.choice(_live_organs(genome))
    other = rng.choice(_endpoints(genome, rng, exclude=organ.organ_id))
    if rng.random() < 0.5:
        src, tgt = other, organ.organ_id
    else:
        src, tgt = organ.organ_id, other
    att = _attach(genome, rng, prov, src, tgt, cfg)
    return MutationRecord(mid, "structural", "ADD_ATTACHMENT",
                          OUTCOME_APPLIED, target=att.attachment_id, op="add",
                          value=att.weight_scale,
                          detail={"source": src, "target": tgt})


def _op_rewire_attachment(genome, rng, cfg, prov, mid) -> MutationRecord:
    atts = _live_attachments(genome)
    if not atts:
        return MutationRecord(mid, "structural", "REWIRE_ATTACHMENT",
                              OUTCOME_NO_TARGET, op="rewire")
    att = rng.choice(atts)
    before = {"source": att.source, "target": att.target}
    move_source = rng.random() < 0.5
    fixed = att.target if move_source else att.source
    choices = [e for e in _endpoints(genome, rng) if e != fixed]
    if not choices:
        return MutationRecord(mid, "structural", "REWIRE_ATTACHMENT",
                              OUTCOME_NO_TARGET, target=att.attachment_id,
                              op="rewire")
    new = rng.choice(choices)
    if move_source:
        att.source = new
    else:
        att.target = new
    return MutationRecord(mid, "structural", "REWIRE_ATTACHMENT",
                          OUTCOME_APPLIED, target=att.attachment_id,
                          op="rewire",
                          detail={"before": before,
                                  "after": {"source": att.source,
                                            "target": att.target}})


def _op_add_inter_organ_edge(genome, rng, cfg, prov, mid) -> MutationRecord:
    organs = _live_organs(genome)
    if len(organs) < 2:
        return MutationRecord(mid, "structural", "ADD_INTER_ORGAN_EDGE",
                              OUTCOME_NO_TARGET, op="add",
                              detail={"live_organs": len(organs)})
    hit = _at_limit(genome, cfg.get("limits") or {}, extra_attachments=1)
    if hit:
        return MutationRecord(mid, "structural", "ADD_INTER_ORGAN_EDGE",
                              OUTCOME_AT_LIMIT, op="add", detail={"limit": hit})
    a, b = rng.sample([o.organ_id for o in organs], 2)
    att = _attach(genome, rng, prov, a, b, cfg)
    return MutationRecord(mid, "structural", "ADD_INTER_ORGAN_EDGE",
                          OUTCOME_APPLIED, target=att.attachment_id, op="add",
                          value=att.weight_scale,
                          detail={"source": a, "target": b})


def _op_prune_edge(genome, rng, cfg, prov, mid) -> MutationRecord:
    atts = _live_attachments(genome)
    if not atts:
        return MutationRecord(mid, "prune", "PRUNE_EDGE", OUTCOME_NO_TARGET,
                              op="remove")
    att = rng.choice(atts)
    genome.attachments = [a for a in genome.attachments
                          if a.attachment_id != att.attachment_id]
    return MutationRecord(mid, "prune", "PRUNE_EDGE", OUTCOME_APPLIED,
                          target=att.attachment_id, op="remove",
                          detail={"source": att.source, "target": att.target})


def _op_prune_organ(genome, rng, cfg, prov, mid) -> MutationRecord:
    organs = _live_organs(genome)
    if not organs:
        return MutationRecord(mid, "prune", "PRUNE_ORGAN", OUTCOME_NO_TARGET,
                              op="remove")
    organ = rng.choice(organs)
    removed = [a.attachment_id for a in genome.attachments
               if organ.organ_id in (a.source, a.target)]
    genome.artificial_organs = [o for o in genome.artificial_organs
                                if o.organ_id != organ.organ_id]
    genome.attachments = [a for a in genome.attachments
                          if organ.organ_id not in (a.source, a.target)]
    return MutationRecord(mid, "prune", "PRUNE_ORGAN", OUTCOME_APPLIED,
                          target=organ.organ_id, op="remove",
                          value=float(organ.size),
                          detail={"size": organ.size,
                                  "removed_attachments": removed})


def _op_disable_organ(genome, rng, cfg, prov, mid) -> MutationRecord:
    organs = _live_organs(genome)
    if not organs:
        return MutationRecord(mid, "prune", "DISABLE_ORGAN", OUTCOME_NO_TARGET,
                              op="disable")
    organ = rng.choice(organs)
    organ.enabled = False
    return MutationRecord(mid, "prune", "DISABLE_ORGAN", OUTCOME_APPLIED,
                          target=organ.organ_id, op="disable",
                          value=float(organ.size),
                          detail={"size": organ.size})


_OPERATORS = {
    "SCALE_PARAMETER": _op_scale_parameter,
    "NEW_ORGAN": _op_new_organ,
    "GROW_ORGAN": _op_grow_organ,
    "DUPLICATE_ORGAN": _op_duplicate_organ,
    "ADD_ATTACHMENT": _op_add_attachment,
    "REWIRE_ATTACHMENT": _op_rewire_attachment,
    "ADD_INTER_ORGAN_EDGE": _op_add_inter_organ_edge,
    "PRUNE_EDGE": _op_prune_edge,
    "PRUNE_ORGAN": _op_prune_organ,
    "DISABLE_ORGAN": _op_disable_organ,
}


def apply_operator(genome: Genome, rng: random.Random, operator: str,
                   cfg: dict, prov: OrganProvenance,
                   mutation_id: str | None = None) -> MutationRecord:
    """Apply one named operator in place. Public for the §10 sweep."""
    fn = _OPERATORS.get(operator)
    if fn is None:
        raise ValueError(f"unknown mutation operator {operator!r}")
    return fn(genome, rng, cfg, prov, mutation_id or _mid(rng))


def mutate(parent: Genome, rng: random.Random, birth_index: int,
           generation: int, config: dict | None = None
           ) -> tuple[Genome, list[MutationRecord]]:
    """Produce one child of ``parent`` and the record of what was tried.

    Returns ``(child, records)``. ``records`` includes attempts that hit a
    cap or found no target; the child is finalised (hashed) once, after
    every operator has run.
    """
    cfg = merged_config(config)
    child = Genome.from_dict(parent.to_dict())
    child.genome_id = ""
    child.created_at = ""
    child.parent_ids = [parent.genome_id] if parent.genome_id else []
    child.generation = generation
    child.birth_index = birth_index
    child.random_seed = rng.randrange(2**31)
    # a child is a new record authored under the current schema — it does
    # not inherit the parent's migration provenance
    child.source_schema_version = None
    child.schema_version = SCHEMA_VERSION

    n = mutation_count(rng, cfg)
    retries = int(cfg.get("retry_no_target", 0) or 0)
    records: list[MutationRecord] = []
    for _ in range(n):
        for attempt in range(retries + 1):
            mid = _mid(rng)
            prov = parent.provenance(birth_mutation_id=mid)
            _category, operator = choose_operator(rng, cfg)
            rec = apply_operator(child, rng, operator, cfg, prov, mid)
            records.append(rec)
            if rec.outcome != OUTCOME_NO_TARGET or attempt == retries:
                break
            # nothing to operate on: draw again rather than spend a
            # mutation on a genome that cannot receive it
            rec.detail = dict(rec.detail, redrawn=True)
    return child.finalize(), records


def mutate_child(parent: Genome, rng: random.Random, birth_index: int,
                 generation: int, config: dict | None = None) -> Genome:
    """``mutate`` without the records — kept for callers that only want
    the child (tests, the §10 sweep helpers)."""
    return mutate(parent, rng, birth_index, generation, config)[0]


def structural_summary(genome: Genome,
                       records: list[MutationRecord] | None = None) -> dict:
    """Structure classification plus, if given, what this birth tried."""
    rep = analyse(genome).to_dict()
    if records is not None:
        rep["mutations"] = [r.to_dict() for r in records]
        rep["applied"] = sum(1 for r in records if r.outcome == OUTCOME_APPLIED)
        rep["at_limit"] = sum(1 for r in records
                              if r.outcome == OUTCOME_AT_LIMIT)
        rep["no_target"] = sum(1 for r in records
                               if r.outcome == OUTCOME_NO_TARGET)
    return rep


__all__ = ["ALL_OPERATORS", "DEFAULTS", "FBA0_REGIONS", "MUTABLE_PARAMS",
           "MutationRecord", "PARAMETER_OPERATORS", "PRUNE_OPERATORS",
           "STRUCTURAL_OPERATORS", "apply_operator", "choose_operator",
           "is_fba0", "merged_config", "mutate", "mutate_child",
           "mutation_count", "structural_summary"]
