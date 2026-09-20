"""Envelope validation: required blocks, types, schema version, checksum.

A turn that fails validation is never allowed past RECEIVING — it can never
reach READY, so it can never trigger model inference, memory writes or
external side effects.
"""

from __future__ import annotations

from typing import Any, Dict

from .schema import REQUIRED_FIELDS, TURN_SCHEMA_VERSION, verify_checksum


class ValidationError(ValueError):
    """Raised when an envelope fails validation. ``reason`` is a stable code."""

    def __init__(self, reason: str, detail: str = ""):
        super().__init__(f"{reason}: {detail}" if detail else reason)
        self.reason = reason
        self.detail = detail


def _is_int(v: Any) -> bool:
    return isinstance(v, int) and not isinstance(v, bool)


def _is_num(v: Any) -> bool:
    return (isinstance(v, (int, float)) and not isinstance(v, bool))


def validate_envelope(d: Dict[str, Any]) -> None:
    """Validate an already-parsed envelope dict. Raises ValidationError.

    Checks, in order: required-block presence, block types, schema version,
    then checksum self-consistency (detects payload/metadata mutation).
    """
    if not isinstance(d, dict):
        raise ValidationError("not_an_object", type(d).__name__)

    for f in REQUIRED_FIELDS:
        if f not in d or d[f] is None:
            raise ValidationError("missing_field", f)

    if not isinstance(d["turn_id"], str) or not d["turn_id"]:
        raise ValidationError("invalid_type", "turn_id must be non-empty str")
    if not isinstance(d["conversation_id"], str) or not d["conversation_id"]:
        raise ValidationError("invalid_type",
                              "conversation_id must be non-empty str")
    if not _is_int(d["sequence"]) or d["sequence"] < 0:
        raise ValidationError("invalid_type", "sequence must be int >= 0")
    if not _is_num(d["timestamp"]):
        raise ValidationError("invalid_type", "timestamp must be a number")
    if not isinstance(d["payload"], dict):
        raise ValidationError("invalid_type", "payload must be an object")
    if not isinstance(d["metadata"], dict):
        raise ValidationError("invalid_type", "metadata must be an object")
    if not isinstance(d["references"], list):
        raise ValidationError("invalid_type", "references must be a list")
    if not _is_int(d["schema_version"]):
        raise ValidationError("invalid_type", "schema_version must be int")
    if not isinstance(d["checksum"], str):
        raise ValidationError("invalid_type", "checksum must be str")

    if d["schema_version"] != TURN_SCHEMA_VERSION:
        raise ValidationError(
            "unsupported_schema_version",
            f"got {d['schema_version']}, want {TURN_SCHEMA_VERSION}")

    if not verify_checksum(d):
        raise ValidationError("checksum_mismatch",
                              "envelope content does not match checksum")
