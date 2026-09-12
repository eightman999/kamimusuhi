"""Does a grown circuit actually sit in a path? (M1 §4 / §13, M2)

M0's single structural operator bolted an organ onto one FBA0 region with
a single incoming attachment. Such an organ receives spikes and emits
them into nothing: it costs neurons, edges and memory and cannot affect
anything the evaluation measures. Growing more of those is not evolution,
and rewarding neuron count would select for exactly that.

So M1 classifies every artificial organ by its place in the graph:

``functional``
    reachable **from** an input side and reaching **back into** an
    output side. Whatever it computes can be driven by the environment
    and can change the organism's activity — the only class that can be
    selected *for* its computation.
``neutral_structure``
    has at least one incoming and one outgoing attachment, but is not on
    a source-to-sink path — an island of organs wired to each other, or
    a branch that only ever feeds other dead ends. It runs, it costs
    resources, it cannot matter.
``invalid_structure``
    missing an incoming or an outgoing attachment entirely — the §13
    minimum is not met.
``disabled``
    switched off by DISABLE_ORGAN; not developed at all.

``topology_mode`` picks what "input side" and "output side" mean:

``m1_fba0_loop`` (default, the M1 historical mode)
    source = sink = the FBA0 substrate. ``functional`` is exactly the
    M1 ``FBA0 -> organ -> FBA0`` loop, and only ``fba0`` endpoints are
    external nodes. M1 genomes classify identically under both modes.
``generic_causal`` (M2)
    sources = the genome's enabled substrates + sensor/env endpoints;
    sinks = the genome's enabled substrates + effector/env endpoints.
    ``functional`` means "on a path from environment/input into
    substrate network to behaviourally effective output". Endpoints
    naming a substrate the genome does not carry are dangling.

Nothing here rejects a genome. A mutation that orphans an organ is a real
evolutionary event and is recorded as one; the organism then carries the
cost of dead tissue, which is precisely the pressure §8 asks for. What
the classification forbids is *crediting* it: neuron count alone never
earns fitness.
"""
from __future__ import annotations

from dataclasses import dataclass, field

from ..substrate.endpoints import (EndpointRef, endpoint_key,
                                   parse_endpoint)

FBA0 = "fba0"

#: Structural topology modes. ``m1_fba0_loop`` is the historical M1
#: classification (frozen); ``generic_causal`` is the M2 mode.
TOPOLOGY_M1_FBA0_LOOP = "m1_fba0_loop"
TOPOLOGY_GENERIC_CAUSAL = "generic_causal"
TOPOLOGY_MODES = (TOPOLOGY_M1_FBA0_LOOP, TOPOLOGY_GENERIC_CAUSAL)

# generic-mode endpoint kinds that source/sink paths. Organs are the
# nodes being classified; everything else terminates a path.
_SOURCE_KINDS = frozenset({"substrate", "sensor", "env"})
_SINK_KINDS = frozenset({"substrate", "effector", "env"})
_EXTERNAL_KINDS = _SOURCE_KINDS | _SINK_KINDS


def is_fba0(endpoint: str) -> bool:
    """True when the endpoint names the FBA0 substrate, in either the
    legacy (``fba0``/``fba0:<region>``) or the explicit
    (``substrate:fba0[/<region>]``) spelling."""
    ref = parse_endpoint(endpoint)
    return ref.is_substrate(FBA0)


def _substrate_ids(genome) -> set[str]:
    from ..substrate.registry import substrate_ids_of
    return set(substrate_ids_of(genome))


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
    topology_mode: str = TOPOLOGY_M1_FBA0_LOOP

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
            "topology_mode": self.topology_mode,
        }


def _external(ref: EndpointRef, substrate_ids: set[str],
              topology_mode: str) -> bool:
    """Is this endpoint a node outside the organ graph? In m1 mode only
    the FBA0 substrate counts (the historical rule); in generic mode any
    enabled substrate plus the habitat interfaces (env/sensor/effector)
    count."""
    if topology_mode == TOPOLOGY_M1_FBA0_LOOP:
        return ref.is_substrate(FBA0)
    if ref.kind == "substrate":
        return ref.id in substrate_ids
    return ref.kind in ("env", "sensor", "effector")


def _edges(attachments, live_organs: set[str], substrate_ids: set[str],
           topology_mode: str):
    """Directed endpoint pairs an attachment contributes.

    ``bidirectional`` contributes both directions. Attachments touching a
    pruned or disabled organ — or naming an external endpoint this
    organism does not have — contribute nothing and are reported as
    dangling.
    """
    out, dangling = [], []
    for att in attachments:
        if not getattr(att, "enabled", True):
            continue
        src, tgt = att.source, att.target
        ok = True
        for e in (src, tgt):
            ref = parse_endpoint(e)
            if not (_external(ref, substrate_ids, topology_mode)
                    or ref.id in live_organs):
                ok = False
        if not ok:
            dangling.append(att.attachment_id)
            continue
        a = src if topology_mode == TOPOLOGY_M1_FBA0_LOOP \
            else endpoint_key(src)
        b = tgt if topology_mode == TOPOLOGY_M1_FBA0_LOOP \
            else endpoint_key(tgt)
        out.append((a, b))
        if getattr(att, "direction", "forward") == "bidirectional":
            out.append((b, a))
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


def _sources_sinks(edges, substrate_ids: set[str],
                   topology_mode: str) -> tuple[set[str], set[str]]:
    """The endpoint nodes paths may start from / must reach."""
    if topology_mode == TOPOLOGY_M1_FBA0_LOOP:
        fba0_nodes = {a for a, _ in edges if is_fba0(a)}
        fba0_nodes |= {b for _, b in edges if is_fba0(b)}
        return fba0_nodes, set(fba0_nodes)
    src, sink = set(), set()
    for a, b in edges:
        for node in (a, b):
            ref = parse_endpoint(node)
            if ref.kind == "substrate" and ref.id in substrate_ids:
                src.add(node)
                sink.add(node)
            else:
                # env endpoints are bidirectional habitat ports: they
                # source paths (observation) and sink them (action)
                if ref.kind in _SOURCE_KINDS:
                    src.add(node)
                if ref.kind in _SINK_KINDS:
                    sink.add(node)
    return src, sink


def _classify(organs, attachments, substrate_ids: set[str],
              topology_mode: str) -> StructureReport:
    """Core classification over an explicit organ/attachment list —
    the developed phenotype may differ from the genome when development
    rules ran, so callers pass the list they mean."""
    organs = list(organs)
    live = {o.organ_id for o in organs if getattr(o, "enabled", True)}
    atts = list(attachments)
    edges, dangling = _edges(atts, live, substrate_ids, topology_mode)
    sources, sinks = _sources_sinks(edges, substrate_ids, topology_mode)

    # organs the environment can drive ...
    downstream = _reachable(edges, sources, live)
    # ... and organs that can drive the environment back (walk reversed)
    upstream = _reachable([(b, a) for a, b in edges], sinks, live)

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
        topology_mode=topology_mode,
    )


def analyse(genome, topology_mode: str = TOPOLOGY_M1_FBA0_LOOP,
            _developed=None) -> StructureReport:
    """Classify every artificial organ of ``genome``.

    ``topology_mode`` selects the historical M1 rule
    (``m1_fba0_loop``, the default — M1 results are unchanged) or the
    M2 generic causal rule (``generic_causal``). ``_developed`` is an
    internal ``(organs, attachments)`` override used by development when
    development rules changed the body the genome describes.
    """
    if topology_mode not in TOPOLOGY_MODES:
        raise ValueError(f"unknown topology_mode {topology_mode!r}; "
                         f"expected one of {TOPOLOGY_MODES}")
    organs = list(genome.artificial_organs)
    atts = list(genome.attachments)
    if _developed is not None:
        organs, atts = _developed
    return _classify(organs, atts, _substrate_ids(genome), topology_mode)


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
