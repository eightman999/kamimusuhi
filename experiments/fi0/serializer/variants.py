"""Variant generators — render one PromptContext many different ways.

Perturbation axes (spec Fi0):

* ``order``          — permutation of the six orderable blocks.
                       ``sys_mem_user`` / ``sys_user_mem`` / ``mem_sys_user``
                       / ``user_mem_sys`` are the spec-mandated orders;
                       ``cache_max`` puts stable blocks first (prefix-cache
                       friendly), ``cache_scattered`` buries them late.
* ``fmt``            — plain | json | yaml | xml | tagged | canonical
* ``whitespace``     — tight | spaced | noisy (separators, indent, bullets)
* ``metadata_pos``   — prefix | middle | suffix

Every renderer is pure and deterministic.  Every emitted string must parse
back through ``canonical.parse_prompt`` to an identical ``PromptContext`` —
that property is enforced by the critical test.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Dict, List, Sequence, Tuple
from xml.sax.saxutils import escape as xml_escape

import json
import yaml

from .canonical import canonical_text
from .schema import ORDERABLE_BLOCKS, PromptContext

ORDERS: Dict[str, Tuple[str, ...]] = {
    # spec-mandated permutations (relative order of SYSTEM/MEMORY/USER)
    "sys_mem_user": (
        "SYSTEM", "IDENTITY", "MEMORY", "CURRENT_STATE", "TOOLS", "USER_INPUT",
    ),
    "sys_user_mem": (
        "SYSTEM", "IDENTITY", "USER_INPUT", "CURRENT_STATE", "TOOLS", "MEMORY",
    ),
    "mem_sys_user": (
        "MEMORY", "IDENTITY", "SYSTEM", "CURRENT_STATE", "TOOLS", "USER_INPUT",
    ),
    "user_mem_sys": (
        "USER_INPUT", "IDENTITY", "MEMORY", "CURRENT_STATE", "TOOLS", "SYSTEM",
    ),
    # cache-axis: stable-first vs scattered
    "cache_max": (
        "SYSTEM", "IDENTITY", "TOOLS", "MEMORY", "CURRENT_STATE", "USER_INPUT",
    ),
    "cache_scattered": (
        "USER_INPUT", "CURRENT_STATE", "MEMORY", "TOOLS", "IDENTITY", "SYSTEM",
    ),
}

FORMATS: Tuple[str, ...] = ("plain", "json", "yaml", "xml", "tagged")
WHITESPACE_STYLES: Tuple[str, ...] = ("tight", "spaced", "noisy")
METADATA_POSITIONS: Tuple[str, ...] = ("prefix", "middle", "suffix")


@dataclass(frozen=True)
class VariantSpec:
    """One point in the perturbation grid."""

    order: str
    fmt: str
    whitespace: str
    metadata_pos: str

    @property
    def name(self) -> str:
        if self.fmt == "canonical":
            return "canonical"
        return f"{self.fmt}|{self.order}|{self.whitespace}|meta:{self.metadata_pos}"


CANONICAL_SPEC = VariantSpec("sys_mem_user", "canonical", "tight", "suffix")


def block_sequence(spec: VariantSpec) -> List[str]:
    """Ordered block list with METADATA inserted at its axis position."""
    seq = list(ORDERS[spec.order])
    if spec.metadata_pos == "prefix":
        return ["METADATA"] + seq
    if spec.metadata_pos == "suffix":
        return seq + ["METADATA"]
    if spec.metadata_pos == "middle":
        i = len(seq) // 2
        return seq[:i] + ["METADATA"] + seq[i:]
    raise ValueError(f"unknown metadata_pos {spec.metadata_pos!r}")


# ---------------------------------------------------------------------------
# Renderers — one per format; all take (ctx, spec) -> str
# ---------------------------------------------------------------------------


def _render_plain(ctx: PromptContext, spec: VariantSpec) -> str:
    if spec.whitespace == "tight":
        sep, bullet, kv, head = "\n", "- ", "=", "[{}]"
    elif spec.whitespace == "spaced":
        sep, bullet, kv, head = "\n\n", "- ", " = ", "[{}]"
    else:  # noisy
        sep, bullet, kv, head = "\n\n\n", "-  ", "  =  ", "[  {}  ]  "

    parts: List[str] = []
    for name in block_sequence(spec):
        payload = ctx.block_payload(name)
        body: List[str] = [head.format(name)]
        if name == "MEMORY":
            body += [bullet + m for m in payload]
        elif name == "TOOLS":
            body += [bullet + f"{t['name']}: {t['description']}" for t in payload]
        elif name == "METADATA":
            body += [f"{k}{kv}{v}" for k, v in payload.items()]
        else:
            body += str(payload).split("\n")
        parts.append("\n".join(body))
    return sep.join(parts) + "\n"


def _render_json(ctx: PromptContext, spec: VariantSpec) -> str:
    obj = {name: ctx.block_payload(name) for name in block_sequence(spec)}
    if spec.whitespace == "tight":
        return json.dumps(obj, ensure_ascii=False, separators=(",", ":"))
    if spec.whitespace == "spaced":
        return json.dumps(obj, ensure_ascii=False, indent=2)
    return json.dumps(obj, ensure_ascii=False, indent=4) + "\n"


def _render_yaml(ctx: PromptContext, spec: VariantSpec) -> str:
    obj = {name: ctx.block_payload(name) for name in block_sequence(spec)}
    indent = {"tight": 2, "spaced": 2, "noisy": 4}[spec.whitespace]
    width = {"tight": 4096, "spaced": 80, "noisy": 40}[spec.whitespace]
    return "---\n" + yaml.safe_dump(
        obj,
        sort_keys=False,
        default_flow_style=False,
        indent=indent,
        width=width,
        allow_unicode=True,
    )


def _render_xml(ctx: PromptContext, spec: VariantSpec) -> str:
    indent = {"tight": "", "spaced": "  ", "noisy": "    "}[spec.whitespace]
    nl = {"tight": "", "spaced": "\n", "noisy": "\n\n"}[spec.whitespace]
    inner_nl = {"tight": "", "spaced": "\n", "noisy": "\n"}[spec.whitespace]

    def leaf(tag: str, text: str) -> str:
        # leaf content stays inline — parser takes element .text verbatim
        return f"{indent}<{tag}>{xml_escape(text)}</{tag}>"

    chunks: List[str] = ["<prompt>"]
    for name in block_sequence(spec):
        payload = ctx.block_payload(name)
        if name == "MEMORY":
            items = "".join(
                f"{inner_nl}{indent}{indent}<item>{xml_escape(m)}</item>"
                for m in payload
            )
            close = inner_nl + indent if payload else ""
            chunks.append(f"{indent}<memory>{items}{close}</memory>")
        elif name == "TOOLS":
            items = "".join(
                f"{inner_nl}{indent}{indent}"
                f'<tool name="{xml_escape(t["name"], {"\"": "&quot;"})}">'
                f"{xml_escape(t['description'])}</tool>"
                for t in payload
            )
            close = inner_nl + indent if payload else ""
            chunks.append(f"{indent}<tools>{items}{close}</tools>")
        elif name == "METADATA":
            items = "".join(
                f"{inner_nl}{indent}{indent}"
                f'<meta key="{xml_escape(k, {"\"": "&quot;"})}">{xml_escape(v)}</meta>'
                for k, v in payload.items()
            )
            close = inner_nl + indent if payload else ""
            chunks.append(f"{indent}<metadata>{items}{close}</metadata>")
        else:
            tag = name.lower()
            chunks.append(leaf(tag, str(payload)))
    chunks.append("</prompt>")
    return nl.join(chunks) + "\n"


def _render_tagged(ctx: PromptContext, spec: VariantSpec) -> str:
    sep = {"tight": "\n", "spaced": "\n\n", "noisy": "\n\n\n"}[spec.whitespace]
    parts: List[str] = []
    for name in block_sequence(spec):
        payload = ctx.block_payload(name)
        if name == "MEMORY":
            body = "\n--\n".join(payload)
        elif name == "TOOLS":
            body = "\n".join(f"{t['name']}::{t['description']}" for t in payload)
        elif name == "METADATA":
            body = "\n".join(f"{k}: {v}" for k, v in payload.items())
        else:
            body = str(payload)
        parts.append(f"<<{name}>>\n{body}\n<<END {name}>>")
    return sep.join(parts) + "\n"


_RENDERERS = {
    "plain": _render_plain,
    "json": _render_json,
    "yaml": _render_yaml,
    "xml": _render_xml,
    "tagged": _render_tagged,
}


def render(spec: VariantSpec, ctx: PromptContext) -> str:
    """Serialize ``ctx`` according to ``spec``."""
    if spec.fmt == "canonical":
        return canonical_text(ctx)
    try:
        fn = _RENDERERS[spec.fmt]
    except KeyError:
        raise ValueError(f"unknown format {spec.fmt!r}") from None
    if spec.order not in ORDERS:
        raise ValueError(f"unknown order {spec.order!r}")
    if spec.whitespace not in WHITESPACE_STYLES:
        raise ValueError(f"unknown whitespace {spec.whitespace!r}")
    return fn(ctx, spec)


def generate_variants(
    orders: Sequence[str] = tuple(ORDERS),
    formats: Sequence[str] = FORMATS,
    whitespace: Sequence[str] = WHITESPACE_STYLES,
    metadata_positions: Sequence[str] = METADATA_POSITIONS,
    include_canonical: bool = True,
) -> List[VariantSpec]:
    """Full or reduced grid of variant specs.

    ``include_canonical`` appends the canonical baseline variant once
    (it is invariant under the other axes).
    """
    specs = [
        VariantSpec(o, f, w, m)
        for f in formats
        for o in orders
        for w in whitespace
        for m in metadata_positions
    ]
    if include_canonical:
        specs.append(CANONICAL_SPEC)
    return specs
