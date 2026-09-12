"""Canonical serialization + runtime_identity_hash determinism."""

from __future__ import annotations

import hashlib

from experiments.fi3.identity.canonical import (
    canonical_bytes,
    canonical_hash,
    canonicalize_text,
)
from experiments.fi3.identity.manifest import (
    collect_manifest,
    runtime_identity_hash,
)


def test_canonical_bytes_order_independent():
    a = {"x": 1, "y": {"p": "a", "q": 2}}
    b = {"y": {"q": 2, "p": "a"}, "x": 1}
    assert canonical_bytes(a) == canonical_bytes(b)


def test_canonical_bytes_golden():
    # fixed separators, sorted dotted keys, header line
    out = canonical_bytes({"b": 2, "a": {"z": "s"}})
    assert out == (
        b"fi3-canonical-v1\n"
        b'a.z="s"\n'
        b"b=2\n"
    )


def test_canonical_bytes_none_and_bool():
    out = canonical_bytes({"n": None, "t": True, "f": False})
    assert b"n=null" in out
    assert b"t=true" in out and b"f=false" in out


def test_canonical_hash_is_sha256_of_bytes():
    comp = {"a": 1}
    assert canonical_hash(comp) == hashlib.sha256(
        canonical_bytes(comp)
    ).hexdigest()


def test_unicode_stable():
    a = canonical_bytes({"k": "こんにちは"})
    b = canonical_bytes({"k": "こんにちは"})
    assert a == b
    assert "こんにちは".encode() in a


def test_identity_hash_covers_manifest_version(baseline_manifest):
    import copy

    mutant = copy.deepcopy(baseline_manifest)
    mutant["manifest_version"] = 2
    assert runtime_identity_hash(mutant) != runtime_identity_hash(
        baseline_manifest
    )


def test_identity_hash_format(baseline_manifest):
    h = runtime_identity_hash(baseline_manifest)
    assert len(h) == 64
    int(h, 16)  # hex


def test_canonicalize_text_cases():
    base = "line one\nline two\n"
    assert canonicalize_text(base + " ") == canonicalize_text(base)
    assert canonicalize_text(base + "\n\n\n") == canonicalize_text(base)
    assert canonicalize_text("\n\n" + base) == canonicalize_text(base)
    assert canonicalize_text(base.replace("\n", "\r\n")) == canonicalize_text(
        base
    )
    # wording/internal spacing still differ
    assert canonicalize_text("line  one\nline two") != canonicalize_text(base)
    assert canonicalize_text("line oneX\nline two") != canonicalize_text(base)


def test_float_serialization_stable():
    # repr-shortest formatting is deterministic across runs
    assert canonical_bytes({"t": 0.7}) == canonical_bytes({"t": 0.7})
    assert b"t=0.7" in canonical_bytes({"t": 0.7})
