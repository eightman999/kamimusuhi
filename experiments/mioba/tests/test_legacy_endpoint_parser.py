"""Endpoint references: legacy and M2 spellings parse to the same
EndpointRef, and serialize back without losing information."""
from __future__ import annotations

import pytest

from experiments.mioba.substrate.endpoints import (EndpointRef,
                                                   endpoint_key,
                                                   is_substrate_endpoint,
                                                   parse_endpoint)


@pytest.mark.parametrize("text,kind,ident,port", [
    ("fba0", "substrate", "fba0", None),
    ("fba0:medulla", "substrate", "fba0", "medulla"),
    ("fba0:central_complex", "substrate", "fba0", "central_complex"),
    ("substrate:fba0/medulla", "substrate", "fba0", "medulla"),
    ("substrate:fba0", "substrate", "fba0", None),
    ("substrate:proto0/root", "substrate", "proto0", "root"),
    ("org_abc123", "organ", "org_abc123", None),
    ("organ:org_abc123/output", "organ", "org_abc123", "output"),
    ("sensor:spatial_1/output", "sensor", "spatial_1", "output"),
    ("effector:motor_1/input", "effector", "motor_1", "input"),
    ("env:habitat/forage", "env", "habitat", "forage"),
])
def test_parse_both_spellings(text, kind, ident, port):
    ref = parse_endpoint(text)
    assert (ref.kind, ref.id, ref.port) == (kind, ident, port)
    assert ref.raw == text


@pytest.mark.parametrize("text", ["fba0", "fba0:medulla",
                                  "substrate:fba0",
                                  "substrate:fba0/medulla"])
def test_is_substrate_endpoint_recognises_fba0_in_both_spellings(text):
    assert is_substrate_endpoint(text)
    assert is_substrate_endpoint(text, "fba0")
    assert not is_substrate_endpoint(text, "other")


def test_non_substrate_endpoints_are_not_substrates():
    for text in ("org_a", "organ:org_a/output", "sensor:s/x",
                 "effector:m/in", "env:h/f"):
        assert not is_substrate_endpoint(text)


def test_external_marks_everything_but_organs():
    assert parse_endpoint("fba0:medulla").external
    assert parse_endpoint("sensor:s/x").external
    assert not parse_endpoint("org_a").external
    assert not parse_endpoint("organ:org_a/out").external


def test_legacy_serialization_round_trips():
    for text in ("fba0", "fba0:medulla", "substrate:fba0/medulla",
                 "org_a", "organ:org_a/out", "sensor:s/out",
                 "effector:m/in", "env:h/f"):
        ref = parse_endpoint(text)
        again = parse_endpoint(ref.legacy())
        assert (again.kind, again.id, again.port) == \
            (ref.kind, ref.id, ref.port)


def test_legacy_fba0_spelling_is_preserved():
    """The on-disk M1 spelling never changes."""
    assert parse_endpoint("substrate:fba0/medulla").legacy() == \
        "fba0:medulla"
    assert parse_endpoint("fba0:medulla").legacy() == "fba0:medulla"
    assert parse_endpoint("fba0").legacy() == "fba0"


def test_endpoint_key_unifies_spellings():
    """The two spellings of one port are the same graph node."""
    assert endpoint_key("fba0:medulla") == \
        endpoint_key("substrate:fba0/medulla")
    assert endpoint_key("org_a") == endpoint_key("organ:org_a")
