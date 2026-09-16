"""Graft schema — the artificial-lobe counterpart to host organs (AFC §8).

A graft is an external, evolvable neural module wired onto the immutable
anatomical host. G0 uses a *fixed* graft (no evolution); the schema is
deliberately evolution-ready — every field listed under §10 is
expressible — but nothing here mutates host anatomy (§7 Immutable Host
Principle).

Grafts compile down to the existing phenotype representation
(``artificial_organs`` + ``attachments``) so the runtime path — backend
extra-neuron allocation, event propagation, ablation via ``enabled`` —
is reused unchanged. What differs is endpoint resolution: a graft's
host-facing endpoints name *anatomical selectors* (neuropil, cell type,
flow class or explicit dataset ids) resolved against the canonical
store, not the synthetic pseudo-regions.
"""
from __future__ import annotations

from dataclasses import dataclass, field, asdict
import hashlib
import json

from ..anatomy.provenance import Provenance


@dataclass
class GraftNeuronModel:
    """Neuron model spec for graft cells. LIF by default; the spec is
    open-ended so conductance-based / RNN / latent modules can be
    declared later without schema churn (§10, §15 G2)."""
    type: str = "lif"            # lif | adaptive | conductance_based | ...
    params: dict = field(default_factory=dict)
    params_provenance: str = Provenance.LITERATURE_PRIOR.value


@dataclass
class HostSelector:
    """An anatomically meaningful port (AFC §11).

    Exactly one selector field should be set. Resolution happens against
    the canonical store — never against dataset-specific code.
    """
    neuropil: str | None = None        # e.g. "central_brain"
    cell_type: str | None = None       # e.g. "DNp32"
    flow_class: str | None = None      # afferent | intrinsic | efferent
    super_class: str | None = None     # e.g. "descending", "sensory"
    dataset_ids: list[str] | None = None  # explicit banc:<id> list
    max_targets: int = 64              # cap on resolved host cells
    seed: int = 0                      # deterministic subsample seed


@dataclass
class GraftLink:
    """One directed host↔graft connection bundle.

    ``direction='in'``:  host population → graft neurons (afferent)
    ``direction='out'``: graft neurons → host population (efferent)
    """
    link_id: str
    direction: str                     # "in" | "out" | "bidirectional"
    host: HostSelector
    graft_fraction: float = 1.0        # share of graft neurons reached
    p: float = 0.5                     # host↔graft edge probability
    weight: float = 1.0                # synaptic weight scale
    provenance: str = Provenance.MODEL_INFERENCE.value


@dataclass
class Graft:
    graft_id: str                      # e.g. "g001" (namespace fills it)
    n_neurons: int
    neuron_model: GraftNeuronModel = field(default_factory=GraftNeuronModel)
    internal_topology: str = "recurrent"   # recurrent | feedforward | none
    internal_p: float = 0.05           # internal edge probability
    links: list[GraftLink] = field(default_factory=list)
    enabled: bool = True               # ablation switch (non-destructive)
    provenance: str = Provenance.MODEL_INFERENCE.value

    def graft_uid(self) -> str:
        return f"graft:{self.graft_id}"

    def neuron_id(self, i: int) -> str:
        from .namespaces import graft_id
        return graft_id(self.graft_id, i)

    def to_json(self) -> str:
        return json.dumps(asdict(self), sort_keys=True)

    def content_hash(self) -> str:
        return hashlib.sha256(self.to_json().encode()).hexdigest()[:16]


@dataclass
class GraftSpec:
    """A collection of grafts applied to one host — the unit an
    evaluation consumes. ``spec_version`` tracks schema revisions."""
    spec_version: str = "graft-v0"
    grafts: list[Graft] = field(default_factory=list)

    def to_json(self) -> str:
        return json.dumps(asdict(self), sort_keys=True)

    @classmethod
    def g0_demo(cls, graft_id: str = "g001", n_neurons: int = 10,
                in_flow_class: str = "afferent",
                out_super_class: str = "descending",
                weight_in: float = 15.0, weight_out: float = 20.0,
                seed: int = 0) -> "GraftSpec":
        """The fixed G0 graft (§15): small lobe tapping the sensory
        afferent stream (an anatomically meaningful 'sensory port',
        AFC §11) and writing onto descending/command-like cells. Both
        directions are exercised so paired ablation can reveal any
        causal effect.

        The afferent default keeps G0 honest: it is the population an
        experiment actually drives, so the graft provably receives the
        host signal rather than waiting on a quiet downstream region.
        ``weight_in`` compensates for the wide loop attenuation — a
        single afferent spike must depolarise a graft LIF cell
        measurably (wScale 0.275mV vs a 7mV v0→vThr gap).
        ``weight_out`` is larger still so that a brief graft burst can
        actually push a host cell over threshold — that is what makes
        graft→host influence observable as host spikes rather than
        only as sub-threshold depolarisation."""
        return cls(grafts=[Graft(
            graft_id=graft_id, n_neurons=n_neurons,
            internal_topology="recurrent", internal_p=0.15,
            links=[
                GraftLink(link_id=f"{graft_id}:in0", direction="in",
                          host=HostSelector(flow_class=in_flow_class,
                                            max_targets=64, seed=seed),
                          graft_fraction=1.0, weight=weight_in),
                GraftLink(link_id=f"{graft_id}:out0", direction="out",
                          host=HostSelector(super_class=out_super_class,
                                            max_targets=64,
                                            seed=seed + 1),
                          graft_fraction=1.0, weight=weight_out),
            ])])
