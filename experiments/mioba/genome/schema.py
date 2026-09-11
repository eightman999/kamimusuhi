"""MIOBA genome schema (schema_version = 1).

A MIOBA genome describes an *artificial organism*: an immutable FBA0
reference base (the FlyWire v783 connectome LIF model) plus artificial
organs, attachments and parameter mutations applied on top.

Genome lineage here is ``genome_lineage`` — it is NOT the Kamimusuhi
canonical identity lineage and never touches kamimusuhi crates/stores.
"""
from __future__ import annotations

import json
from dataclasses import asdict, dataclass, field, fields, is_dataclass
from datetime import datetime, timezone
from typing import Literal

SCHEMA_VERSION = 3

# Name of the immutable FBA0 reference base (see fba/fba0.py).
FBA0_BASE_NAME = "flywire-v783-shiu-lif"


def utcnow() -> str:
    return datetime.now(timezone.utc).isoformat()


@dataclass
class OrganProvenance:
    origin: Literal["fba0", "artificial"] = "artificial"
    parent_gene: str | None = None
    birth_mutation_id: str | None = None
    ancestry: list[str] = field(default_factory=list)


@dataclass
class ArtificialOrgan:
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


@dataclass
class Attachment:
    attachment_id: str
    source: str  # "fba0:<region-or-celltype>" or an organ_id
    target: str  # organ_id or "fba0:<region>"
    # "forward": source (pre) -> target (post); "bidirectional": both ways
    direction: Literal["forward", "bidirectional"] = "forward"
    weight_scale: float = 1.0
    provenance: OrganProvenance = field(default_factory=OrganProvenance)
    enabled: bool = True


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
    artificial_organs: list[ArtificialOrgan] = field(default_factory=list)
    attachments: list[Attachment] = field(default_factory=list)
    parameter_mutations: list[ParameterMutation] = field(default_factory=list)
    development_rules: list[dict] = field(default_factory=list)
    plasticity_rules: list[dict] = field(default_factory=list)
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
        known = {f.name for f in fields(cls)}
        kwargs = {k: v for k, v in data.items() if k in known}
        kwargs["artificial_organs"] = organs
        kwargs["attachments"] = atts
        kwargs["parameter_mutations"] = pms
        return cls(**kwargs)

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
    """The pure-FBA0 founder genome (generation 0, birth_index 0)."""
    g = Genome(parent_ids=[], generation=0, birth_index=0, random_seed=seed)
    return g.finalize()
