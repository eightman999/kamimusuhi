"""Organ IR: the structured representation of a grown component (M2 §11).

An organ is not a bare neuron count. It has a kind (the implementation
contract), a size, parameter overrides, typed *ports* it attaches
through, an internal topology/config, and a runtime-state schema. The
M1 ``lif_cluster`` is the backward-compatible special case: it has the
two event ports M1 wiring always used and no internal structure beyond
"all-to-all-ish cluster".

The IR is *data*: these helpers normalise what is stored on
``ArtificialOrgan`` (``ports``/``internal``/``state`` fields, schema
v4). Runtime values — membrane state, buffers, plasticity adjustments —
live in the lifetime state, never here.
"""
from __future__ import annotations

from ..substrate.base import PORT_SIGNALS

#: Canonical port sets per organ kind. Every organ has an "in" and an
#: "out" event port at minimum — the M1 wiring rule (an organ must sit
#: on a source -> sink path) assumes both exist.
_DEFAULT_PORTS = {
    "lif_cluster": {
        "in": {"direction": "input", "signal": "event"},
        "out": {"direction": "output", "signal": "event"},
    },
}


def organ_ports(organ) -> dict:
    """The organ's declared ports, or its kind's canonical set when the
    organ declares none (every existing lif_cluster genome is the
    canonical case)."""
    declared = getattr(organ, "ports", None) or {}
    if declared:
        return dict(declared)
    return dict(_DEFAULT_PORTS.get(getattr(organ, "kind", ""), {}))


def organ_ir(organ) -> dict:
    """The normalised IR view of one organ (serialisable)."""
    return {
        "organ_id": organ.organ_id,
        "kind": organ.kind,
        "size": int(organ.size),
        "params": dict(getattr(organ, "params", None) or {}),
        "ports": organ_ports(organ),
        "internal": dict(getattr(organ, "internal", None) or {}),
        "state": dict(getattr(organ, "state", None) or {}),
        "enabled": bool(getattr(organ, "enabled", True)),
    }


def attachment_signal(attachment) -> str:
    """The port discipline an attachment carries. Every M1 edge is an
    ``event`` edge (the legacy neural-event port)."""
    return getattr(attachment, "signal", None) or "event"


def signal_known(signal: str) -> bool:
    return signal in PORT_SIGNALS
