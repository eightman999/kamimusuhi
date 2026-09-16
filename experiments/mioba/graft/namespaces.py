"""Host/graft ID namespaces (AFC §19).

Host neurons keep their real dataset identity; graft neurons get an
explicitly artificial namespace. The two must never collide and an
artificial neuron must never be issued a BANC/FlyWire-looking id — that
is what lets data, summaries and the Observatory keep host and graft
visually and structurally separate.

    host:  ``banc:720575941556393882``     (dataset prefix : root id)
    graft: ``graft:g001:n00042``           (graft keyword : id : index)
"""
from __future__ import annotations

import re

_GRAFT_RE = re.compile(r"^graft:(?P<graft>[A-Za-z0-9_\-]+):n(?P<n>\d+)$")
_HOST_RE = re.compile(r"^(?P<ds>[a-z][a-z0-9_]*):(?P<rid>[A-Za-z0-9_\-]+)$")


def host_id(dataset: str, root_id) -> str:
    """Canonical host neuron id for a dataset root id."""
    return f"{dataset}:{root_id}"


def graft_id(graft: str, index: int) -> str:
    """Canonical graft neuron id — visibly artificial by construction."""
    return f"graft:{graft}:n{int(index):05d}"


def graft_attachment_id(graft: str, index: int) -> str:
    return f"graft:{graft}:a{int(index):03d}"


def is_graft_id(neuron_id: str) -> bool:
    return bool(_GRAFT_RE.match(neuron_id or ""))


def is_host_id(neuron_id: str) -> bool:
    """A host id is a namespaced dataset id that is not a graft id."""
    return bool(_HOST_RE.match(neuron_id or "")) \
        and not is_graft_id(neuron_id)


def parse_graft_id(neuron_id: str) -> tuple[str, int]:
    m = _GRAFT_RE.match(neuron_id or "")
    if not m:
        raise ValueError(f"not a graft id: {neuron_id!r}")
    return m["graft"], int(m["n"])


def parse_host_id(neuron_id: str) -> tuple[str, str]:
    """Return (dataset, root_id). Rejects graft ids outright."""
    if is_graft_id(neuron_id):
        raise ValueError(f"graft id is not a host id: {neuron_id!r}")
    m = _HOST_RE.match(neuron_id or "")
    if not m:
        raise ValueError(f"not a host id: {neuron_id!r}")
    return m["ds"], m["rid"]
