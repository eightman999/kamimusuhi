"""Canonical serializer and universal prompt parser.

Two halves:

1. ``canonical_bytes(ctx)`` — emit the canonical serialization of a
   :class:`PromptContext`.  The canonical format is frozen:

   * version: ``FI0-CANON-v1`` (bump on any format change)
   * field order: ``sort_keys=True`` over the fixed block set
   * metadata schema: keys sorted, values as strings
   * escaping: ``json.dumps`` with ``ensure_ascii=True``
   * separators: ``(",", ":")`` — no incidental whitespace
   * encoding: UTF-8

2. ``parse_prompt(text)`` — parse *any* variant serialization produced by
   ``serializer/variants.py`` (plain / JSON / YAML / XML-like / tagged, any
   block order, any metadata position, any whitespace style, and the
   canonical format itself) back into a :class:`PromptContext`.

   Critical property under test:
   ``canonical_bytes(parse_prompt(variant_i)) ==
    canonical_bytes(parse_prompt(variant_j))`` for all i, j over the same
   semantic input.
"""

from __future__ import annotations

import json
import re
import xml.etree.ElementTree as ET
from typing import Dict, List

import yaml

from .schema import BLOCKS, PromptContext

CANONICAL_VERSION = "FI0-CANON-v1"
CANONICAL_HEADER = f'"version":"{CANONICAL_VERSION}"'


class PromptParseError(ValueError):
    """Raised when a serialized prompt cannot be mapped back to blocks."""


# ---------------------------------------------------------------------------
# Canonical serialization
# ---------------------------------------------------------------------------


def canonical_object(ctx: PromptContext) -> dict:
    """The canonical JSON-able object for a context."""
    return {
        "version": CANONICAL_VERSION,
        "blocks": {
            "SYSTEM": ctx.system,
            "IDENTITY": ctx.identity,
            "MEMORY": list(ctx.memory),
            "CURRENT_STATE": ctx.current_state,
            "USER_INPUT": ctx.user_input,
            "TOOLS": [
                {"description": t.description, "name": t.name} for t in ctx.tools
            ],
            "METADATA": ctx.metadata_dict(),
        },
    }


def canonical_bytes(ctx: PromptContext) -> bytes:
    """Deterministic canonical serialization.

    ``json.dumps(sort_keys=True, separators=(",", ":"),
    ensure_ascii=True)`` guarantees: fixed field order, fixed metadata key
    order, fixed escaping, fixed separators, fixed encoding.
    """
    return json.dumps(
        canonical_object(ctx),
        sort_keys=True,
        separators=(",", ":"),
        ensure_ascii=True,
    ).encode("utf-8")


def canonical_text(ctx: PromptContext) -> str:
    return canonical_bytes(ctx).decode("utf-8")


# ---------------------------------------------------------------------------
# Format detection
# ---------------------------------------------------------------------------


def detect_format(text: str) -> str:
    """Classify a serialized prompt into a format tag.

    Order matters: every variant serializer emits an unambiguous leading
    signature — ``{`` JSON, ``<prompt`` XML, ``<<`` tagged, ``---`` YAML
    document marker; anything else falls back to the plain ``[BLOCK]``
    section format.
    """
    s = text.lstrip("\ufeff").lstrip()
    if s.startswith("{"):
        return "json"
    if s.startswith("<<"):
        return "tagged"
    if s.startswith("<prompt"):
        return "xml"
    if s.startswith("---"):
        return "yaml"
    return "plain"


# ---------------------------------------------------------------------------
# Per-format parsers -> {BLOCK: payload}
# ---------------------------------------------------------------------------

_KNOWN_BLOCKS = set(BLOCKS)


def _check_blocks(m: Dict[str, object], where: str) -> Dict[str, object]:
    bad = set(m) - _KNOWN_BLOCKS
    if bad:
        raise PromptParseError(f"{where}: unknown blocks {sorted(bad)}")
    return m


def _parse_json(text: str) -> Dict[str, object]:
    try:
        obj = json.loads(text)
    except json.JSONDecodeError as e:
        raise PromptParseError(f"json: {e}") from e
    if not isinstance(obj, dict):
        raise PromptParseError("json: top level must be an object")
    if obj.get("version") == CANONICAL_VERSION:
        blocks = obj.get("blocks")
        if not isinstance(blocks, dict):
            raise PromptParseError("canonical json: missing 'blocks'")
        return _check_blocks(blocks, "canonical json")
    return _check_blocks(obj, "json")


def _parse_yaml(text: str) -> Dict[str, object]:
    try:
        obj = yaml.safe_load(text)
    except yaml.YAMLError as e:
        raise PromptParseError(f"yaml: {e}") from e
    if not isinstance(obj, dict):
        raise PromptParseError("yaml: top level must be a mapping")
    return _check_blocks({str(k): v for k, v in obj.items()}, "yaml")


def _parse_xml(text: str) -> Dict[str, object]:
    try:
        root = ET.fromstring(text)
    except ET.ParseError as e:
        raise PromptParseError(f"xml: {e}") from e
    if root.tag != "prompt":
        raise PromptParseError(f"xml: root must be <prompt>, got <{root.tag}>")
    out: Dict[str, object] = {}
    tag2block = {
        "system": "SYSTEM",
        "identity": "IDENTITY",
        "memory": "MEMORY",
        "current_state": "CURRENT_STATE",
        "user_input": "USER_INPUT",
        "tools": "TOOLS",
        "metadata": "METADATA",
    }
    for child in root:
        block = tag2block.get(child.tag)
        if block is None:
            raise PromptParseError(f"xml: unknown element <{child.tag}>")
        if block in out:
            raise PromptParseError(f"xml: duplicate block <{child.tag}>")
        if block == "MEMORY":
            out[block] = [(it.text or "") for it in child.findall("item")]
        elif block == "TOOLS":
            tools = []
            for t in child.findall("tool"):
                name = t.get("name")
                if name is None:
                    raise PromptParseError("xml: <tool> missing name attr")
                tools.append({"name": name, "description": t.text or ""})
            out[block] = tools
        elif block == "METADATA":
            meta = {}
            for mnode in child.findall("meta"):
                k = mnode.get("key")
                if k is None:
                    raise PromptParseError("xml: <meta> missing key attr")
                meta[k] = mnode.text or ""
            out[block] = meta
        else:
            out[block] = child.text or ""
    return out


_TAGGED_RE = re.compile(r"<<([A-Z_]+)>>\n?(.*?)<<END \1>>", re.DOTALL)


def _parse_tagged(text: str) -> Dict[str, object]:
    out: Dict[str, object] = {}
    for name, body in _TAGGED_RE.findall(text):
        if name not in _KNOWN_BLOCKS:
            raise PromptParseError(f"tagged: unknown block <<{name}>>")
        if name in out:
            raise PromptParseError(f"tagged: duplicate block <<{name}>>")
        body = body.strip("\n")
        if name == "MEMORY":
            items = [i.strip() for i in re.split(r"\n--\n", body)] if body else []
            out[name] = [i for i in items if i]
        elif name == "TOOLS":
            tools = []
            for line in body.split("\n"):
                line = line.strip()
                if not line:
                    continue
                if "::" not in line:
                    raise PromptParseError(
                        f"tagged: TOOLS line lacks '::' separator: {line!r}"
                    )
                nm, desc = line.split("::", 1)
                tools.append({"name": nm.strip(), "description": desc.strip()})
            out[name] = tools
        elif name == "METADATA":
            meta = {}
            for line in body.split("\n"):
                line = line.strip()
                if not line:
                    continue
                if ":" not in line:
                    raise PromptParseError(
                        f"tagged: METADATA line lacks ':' separator: {line!r}"
                    )
                k, v = line.split(":", 1)
                meta[k.strip()] = v.strip()
            out[name] = meta
        else:
            out[name] = body
    if not out:
        raise PromptParseError("tagged: no <<BLOCK>> sections found")
    return out


_PLAIN_HEADER_RE = re.compile(r"^\[\s*([A-Z_]+)\s*\]\s*$")


def _plain_block_payload(name: str, lines: List[str]) -> object:
    if name == "MEMORY":
        items: List[str] = []
        for ln in lines:
            ln = ln.strip()
            if not ln:
                continue
            m = re.match(r"^-\s+(.*)$", ln)
            if m:
                items.append(m.group(1).strip())
            elif items:
                # continuation of a multi-line item
                items[-1] += "\n" + ln
            else:
                raise PromptParseError(
                    f"plain: MEMORY line lacks '- ' bullet: {ln!r}"
                )
        return items
    if name == "TOOLS":
        tools = []
        for ln in lines:
            ln = ln.strip()
            if not ln:
                continue
            m = re.match(r"^-\s+(.*)$", ln)
            if not m:
                raise PromptParseError(
                    f"plain: TOOLS line lacks '- ' bullet: {ln!r}"
                )
            body = m.group(1)
            if ":" not in body:
                raise PromptParseError(
                    f"plain: TOOLS line lacks 'name: desc': {ln!r}"
                )
            nm, desc = body.split(":", 1)
            tools.append({"name": nm.strip(), "description": desc.strip()})
        return tools
    if name == "METADATA":
        meta = {}
        for ln in lines:
            ln = ln.strip()
            if not ln:
                continue
            if "=" not in ln:
                raise PromptParseError(
                    f"plain: METADATA line lacks 'k = v': {ln!r}"
                )
            k, v = ln.split("=", 1)
            meta[k.strip()] = v.strip()
        return meta
    return "\n".join(lines).strip()


def _parse_plain(text: str) -> Dict[str, object]:
    out: Dict[str, object] = {}
    cur_name = None
    cur_lines: List[str] = []
    saw_header = False

    def flush() -> None:
        nonlocal cur_name, cur_lines
        if cur_name is not None:
            if cur_name in out:
                raise PromptParseError(f"plain: duplicate block [{cur_name}]")
            out[cur_name] = _plain_block_payload(cur_name, cur_lines)
        cur_name, cur_lines = None, []

    for ln in text.split("\n"):
        m = _PLAIN_HEADER_RE.match(ln)
        if m:
            saw_header = True
            flush()
            name = m.group(1)
            if name not in _KNOWN_BLOCKS:
                raise PromptParseError(f"plain: unknown block [{name}]")
            cur_name = name
        else:
            if cur_name is not None:
                cur_lines.append(ln)
            elif ln.strip():
                raise PromptParseError(
                    f"plain: content before first [BLOCK] header: {ln!r}"
                )
    flush()
    if not saw_header:
        raise PromptParseError("plain: no [BLOCK] section headers found")
    return out


_PARSERS = {
    "json": _parse_json,
    "yaml": _parse_yaml,
    "xml": _parse_xml,
    "tagged": _parse_tagged,
    "plain": _parse_plain,
}


def parse_prompt(text: str) -> PromptContext:
    """Parse any supported serialization back to a ``PromptContext``."""
    text = text.lstrip("\ufeff")
    fmt = detect_format(text)
    mapping = _PARSERS[fmt](text)
    return PromptContext.from_mapping(mapping)


def canonicalize(text: str) -> bytes:
    """Parse any variant serialization and emit canonical bytes."""
    return canonical_bytes(parse_prompt(text))
