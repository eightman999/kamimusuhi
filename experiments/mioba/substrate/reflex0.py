"""reflex0: a minimal non-FBA substrate — the M3 abstraction proof.

This adapter exists to prove the substrate contract is real: an
organism built on something other than the FlyWire ancestor must
develop, mutate, lesion, evaluate, reproduce and replay through exactly
the same machinery. Nothing about reflex0 is biologically meaningful —
it is a three-region sensorimotor arc small enough to verify by
inspection:

    sensor ───────────────▶ integrator ───────────────▶ motor
      ▲                       ▲                          │   │
  sensory_in              drive_in                  turn_out forward_out
   (event)              (continuous)                 (event)   (event)

reflex0 carries 48 neurons laid out as contiguous per-region blocks
(sensor 0-15, integrator 16-31, motor 32-47). That layout is the
adapter's own description — ports, regions, lesion masks, the resource
summary — while the *simulation* of a reflex0 phenotype is the
backend's business: the generic CPU mock backend simulates the
population like any other. (A reflex0-native simulator is not part of
the abstraction proof.)

Determinism is load-bearing here exactly as it is for the founder:
``regions()``/``ports()`` return the canonical endpoint order mutation
draws from, and lesion masks come from a dedicated seeded stream —
never a shared RNG.
"""
from __future__ import annotations

import random

from .base import LesionSpec, PortSpec

SUBSTRATE_ID = "reflex0"
KIND = "reflex0-sensorimotor-v0"

#: Anatomical regions in canonical order — the arc the name promises.
#: These are descriptive anatomy (reported by ``describe`` /
#: ``resource_summary``); the attachable namespace is the ports below.
REGIONS = ("sensor", "integrator", "motor")

#: Neurons per region (contiguous blocks in region order).
REGION_SIZES = {"sensor": 16, "integrator": 16, "motor": 16}
N_NEURONS = sum(REGION_SIZES.values())

#: Attachable ports: (name, kind, signal, home region). The typed
#: signals exercise the M2 port disciplines: the drive is a continuous
#: modulatory input, the rest carry spike events. ``regions()``
#: returns the port names — the attachable namespace — so a
#: genome-disabled "region" (the staged M3 operator) removes exactly
#: the endpoint it names.
PORTS = (
    ("sensory_in", "input", "event", "sensor"),
    ("drive_in", "input", "continuous", "integrator"),
    ("turn_out", "output", "event", "motor"),
    ("forward_out", "output", "event", "motor"),
)
PORT_NAMES = tuple(p[0] for p in PORTS)

#: The upstream/downstream split mutation policy wires new organs
#: through (M1 §13): an organ must sit where afferent input can drive
#: it and it can drive an efferent output.
UPSTREAM_PORTS = ("sensory_in", "drive_in")
DOWNSTREAM_PORTS = ("turn_out", "forward_out")

#: The arc's internal wiring — descriptive metadata for the phenotype
#: record (the adapter describes the substrate; simulating it is the
#: backend's business).
INTERNAL_EDGES = (("sensor", "integrator"), ("integrator", "motor"))

#: This substrate's own simulator parameter vocabulary — deliberately
#: *not* the Shiu et al. LIF set. Parameter mutations name the
#: substrate's own paths; paths it does not know resolve to nothing.
DEFAULT_PARAMS = {"dt": 0.1, "gain": 1.0, "threshold": 0.5,
                  "leak": 0.05}


def _endpoint(port: str) -> str:
    """The canonical stored spelling of a reflex0 port. Only the
    founder substrate keeps a legacy (``fba0:<region>``) spelling —
    every other component uses the explicit M2 form."""
    return f"substrate:{SUBSTRATE_ID}/{port}"


class Reflex0Adapter:
    """The SubstrateProtocol implementation for the reflex0 arc."""

    substrate_id = SUBSTRATE_ID
    kind = KIND

    def __init__(self, gene=None):
        # ``gene`` is the genome-layer SubstrateGene record
        # (duck-typed); its params may carry per-genome configuration
        # (``n_neurons``, ``disabled_regions``).
        self.gene = gene

    # ---------------------------------------------------------- identity
    def describe(self) -> dict:
        return {
            "substrate_id": self.substrate_id,
            "kind": self.kind,
            "n_neurons": self.neuron_count(),
            "regions": list(REGIONS),
            "ports": [p.to_dict() for p in self.ports()],
            "port_groups": {"upstream": list(UPSTREAM_PORTS),
                            "downstream": list(DOWNSTREAM_PORTS)},
            "port_regions": {name: region
                             for name, _, _, region in PORTS},
            "internal_edges": [list(e) for e in INTERNAL_EDGES],
            "reference": self.reference(),
        }

    def reference(self) -> dict:
        """The phenotype ``base`` record, shaped like the FBA0
        reference so stored phenotypes keep one schema."""
        return {
            "name": KIND,
            "neurons": self.neuron_count(),
            "source": "mioba synthetic sensorimotor arc "
                      "(M3 abstraction proof)",
            "license_note": "no external data",
            "data_hash": None,
        }

    def resource_summary(self) -> dict:
        """The substrate's static resource footprint."""
        return {
            "n_neurons": self.neuron_count(),
            "n_regions": len(REGIONS),
            "n_ports": len(self.ports()),
            "n_internal_edges": len(INTERNAL_EDGES),
            "region_sizes": dict(REGION_SIZES),
        }

    # ------------------------------------------------------------- ports
    def _disabled_ports(self) -> set:
        """Ports a genome-level substrate lesion removed (the staged M3
        ``DISABLE_SUBSTRATE_REGION`` operator writes
        ``gene.params["disabled_regions"]``; for reflex0 the disable
        namespace is the attachable ports)."""
        return set((getattr(self.gene, "params", None) or {})
                   .get("disabled_regions") or [])

    def ports(self) -> list[PortSpec]:
        disabled = self._disabled_ports()
        return [PortSpec(name=name, kind=kind, signal=signal,
                         endpoint=_endpoint(name))
                for name, kind, signal, _region in PORTS
                if name not in disabled]

    def regions(self) -> list[str]:
        """The attachable port names in canonical order — the namespace
        the substrate-region mutation operators draw from."""
        disabled = self._disabled_ports()
        return [name for name in PORT_NAMES if name not in disabled]

    def port_groups(self) -> dict:
        return {"upstream": list(UPSTREAM_PORTS),
                "downstream": list(DOWNSTREAM_PORTS)}

    # ----------------------------------------------------------- params
    def neuron_count(self) -> int | None:
        params = getattr(self.gene, "params", None) or {}
        return int(params.get("n_neurons", N_NEURONS))

    def default_params(self) -> dict:
        return dict(DEFAULT_PARAMS)

    def resolve_params(self, mutations) -> dict:
        """Global-scope parameter mutations resolved over this
        substrate's own defaults; scoped mutations and unknown paths
        are left for the backend (the founder substrate's rule)."""
        params = dict(DEFAULT_PARAMS)
        for m in mutations or []:
            get = (lambda k: getattr(m, k)) if not isinstance(m, dict) \
                else m.get
            if get("scope") != "global" or get("path") not in params:
                continue
            op, val, path = get("op"), float(get("value")), get("path")
            if op == "scale":
                params[path] *= val
            elif op == "add":
                params[path] += val
            else:
                params[path] = val
        return params

    # ----------------------------------------------------------- lesion
    def lesion(self, severity: float, n_neurons: int,
               seed: int) -> LesionSpec:
        """Uniform random ``severity`` fraction of the substrate's
        population, drawn from a dedicated seeded stream. The neuron
        ids are deterministic given (severity, n_neurons, seed) — the
        same contract every substrate implements."""
        n = max(0, int(n_neurons))
        k = min(n, max(0, int(round(n * float(severity)))))
        ids = sorted(random.Random(int(seed)).sample(range(n), k))
        return LesionSpec(substrate_id=self.substrate_id,
                          severity=float(severity), neuron_ids=ids)

    # -------------------------------------- adapter-level state (none)
    def reset(self, seed: int) -> None:
        del seed                      # the arc has no mutable state

    def snapshot(self) -> dict:
        return {"substrate_id": self.substrate_id, "kind": self.kind}

    def restore(self, snapshot: dict) -> None:
        if (snapshot or {}).get("substrate_id") != self.substrate_id:
            raise ValueError("not a reflex0 substrate snapshot")
