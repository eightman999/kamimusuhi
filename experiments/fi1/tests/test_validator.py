"""Validator negative tests: every malformed envelope is refused."""
import pytest

from experiments.fi1.protocol.schema import REQUIRED_FIELDS, compute_checksum
from experiments.fi1.protocol.validator import (ValidationError,
                                                validate_envelope)


def test_valid_envelope_passes(make):
    validate_envelope(make())  # no raise


@pytest.mark.parametrize("field", REQUIRED_FIELDS)
def test_each_required_field_missing(make, field):
    d = make()
    del d[field]
    with pytest.raises(ValidationError) as ei:
        validate_envelope(d)
    assert ei.value.reason == "missing_field"


@pytest.mark.parametrize("field", REQUIRED_FIELDS)
def test_each_required_field_null(make, field):
    d = make()
    d[field] = None
    with pytest.raises(ValidationError):
        validate_envelope(d)


def test_missing_metadata_block_rejected(make):
    d = make()
    del d["metadata"]
    with pytest.raises(ValidationError) as ei:
        validate_envelope(d)
    assert ei.value.reason == "missing_field"


def test_metadata_wrong_type_rejected(make):
    d = make()
    d["metadata"] = "sender=user"   # not a dict
    with pytest.raises(ValidationError):
        validate_envelope(d)


def test_bad_checksum_rejected(make):
    d = make()
    d["checksum"] = "f" * 64
    with pytest.raises(ValidationError) as ei:
        validate_envelope(d)
    assert ei.value.reason == "checksum_mismatch"


def test_mutated_payload_stale_checksum_rejected(make):
    d = make()
    d["payload"]["text"] = "changed in transit"
    with pytest.raises(ValidationError) as ei:
        validate_envelope(d)
    assert ei.value.reason == "checksum_mismatch"


def test_unsupported_schema_version(make):
    d = make()
    d["schema_version"] = 999
    d["checksum"] = compute_checksum(d)  # honest recompute, still refused
    with pytest.raises(ValidationError) as ei:
        validate_envelope(d)
    assert ei.value.reason == "unsupported_schema_version"


@pytest.mark.parametrize("seq", [-1, 1.5, "3", True])
def test_bad_sequence(make, seq):
    d = make()
    d["sequence"] = seq
    with pytest.raises(ValidationError):
        validate_envelope(d)


def test_empty_turn_id(make):
    d = make()
    d["turn_id"] = ""
    with pytest.raises(ValidationError):
        validate_envelope(d)
