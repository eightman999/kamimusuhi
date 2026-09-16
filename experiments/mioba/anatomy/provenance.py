"""Provenance tracking for anatomical/physiological values (AFC §5).

Every measured, inferred or assumed quantity carries an explicit
provenance tag. The load-bearing rule of the whole redesign:

    ``UNKNOWN`` is not ``0``.

A value that was never measured is stored as ``value=None`` with
``provenance=UNKNOWN`` — never as a plausible-looking number, and never
silently filled with a generic constant. Downstream code that needs a
number must either map the value through a *versioned* model-inference
table (recording the mapping's own provenance) or refuse the field.
"""
from __future__ import annotations

from dataclasses import dataclass
from enum import Enum


class Provenance(str, Enum):
    """Where a value came from, ordered loosely by evidentiary strength."""

    #: Counted/measured directly in the EM reconstruction (synapse
    #: counts, soma coordinates, skeleton geometry).
    EXACT_EM = "EXACT_EM"
    #: Physiology measured on this very cell or recording site.
    DIRECT_MEASUREMENT = "DIRECT_MEASUREMENT"
    #: Physiology measured on the cell's type/class in this dataset
    #: or a comparable one.
    CELL_TYPE_MEASUREMENT = "CELL_TYPE_MEASUREMENT"
    #: Inferred from transcriptomic/cell-type expression data.
    TRANSCRIPTOMIC_INFERENCE = "TRANSCRIPTOMIC_INFERENCE"
    #: Prior from published literature (not this dataset).
    LITERATURE_PRIOR = "LITERATURE_PRIOR"
    #: Produced by an explicit, versioned model/mapping (e.g.
    #: synapse-count → conductance prior, NT classifiers).
    MODEL_INFERENCE = "MODEL_INFERENCE"
    #: Filled by an imputation rule that is on record.
    IMPUTED = "IMPUTED"
    #: Genuinely absent — the dataset does not carry this value.
    UNKNOWN = "UNKNOWN"


_ORDER = [
    Provenance.DIRECT_MEASUREMENT,
    Provenance.EXACT_EM,
    Provenance.CELL_TYPE_MEASUREMENT,
    Provenance.TRANSCRIPTOMIC_INFERENCE,
    Provenance.LITERATURE_PRIOR,
    Provenance.MODEL_INFERENCE,
    Provenance.IMPUTED,
    Provenance.UNKNOWN,
]


def rank(p: Provenance) -> int:
    """Lower is stronger evidence; used when several candidate sources
    exist for one field (AFC §6.1 priority order)."""
    return _ORDER.index(p)


@dataclass(frozen=True)
class Provenanced:
    """A value plus its provenance and optional source reference.

    Invariants enforced at construction:

    * ``value is None`` ⇒ ``provenance`` must be ``UNKNOWN`` or
      ``IMPUTED``-pending — anything stronger without a value is a lie.
    * ``value is not None`` ⇒ ``provenance`` may not be ``UNKNOWN``.
    """

    value: object
    provenance: Provenance
    unit: str | None = None
    source: str | None = None

    def __post_init__(self):
        if not isinstance(self.provenance, Provenance):
            object.__setattr__(self, "provenance",
                             Provenance(self.provenance))
        if self.value is None and self.provenance not in (
                Provenance.UNKNOWN, Provenance.IMPUTED):
            raise ValueError(
                f"value=None requires provenance UNKNOWN/IMPUTED, "
                f"got {self.provenance}")
        if self.value is not None and self.provenance is Provenance.UNKNOWN:
            raise ValueError("a non-null value cannot claim UNKNOWN "
                             "provenance")

    @property
    def known(self) -> bool:
        return self.value is not None

    def to_dict(self) -> dict:
        return {"value": self.value, "unit": self.unit,
                "provenance": self.provenance.value, "source": self.source}

    @classmethod
    def unknown(cls, unit: str | None = None) -> "Provenanced":
        return cls(value=None, provenance=Provenance.UNKNOWN, unit=unit)
