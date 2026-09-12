"""Development rules: deterministic, heritable, applied at birth (M2 §13).

``genome.development_rules`` are genome content — they are hashed,
inherited and replayed like any other gene. What they express is the
split the architecture was missing: the genome is a *recipe*, and
``develop()`` turns the recipe into a body that may differ from the
literal gene list. Genome != mature phenotype.

Two stages, kept strictly apart:

- **birth rules** run here, in declared order, deterministically (no RNG:
  a rule either applies or it is recorded skipped): adding, growing or
  scaling organs, wiring them onto the substrate's ports.
- **lifetime rules** (``GROW_AFTER_N_STEPS``, ``ENABLE_IF_SIGNAL``,
  ``DISABLE_IF_RESOURCE_LOW``, ``DUPLICATE_ON_THRESHOLD``) are *deferred*
  — they describe change during life, so they are handed to the
  plasticity runtime (``development/lifetime.py``), whose state is
  per-individual and never inherited.

Birth-stage ops implemented:

``ADD_ORGAN_AT_BIRTH``
    ``{"op", "kind"?, "size", "organ_id"?, "source"?, "target"?,
       "direction"?, "weight_scale"?}`` — a new organ wired
    ``source -> organ -> target``. Missing endpoints default to the
    primary substrate's first upstream/downstream ports, so a rule that
    only says "grow a 32-neuron organ" produces the §13 in-and-out
    wiring. ``provenance.origin`` is ``development``.
``GROW_ORGAN_AT_BIRTH``
    ``{"op", "organ_id"|"kind", "delta"}`` — size += delta (clamped >=1).
``SCALE_ORGAN_AT_BIRTH``
    ``{"op", "organ_id"|"kind", "factor"}`` — size *= factor.
"""
from __future__ import annotations

from ..genome.schema import ArtificialOrgan, Attachment, OrganProvenance

BIRTH_RULE_OPS = ("ADD_ORGAN_AT_BIRTH", "GROW_ORGAN_AT_BIRTH",
                  "SCALE_ORGAN_AT_BIRTH")
LIFETIME_RULE_OPS = ("GROW_AFTER_N_STEPS", "ENABLE_IF_SIGNAL",
                     "DISABLE_IF_RESOURCE_LOW", "DUPLICATE_ON_THRESHOLD")


def _default_endpoints(adapters) -> tuple[str | None, str | None]:
    """(source, target) for a rule that does not name endpoints: the
    primary substrate's first upstream / downstream ports — the §13
    in-and-out wiring, without drawing randomness."""
    for adapter in adapters or ():
        by_name = {p.name: p.endpoint for p in adapter.ports()}
        groups = adapter.port_groups() or {}
        up = [by_name[n] for n in groups.get("upstream", [])
              if n in by_name] or list(by_name.values())
        down = [by_name[n] for n in groups.get("downstream", [])
                if n in by_name] or list(by_name.values())
        if up and down:
            return up[0], down[0]
    return None, None


def apply_development_rules(genome, organs, attachments, adapters=None):
    """Apply the genome's birth-stage ``development_rules``.

    ``organs``/``attachments`` are the enabled lists development is
    already building; the function returns them extended, plus the
    report and the objects the rules added (so the structure report can
    classify the *developed* body, not just the gene list).
    """
    organs = list(organs)
    attachments = list(attachments)
    report = {"applied": [], "skipped": [], "deferred": [],
              "added_organs": 0, "added_neurons": 0}
    added_organs: list[ArtificialOrgan] = []
    added_atts: list[Attachment] = []
    rules = getattr(genome, "development_rules", None) or []

    known_ids = {o.organ_id for o in organs}
    att_ids = {a.attachment_id for a in genome.attachments} | \
        {a.attachment_id for a in attachments}
    src_default, dst_default = _default_endpoints(adapters)

    for i, rule in enumerate(rules):
        op = (rule or {}).get("op")
        entry = {"index": i, "op": op}
        if op in LIFETIME_RULE_OPS:
            report["deferred"].append(dict(entry, rule=dict(rule)))
            continue
        if op == "ADD_ORGAN_AT_BIRTH":
            oid = rule.get("organ_id") or f"dev_{i}"
            size = int(rule.get("size", 0) or 0)
            src = rule.get("source") or src_default
            tgt = rule.get("target") or dst_default
            if oid in known_ids:
                report["skipped"].append(
                    dict(entry, reason="organ_id_exists"))
                continue
            if size <= 0 or not src or not tgt:
                report["skipped"].append(
                    dict(entry, reason="missing_size_or_endpoints"))
                continue
            organ = ArtificialOrgan(
                organ_id=oid, kind=rule.get("kind", "lif_cluster"),
                size=size, params=dict(rule.get("params") or {}),
                provenance=OrganProvenance(
                    origin="development",
                    parent_gene=genome.genome_id or None,
                    ancestry=([genome.genome_id]
                              if genome.genome_id else [])),
                enabled=True,
                ports=dict(rule.get("ports") or {}),
                internal=dict(rule.get("internal") or {}))
            a_in = Attachment(
                attachment_id=f"att_dev{i}_in", source=src, target=oid,
                direction=rule.get("direction", "forward"),
                weight_scale=float(rule.get("weight_scale", 1.0)),
                signal=rule.get("signal", "event"))
            a_out = Attachment(
                attachment_id=f"att_dev{i}_out", source=oid, target=tgt,
                direction=rule.get("direction", "forward"),
                weight_scale=float(rule.get("weight_scale", 1.0)),
                signal=rule.get("signal", "event"))
            if a_in.attachment_id in att_ids or a_out.attachment_id in att_ids:
                report["skipped"].append(
                    dict(entry, reason="attachment_id_collision"))
                continue
            organs.append(organ)
            attachments += [a_in, a_out]
            added_organs.append(organ)
            added_atts += [a_in, a_out]
            known_ids.add(oid)
            att_ids |= {a_in.attachment_id, a_out.attachment_id}
            report["applied"].append(
                dict(entry, organ_id=oid, size=size,
                     attachments=[a_in.attachment_id,
                                  a_out.attachment_id]))
            report["added_organs"] += 1
            report["added_neurons"] += size
        elif op in ("GROW_ORGAN_AT_BIRTH", "SCALE_ORGAN_AT_BIRTH"):
            target = _find_organ(organs, rule)
            if target is None:
                report["skipped"].append(
                    dict(entry, reason="no_matching_organ"))
                continue
            if op == "GROW_ORGAN_AT_BIRTH":
                delta = int(rule.get("delta", 0) or 0)
                target.size = max(1, int(target.size) + delta)
                report["applied"].append(
                    dict(entry, organ_id=target.organ_id, delta=delta,
                         size=target.size))
                report["added_neurons"] += delta
            else:
                factor = float(rule.get("factor", 1.0))
                new_size = max(1, int(round(int(target.size) * factor)))
                target.size = new_size
                report["applied"].append(
                    dict(entry, organ_id=target.organ_id, factor=factor,
                         size=new_size))
        else:
            report["skipped"].append(dict(entry, reason="unknown_op"))

    return {"organs": organs, "attachments": attachments,
            "added_organs": added_organs,
            "added_attachments": added_atts, "report": report}


def _find_organ(organs, rule):
    """The first enabled organ matching ``organ_id`` or ``kind``."""
    oid = rule.get("organ_id")
    kind = rule.get("kind") if rule.get("match_kind") or \
        not oid else None
    for o in organs:
        if oid and o.organ_id == oid:
            return o
        if kind and o.kind == kind:
            return o
    return None
