"""Does a grown circuit actually sit in a path? (M1 §4 / §13)

M0's single structural operator bolted an organ onto one FBA0 region with
a single incoming attachment. Such an organ receives spikes and emits
them into nothing: it costs neurons, edges and memory and cannot affect
anything the evaluation measures. Growing more of those is not evolution,
and rewarding neuron count would select for exactly that.

So M1 classifies every artificial organ by its place in the graph:

``functional``
    reachable **from** FBA0 and reaching **back into** FBA0. Whatever it
    computes can be driven by the environment and can change the
    organism's activity — the only class that can be selected *for* its
    computation.
``neutral_structure``
    has at least one incoming and one outgoing attachment, but is not on
    a path between FBA0 and FBA0 — an island of organs wired to each
    other, or a branch that only ever feeds other dead ends. It runs, it
    costs resources, it cannot matter.
``invalid_structure``
    missing an incoming or an outgoing attachment entirely — the §13
    minimum is not met.
``disabled``
    switched off by DISABLE_ORGAN; not developed at all.

Nothing here rejects a genome. A mutation that orphans an organ is a real
evolutionary event and is recorded as one; the organism then carries the
cost of dead tissue, which is precisely the pressure §8 asks for. What
the classification forbids is *crediting* it: neuron count alone never
earns fitness.
"""
from __future__ import annotations

from dataclasses import dataclass, field

FBA0 = "fba0"


def is_fba0(endpoint: str) -> bool:
    return endpoint == FBA0 or endpoint.startswith("fba0:")


@dataclass
class StructureReport:
    organs: dict = field(default_factory=dict)      # organ_id -> class
    counts: dict = field(default_factory=dict)      # class -> n
    n_organs: int = 0
    n_enabled_organs: int = 0
    n_attachments: int = 0
    n_enabled_attachments: int = 0
    n_artificial_neurons: int = 0
    n_functional_neurons: int = 0
    dangling_attachments: list = field(default_factory=list)

    def to_dict(self) -> dict:
        return {
            "organs": dict(self.organs),
            "counts": dict(self.counts),
            "n_organs": self.n_organs,
            "n_enabled_organs": self.n_enabled_organs,
            "n_attachments": self.n_attachments,
            "n_enabled_attachments": self.n_enabled_attachments,
            "n_artificial_neurons": self.n_artificial_neurons,
            "n_functional_neurons": self.n_functional_neurons,
            "dangling_attachments": list(self.dangling_attachments),
            "functional": self.counts.get("functional", 0) > 0,
        }


def _edges(attachments, live_organs: set[str]):
    """Directed endpoint pairs an attachment contributes.

    ``bidirectional`` contributes both directions. Attachments touching a
    pruned or disabled organ contribute nothing and are reported as
    dangling.
    """
    out, dangling = [], []
    for att in attachments:
        if not getattr(att, "enabled", True):
            continue
        src, tgt = att.source, att.target
        ok = ((is_fba0(src) or src in live_organs)
              and (is_fba0(tgt) or tgt in live_organs))
        if not ok:
            dangling.append(att.attachment_id)
            continue
        out.append((src, tgt))
        if getattr(att, "direction", "forward") == "bidirectional":
            out.append((tgt, src))
    return out, dangling


def _reachable(edges, sources, organs) -> set[str]:
    adj: dict[str, list[str]] = {}
    for a, b in edges:
        adj.setdefault(a, []).append(b)
    seen, stack = set(), list(sources)
    while stack:
        node = stack.pop()
        for nxt in adj.get(node, ()):
            if nxt not in seen:
                seen.add(nxt)
                stack.append(nxt)
    return seen & set(organs)


def analyse(genome) -> StructureReport:
    """Classify every artificial organ of ``genome``."""
    organs = list(genome.artificial_organs)
    live = {o.organ_id for o in organs if getattr(o, "enabled", True)}
    atts = list(genome.attachments)
    edges, dangling = _edges(atts, live)

    fba0_nodes = {src for src, _ in edges if is_fba0(src)}
    fba0_nodes |= {tgt for _, tgt in edges if is_fba0(tgt)}
    # organs the environment can drive ...
    downstream = _reachable(edges, fba0_nodes, live)
    # ... and organs that can drive the environment back (walk reversed)
    upstream = _reachable([(b, a) for a, b in edges], fba0_nodes, live)

    incoming = {o: 0 for o in live}
    outgoing = {o: 0 for o in live}
    for a, b in edges:
        if b in incoming:
            incoming[b] += 1
        if a in outgoing:
            outgoing[a] += 1

    classes: dict[str, str] = {}
    for organ in organs:
        oid = organ.organ_id
        if not getattr(organ, "enabled", True):
            classes[oid] = "disabled"
        elif incoming.get(oid, 0) < 1 or outgoing.get(oid, 0) < 1:
            classes[oid] = "invalid_structure"
        elif oid in downstream and oid in upstream:
            classes[oid] = "functional"
        else:
            classes[oid] = "neutral_structure"

    counts: dict[str, int] = {}
    for cls in classes.values():
        counts[cls] = counts.get(cls, 0) + 1
    sizes = {o.organ_id: int(o.size) for o in organs}
    return StructureReport(
        organs=classes,
        counts=counts,
        n_organs=len(organs),
        n_enabled_organs=len(live),
        n_attachments=len(atts),
        n_enabled_attachments=sum(1 for a in atts
                                  if getattr(a, "enabled", True)),
        n_artificial_neurons=sum(sizes[o] for o in live),
        n_functional_neurons=sum(sizes[o] for o, c in classes.items()
                                 if c == "functional"),
        dangling_attachments=dangling,
    )


def limits_report(genome, limits: dict) -> dict:
    """Which configured structural caps this genome is at (M1 §3.1)."""
    rep = analyse(genome)
    return {
        "artificial_neurons": (rep.n_artificial_neurons,
                               int(limits.get("artificial_neurons", 512))),
        "artificial_organs": (rep.n_enabled_organs,
                              int(limits.get("artificial_organs", 8))),
        "attachments": (rep.n_enabled_attachments,
                        int(limits.get("attachments", 24))),
    }
