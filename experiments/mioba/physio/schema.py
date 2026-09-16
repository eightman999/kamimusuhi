"""A3 physiology parameter schema (§3–§7).

Membrane/channel parameters live in a *physiology overlay*, separate
from the canonical anatomy store (§5): anatomy says what exists, the
overlay says how we currently model it. Every value carries
value/unit/provenance/source/confidence (§3).

Resolution order per entity (§4/§6):

    entity-specific row
      > cell_type profile
      > super_class profile          (coarse class, §39)
      > GENERIC_FALLBACK table
      > UNKNOWN                      (never silently filled)
"""
from __future__ import annotations

from dataclasses import dataclass, field

from ..anatomy.provenance import Provenance

#: parameters every membrane profile may carry (§3)
MEMBRANE_PARAMS = ("Cm", "g_leak", "Ra", "E_leak", "V_rest",
                   "V_threshold", "V_reset")

PARAM_UNITS = {"Cm": "uF/cm^2", "g_leak": "mS/cm^2", "Ra": "ohm*cm",
               "E_leak": "mV", "V_rest": "mV", "V_threshold": "mV",
               "V_reset": "mV", "g_density": "mS/cm^2",
               "E_rev": "mV", "tau": "ms"}

#: coarse profile classes (§39) — MODEL_INFERENCE/LITERATURE_PRIOR
#: priors, not cell-specific measurements
PROFILE_CLASSES = ("generic_excitatory", "generic_inhibitory",
                   "generic_modulatory", "sensory", "descending",
                   "motor")


@dataclass
class ParamSet:
    """A named parameter bag — one overlay row. Each entry is a
    Provenanced-style dict; unknown params stay absent (never 0)."""
    profile_id: str
    scope: str                       # entity|cell_type|super_class|generic
    scope_key: str
    params: dict = field(default_factory=dict)   # name -> record dict
    channels: dict = field(default_factory=dict)  # name -> record dict

    def get(self, name: str):
        return self.params.get(name)


def param(value, unit: str, provenance: Provenance, source: str,
          confidence: str = "low") -> dict:
    return {"value": value, "unit": unit,
            "provenance": Provenance(provenance).value,
            "source": source, "confidence": confidence}


def channel_record(channel_model: str, g_density, e_rev, kinetics: dict,
                   provenance: Provenance, source: str,
                   confidence: str = "low") -> dict:
    return {"channel_model": channel_model, "g_density": g_density,
            "E_rev": e_rev, "kinetics": kinetics,
            "provenance": Provenance(provenance).value,
            "source": source, "confidence": confidence}
