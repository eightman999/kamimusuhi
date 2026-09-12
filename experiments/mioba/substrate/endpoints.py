"""Generic attachment endpoint references.

A MIOA attachment endpoint names a port on a component of the organism:
a substrate region, an organ, a sensor/transducer, an effector, or the
environment itself. M0/M1 wrote these as plain strings
(``fba0:medulla``, ``org_abc``); this module parses the legacy spellings
and the explicit M2 spellings into a typed :class:`EndpointRef` without
changing the stored strings.

Legacy spellings (still the on-disk format)::

    fba0                  substrate "fba0", whole-population port
    fba0:medulla          substrate "fba0", port "medulla"
    org_abc               organ "org_abc"  (a bare id is an organ)

M2 spellings::

    substrate:fba0/medulla
    organ:org_abc/output
    sensor:spatial_1/output
    effector:motor_1/input
    env:habitat/forage

The architecture-level rule: code that asks "is this endpoint on the
FBA0 substrate" goes through ``parse_endpoint`` (or
``is_substrate_endpoint``); ``endpoint.startswith("fba0:")`` must not
proliferate outside the backend's own endpoint resolver.
"""
from __future__ import annotations

from dataclasses import dataclass

ENDPOINT_KINDS = ("substrate", "organ", "sensor", "effector", "env")
_EXPLICIT_KINDS = frozenset(ENDPOINT_KINDS)

#: The substrate id every M-series lineage starts from.
FOUNDER_SUBSTRATE = "fba0"


@dataclass(frozen=True)
class EndpointRef:
    """A parsed attachment endpoint.

    ``kind`` is one of :data:`ENDPOINT_KINDS`; ``id`` is the component
    name (substrate id, organ id, transducer id); ``port`` is the named
    port on it (``None`` = the whole component). ``raw`` keeps the exact
    string it was parsed from so provenance survives.
    """
    kind: str
    id: str
    port: str | None = None
    raw: str = ""

    @property
    def external(self) -> bool:
        """True when the endpoint is not an organ node: substrates,
        sensors, effectors and environment ports all sit outside the
        organ graph and terminate paths through it."""
        return self.kind != "organ"

    def is_substrate(self, substrate_id: str | None = None) -> bool:
        if self.kind != "substrate":
            return False
        return substrate_id is None or self.id == substrate_id

    def legacy(self) -> str:
        """The canonical stored spelling of this endpoint.

        The FBA0 substrate keeps its M1 spelling (``fba0:<port>``); every
        other component uses the explicit ``<kind>:<id>[/<port>]`` form.
        """
        if self.kind == "substrate" and self.id == FOUNDER_SUBSTRATE:
            return f"{self.id}:{self.port}" if self.port else self.id
        if self.kind == "organ" and self.port is None:
            return self.id
        return (f"{self.kind}:{self.id}/{self.port}" if self.port
                else f"{self.kind}:{self.id}")

    def __str__(self) -> str:
        return self.legacy()


def parse_endpoint(text) -> EndpointRef:
    """Parse a stored endpoint string into an :class:`EndpointRef`.

    Never raises for a non-empty string: anything that is not an
    explicit ``<kind>:`` form and not a legacy ``fba0`` spelling is an
    organ id (M0/M1 wrote bare organ ids).
    """
    s = str(text)
    if s == FOUNDER_SUBSTRATE:
        return EndpointRef("substrate", FOUNDER_SUBSTRATE, None, raw=s)
    if s.startswith(f"{FOUNDER_SUBSTRATE}:"):
        port = s.split(":", 1)[1]
        return EndpointRef("substrate", FOUNDER_SUBSTRATE,
                           port or None, raw=s)
    head, sep, rest = s.partition(":")
    if sep and head in _EXPLICIT_KINDS:
        ident, _, port = rest.partition("/")
        return EndpointRef(head, ident, port or None, raw=s)
    return EndpointRef("organ", s, None, raw=s)


def is_substrate_endpoint(text, substrate_id: str | None = None) -> bool:
    """True when ``text`` names a substrate endpoint (any substrate, or
    the named one). Legacy ``fba0``/``fba0:<region>`` spellings count."""
    return parse_endpoint(text).is_substrate(substrate_id)


def endpoint_key(ref_or_text) -> str:
    """The identity key of an endpoint for graph analysis: the component
    id for organs (organ ports share one node), ``kind:id/port`` for
    external endpoints."""
    ref = (ref_or_text if isinstance(ref_or_text, EndpointRef)
           else parse_endpoint(ref_or_text))
    if ref.kind == "organ":
        return ref.id
    return ref.legacy()
