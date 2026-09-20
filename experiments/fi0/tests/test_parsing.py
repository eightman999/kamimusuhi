"""Parser robustness: adversarial inputs, malformed serializations,
format detection, whitespace/position extremes."""

from __future__ import annotations

import pytest

from experiments.fi0.serializer.canonical import (
    PromptParseError,
    canonical_bytes,
    canonicalize,
    detect_format,
    parse_prompt,
)
from experiments.fi0.serializer.schema import PromptContext, Tool
from experiments.fi0.serializer.variants import VariantSpec, render

CTX = PromptContext(
    system="sys text",
    identity="id [K7]",
    memory=("m1", "m2"),
    current_state="battery=12",
    user_input="do it",
    tools=(Tool("recharge", "dock and recharge"),),
    metadata=(("k", "v"),),
)


def test_detect_format():
    assert detect_format('{"SYSTEM": "x"}') == "json"
    assert detect_format("  \n{}") == "json"
    assert detect_format("<<SYSTEM>>\nx\n<<END SYSTEM>>") == "tagged"
    assert detect_format("<prompt><system>x</system></prompt>") == "xml"
    assert detect_format("---\nSYSTEM: x\n") == "yaml"
    assert detect_format("[SYSTEM]\nx\n") == "plain"


def test_parse_rejects_garbage():
    with pytest.raises(PromptParseError):
        parse_prompt("no headers at all")
    with pytest.raises(PromptParseError):
        parse_prompt("{not json")
    with pytest.raises(PromptParseError):
        parse_prompt("<wrongroot></wrongroot>")
    with pytest.raises(PromptParseError):
        parse_prompt("[BOGUS]\nx\n")
    with pytest.raises(PromptParseError):
        parse_prompt("orphan line\n[SYSTEM]\nx\n")


def test_parse_rejects_truncated_variants():
    spec = VariantSpec("sys_mem_user", "xml", "tight", "suffix")
    p = render(spec, CTX)
    with pytest.raises(PromptParseError):
        parse_prompt(p[: len(p) // 2])  # truncated XML
    spec = VariantSpec("sys_mem_user", "json", "tight", "suffix")
    p = render(spec, CTX)
    with pytest.raises(PromptParseError):
        parse_prompt(p[: len(p) // 2])


def test_parse_rejects_unknown_xml_element():
    bad = "<prompt><system>x</system><evil>y</evil></prompt>"
    with pytest.raises(PromptParseError):
        parse_prompt(bad)


def test_parse_rejects_duplicate_block():
    bad = "[SYSTEM]\nx\n[SYSTEM]\ny\n"
    with pytest.raises(PromptParseError):
        parse_prompt(bad)


def test_whitespace_noise_is_tolerated():
    """Leading blank lines / BOM must not break detection or parsing."""
    spec = VariantSpec("sys_mem_user", "plain", "noisy", "prefix")
    p = render(spec, CTX)
    assert parse_prompt("\n\n" + p) == CTX
    assert parse_prompt("\ufeff" + p) == CTX


def test_empty_memory_and_metadata_roundtrip():
    ctx = CTX.mutated(memory=(), metadata=())
    for fmt in ("plain", "json", "yaml", "xml", "tagged"):
        spec = VariantSpec("mem_sys_user", fmt, "spaced", "middle")
        assert parse_prompt(render(spec, ctx)) == ctx


def test_unicode_and_crlf_normalized():
    ctx = CTX.mutated(user_input="café\r\nnaïve  \r\n")
    spec = VariantSpec("sys_mem_user", "plain", "tight", "suffix")
    parsed = parse_prompt(render(spec, ctx))
    assert canonical_bytes(parsed) == canonical_bytes(ctx)
    assert "\r" not in parsed.user_input


def test_multiline_content_roundtrips_all_formats():
    ctx = CTX.mutated(
        system="line one\nline two\nline three",
        memory=("multi\nline\nitem", "simple"),
    )
    for fmt in ("plain", "json", "yaml", "xml", "tagged"):
        for ws in ("tight", "spaced", "noisy"):
            spec = VariantSpec("user_mem_sys", fmt, ws, "prefix")
            parsed = parse_prompt(render(spec, ctx))
            assert canonical_bytes(parsed) == canonical_bytes(ctx), (
                f"{fmt}/{ws} broke multiline round-trip"
            )
