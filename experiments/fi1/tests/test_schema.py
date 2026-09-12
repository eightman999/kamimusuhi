"""Envelope schema: checksum binding, canonical serialization, strict parse."""
import json

import pytest

from experiments.fi1.protocol.schema import (DuplicateKeyError, Turn,
                                             canonical_json, compute_checksum,
                                             loads_strict, make_turn,
                                             verify_checksum)


def test_checksum_detects_payload_mutation(make):
    d = make()
    d["payload"]["text"] = "tampered"
    assert not verify_checksum(d)


def test_checksum_detects_metadata_mutation(make):
    d = make()
    d["metadata"]["sender"] = "mallory"
    assert not verify_checksum(d)


@pytest.mark.parametrize("field,value", [
    ("sequence", 999),
    ("timestamp", 42.0),
    ("conversation_id", "other"),
    ("references", ["turn_x"]),
    ("schema_version", 99),
])
def test_checksum_detects_field_mutation(make, field, value):
    d = make()
    d[field] = value
    assert not verify_checksum(d)


def test_roundtrip_and_recompute(make):
    d = make()
    t = Turn.from_dict(d)
    assert t.checksum_valid()
    t2 = Turn.from_dict({**d, "checksum": ""}).recompute_checksum()
    assert t2.checksum == d["checksum"]


def test_canonical_json_key_order():
    a = canonical_json({"x": 1, "y": {"b": 2, "a": 1}})
    b = canonical_json({"y": {"a": 1, "b": 2}, "x": 1})
    assert a == b


def test_checksum_stable_across_serializations(make):
    d = make()
    reparsed = json.loads(json.dumps(d))
    assert compute_checksum(reparsed) == d["checksum"]


def test_loads_strict_rejects_duplicate_keys():
    raw = b'{"a": 1, "b": 2, "a": 3}'
    with pytest.raises(DuplicateKeyError):
        loads_strict(raw)


def test_loads_strict_rejects_non_object():
    with pytest.raises(Exception):
        loads_strict(b"[1,2,3]")


def test_loads_strict_rejects_truncated_json(make):
    d = make()
    raw = json.dumps(d).encode()[:40]
    with pytest.raises(json.JSONDecodeError):
        loads_strict(raw)
