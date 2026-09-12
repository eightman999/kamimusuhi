"""MIOBA genome schema.

A MIOA genome describes an *artificial organism*: substrate genes (the
inherited tissue the body is built on) plus artificial organs,
attachments and parameter mutations applied on top, and the rules that
shape development and lifetime plasticity.

The genome is not the organism: development turns it into a body, and a
lifetime turns the body into behaviour. Plasticity events are
per-individual runtime state — they are never written back into the
genome and never inherited.

``SCHEMA_VERSION`` below is authoritative (§8: this docstring no longer
carries a frozen literal that drifts):

    1-3  M0/M1 records — no ``substrates`` field
    4    explicit substrate genes + organ IR fields (ports/internal/
         state on organs, signal on attachments) +
         ``source_schema_version`` provenance for migrated records

A v1-v3 document has no ``substrates`` field; loading it assigns the
implicit ancestral FBA0 substrate and records the authored version in
``source_schema_version``. ``genome_id`` is a stored field and is never
recomputed: a migrated record keeps its historical identity even though
its in-memory serialisation now carries more fields.

Genome lineage here is ``genome_lineage`` — it is NOT the Kamimusuhi
canonical identity lineage and never touches kamimusuhi crates/stores.
"""
from __future__ import annotations

import json
from dataclasses import asdict, dataclass, field, fields, is_dataclass
from datetime import datetime, timezone
from typing import Literal

SCHEMA_VERSION = 4

# Name of the ancestral FBA0 reference base (see fba/fba0.py): the
# M-series founder condition and one substrate implementation, not the
# definition of a MIOA organism.
FBA0_BASE_NAME = "flywire-v783-shiu-lif"


def utcnow() -> str:
    return datetime.now(timezone.utc).isoformat()


@dataclass
class OrganProvenance:
    origin: Literal["fba0", "artificial", "substrate", "development"] = \
        "artificial"
    parent_gene: str | None = None
    birth_mutation_id: str | None = None
    ancestry: list[str] = field(default_factory=list)


@dataclass
class SubstrateGene:
    """One substrate implementation the organism is built on (M2 §7).

    ``substrate_id`` names a registered substrate (``fba0`` today);
    ``kind`` is its implementation/dataset identity; ``params`` holds
    per-genome substrate configuration (e.g. a population size when the
    adapter cannot know it). Ordering is the genome's declared order.
    """
    substrate_id: str = "fba0"
    kind: str = FBA0_BASE_NAME
    enabled: bool = True
    params: dict = field(default_factory=dict)


@dataclass
class ArtificialOrgan:
    """A grown component. ``kind`` selects the organ implementation
    (``lif_cluster`` today — a backward-compatible special case of the
    M2 organ IR). ``ports`` declares typed ports, ``internal`` the
    internal topology/config, ``state`` the runtime-state schema —
    runtime *values* live in the lifetime state, never in the genome."""
    organ_id: str
    kind: str
    size: int
    params: dict = field(default_factory=dict)
    provenance: OrganProvenance = field(default_factory=OrganProvenance)
    # DISABLE_ORGAN keeps the organ in the genome (it is part of the
    # lineage and can be re-enabled by a later mutation) but development
    # does not build it, so a disabled organ costs nothing to simulate
    # and contributes nothing. Removal is PRUNE_ORGAN.
    enabled: bool = True
    ports: dict = field(default_factory=dict)
    internal: dict = field(default_factory=dict)
    state: dict = field(default_factory=dict)


@dataclass
class Attachment:
    attachment_id: str
    source: str  # endpoint: "fba0:<region>", "substrate:<id>/<port>", organ_id
    target: str  # organ_id or an external endpoint
    # "forward": source (pre) -> target (post); "bidirectional": both ways
    direction: Literal["forward", "bidirectional"] = "forward"
    weight_scale: float = 1.0
    provenance: OrganProvenance = field(default_factory=OrganProvenance)
    enabled: bool = True
    # the port discipline; M1 edges are all "event" (legacy neural-event
    # port). "continuous"/"signal" exist for M2 transducers/effectors.
    signal: str = "event"


@dataclass
class ParameterMutation:
    mutation_id: str
    path: str
    op: Literal["set", "scale", "add"] = "scale"
    value: float = 1.0
    scope: str = "global"  # "global" | "region:<x>" | "organ:<id>"


@dataclass
class Genome:
    parent_ids: list[str] = field(default_factory=list)
    generation: int = 0
    birth_index: int = 0
    random_seed: int = 0
    genome_id: str = ""  # content hash, filled by finalize()
    schema_version: int = SCHEMA_VERSION
    species_base: str = "fba0"
    ancestral_base: str = FBA0_BASE_NAME
    substrates: list[SubstrateGene] = field(default_factory=list)
    artificial_organs: list[ArtificialOrgan] = field(default_factory=list)
    attachments: list[Attachment] = field(default_factory=list)
    parameter_mutations: list[ParameterMutation] = field(default_factory=list)
    development_rules: list[dict] = field(default_factory=list)
    plasticity_rules: list[dict] = field(default_factory=list)
    # schema version this record was authored in, set when loading a
    # v1-v3 document migrated it; None for natively-authored records
    source_schema_version: int | None = None
    created_at: str = ""

    def to_dict(self) -> dict:
        return asdict(self)

    def to_json(self) -> str:
        return json.dumps(self.to_dict(), sort_keys=True)

    @classmethod
    def from_dict(cls, data: dict) -> "Genome":
        def build(klass, value):
            if is_dataclass(klass):
                kwargs = {}
                for f in fields(klass):
                    if f.name in value:
                        kwargs[f.name] = value[f.name]
                # nested dataclasses
                for name, v in list(kwargs.items()):
                    if name == "provenance" and isinstance(v, dict):
                        kwargs[name] = build(OrganProvenance, v)
                return klass(**kwargs)
            return value

        organs = [build(ArtificialOrgan, o) for o in data.get("artificial_organs", [])]
        atts = [build(Attachment, a) for a in data.get("attachments", [])]
        pms = [build(ParameterMutation, m) for m in data.get("parameter_mutations", [])]
        subs = [build(SubstrateGene, s) for s in data.get("substrates", [])]
        known = {f.name for f in fields(cls)}
        kwargs = {k: v for k, v in data.items() if k in known}
        kwargs["artificial_organs"] = organs
        kwargs["attachments"] = atts
        kwargs["parameter_mutations"] = pms
        kwargs["substrates"] = subs
        g = cls(**kwargs)
        if int(data.get("schema_version") or 0) < 4 and not subs:
            # v1-v3 document: the FBA0 substrate was implied by the
            # architecture itself. Migration spells it out; the record
            # keeps its historical genome_id (a stored field, never
            # recomputed).
            g.substrates = [SubstrateGene(substrate_id="fba0",
                                          kind=FBA0_BASE_NAME)]
            g.source_schema_version = int(data.get("schema_version") or 0)
            g.schema_version = SCHEMA_VERSION
        return g

    @classmethod
    def from_json(cls, text: str) -> "Genome":
        return cls.from_dict(json.loads(text))

    def finalize(self) -> "Genome":
        """Fill created_at (if empty) and genome_id content hash."""
        from .hashing import genome_hash

        if not self.created_at:
            self.created_at = utcnow()
        self.genome_id = genome_hash(self)
        return self

    def provenance(self, birth_mutation_id: str | None = None) -> OrganProvenance:
        return OrganProvenance(
            origin="artificial",
            parent_gene=self.genome_id or None,
            birth_mutation_id=birth_mutation_id,
            ancestry=([self.genome_id] if self.genome_id else []),
        )


def fba0_genome(seed: int = 0) -> Genome:
    """The ancestral founder genome (generation 0, birth_index 0): the
    FBA0 substrate and nothing else. Since schema v4 the substrate is an
    explicit gene; older records carry it implicitly."""
    g = Genome(parent_ids=[], generation=0, birth_index=0, random_seed=seed,
               substrates=[SubstrateGene(substrate_id="fba0",
                                         kind=FBA0_BASE_NAME)])
    return g.finalize()
