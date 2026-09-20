"""Deterministic canonical serialization + simplified text canonicalization.

Two distinct jobs live here:

1. ``canonical_bytes`` / ``canonical_hash`` — serialize a manifest's
   component map to deterministic bytes: fixed (sorted dotted-path)
   field order, fixed separators, UTF-8, explicit format-version header.
   sha256 of those bytes is the ``runtime_identity_hash``.

2. ``canonicalize_text`` — a *simplified standalone* canonicalization for
   prompt text (line-ending normalization, trailing-whitespace strip,
   blank-line collapsing).  Fi0 owns the full canonicalizer; this version
   intentionally only erases byte-level presentation differences, never
   wording differences.  Fi3 treats canonical-equal-but-raw-different
   prompts conservatively (REVIEW minimum, never SAFE).
"""

from __future__ import annotations

import hashlib
import json
from typing import Any, Dict, Mapping

CANONICAL_FORMAT_VERSION = "fi3-canonical-v1"
_HEADER = CANONICAL_FORMAT_VERSION.encode("ascii") + b"\n"


def flatten_components(mapping: Mapping[str, Any], prefix: str = "") -> Dict[str, Any]:
    """Flatten a nested mapping to ``{"a.b.c": value}`` dotted paths.

    Lists are treated as leaf values (serialized via JSON), dicts are
    recursed.  Output order is insertion order; callers that need a fixed
    order should sort the keys.
    """
    out: Dict[str, Any] = {}
    for key, value in mapping.items():
        path = f"{prefix}{key}" if not prefix else f"{prefix}.{key}"
        if isinstance(value, Mapping):
            out.update(flatten_components(value, path))
        else:
            out[path] = value
    return out


def _canonical_scalar(value: Any) -> str:
    """Deterministic JSON encoding of one leaf value.

    ``sort_keys``/``separators``/``ensure_ascii=False`` make the encoding
    byte-stable for nested leaf structures (lists, small dicts); floats
    go through repr-shortest formatting which is deterministic in
    CPython.  ``allow_nan=False`` rejects NaN/inf which have no stable
    cross-runtime encoding.
    """
    return json.dumps(
        value,
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=False,
        allow_nan=False,
    )


def canonical_bytes(components: Mapping[str, Any]) -> bytes:
    """Serialize a component map to canonical bytes.

    Layout: ASCII header line ``fi3-canonical-v1`` then one record per
    flattened field, ``<dotted-key>=<json-value>\n``, keys sorted
    lexicographically (a fixed order independent of construction order).
    """
    flat = flatten_components(components)
    parts = [_HEADER]
    for key in sorted(flat):
        record = f"{key}={_canonical_scalar(flat[key])}\n"
        parts.append(record.encode("utf-8"))
    return b"".join(parts)


def canonical_hash(components: Mapping[str, Any]) -> str:
    """sha256 over :func:`canonical_bytes` — the identity primitive."""
    return hashlib.sha256(canonical_bytes(components)).hexdigest()


def canonicalize_text(text: str) -> str:
    """Whitespace/line-ending canonicalization for prompt text.

    Normalizes, in order:
      * CRLF / CR / U+2028 / U+2029 line endings -> LF
      * trailing spaces/tabs on each line -> stripped
      * leading & trailing blank lines -> removed
      * runs of 2+ blank lines -> single blank line

    Internal spacing and wording are preserved: swapping a word or adding
    a space inside a line still changes the canonical form.  This is a
    deliberately conservative subset of the Fi0 canonicalizer.
    """
    text = text.replace("\r\n", "\n").replace("\r", "\n")
    text = text.replace("\u2028", "\n").replace("\u2029", "\n")
    lines = [ln.rstrip(" \t") for ln in text.split("\n")]
    while lines and lines[0] == "":
        lines.pop(0)
    while lines and lines[-1] == "":
        lines.pop()
    collapsed = []
    prev_blank = False
    for ln in lines:
        if ln == "":
            if prev_blank:
                continue
            prev_blank = True
        else:
            prev_blank = False
        collapsed.append(ln)
    return "\n".join(collapsed)


def canonical_text_hash(text: str) -> str:
    """sha256 of the canonicalized text (UTF-8)."""
    return hashlib.sha256(canonicalize_text(text).encode("utf-8")).hexdigest()
