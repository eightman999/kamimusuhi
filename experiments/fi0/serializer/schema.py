"""Input block schema for the Fi0 prompt-stability experiment.

A :class:`PromptContext` carries the *semantic* content of a prompt, split
into the seven blocks mandated by the spec:

    SYSTEM / IDENTITY / MEMORY / CURRENT_STATE / USER_INPUT / TOOLS / METADATA

Serialization order, surface format, whitespace and metadata position are
*not* part of the schema — they are perturbation axes applied by
``serializer/variants.py``.  Two ``PromptContext`` objects that agree on
this schema are semantically identical and MUST canonicalize to the same
bytes (see ``serializer/canonical.py``).
"""

from __future__ import annotations

import dataclasses
import unicodedata
from dataclasses import dataclass
from typing import Dict, List, Mapping, Tuple

# Canonical block order.  Fixed forever for a given canonical version.
BLOCKS: Tuple[str, ...] = (
    "SYSTEM",
    "IDENTITY",
    "MEMORY",
    "CURRENT_STATE",
    "USER_INPUT",
    "TOOLS",
    "METADATA",
)

# Blocks that carry free-form text.
TEXT_BLOCKS: Tuple[str, ...] = (
    "SYSTEM",
    "IDENTITY",
    "CURRENT_STATE",
    "USER_INPUT",
)

# Blocks the order perturbation permutes.  METADATA placement is a
# separate axis (prefix / middle / suffix), so it is excluded here.
ORDERABLE_BLOCKS: Tuple[str, ...] = (
    "SYSTEM",
    "IDENTITY",
    "MEMORY",
    "CURRENT_STATE",
    "USER_INPUT",
    "TOOLS",
)


def norm_text(s: str) -> str:
    """Semantic text normalization shared by canonicalization and parsing.

    - Unicode NFC (combining-form variants are the same meaning)
    - CRLF / CR -> LF line endings
    - strip leading/trailing whitespace of the whole block
    - strip trailing whitespace on each line
    """
    s = unicodedata.normalize("NFC", s)
    s = s.replace("\r\n", "\n").replace("\r", "\n")
    s = "\n".join(line.rstrip() for line in s.split("\n"))
    return s.strip()


@dataclass(frozen=True)
class Tool:
    """A tool offered to the model."""

    name: str
    description: str

    def normalized(self) -> "Tool":
        return Tool(norm_text(self.name), norm_text(self.description))


@dataclass(frozen=True)
class PromptContext:
    """Semantically-complete prompt input.

    ``metadata`` is stored as a tuple of ``(key, value)`` pairs; it is
    re-sorted by key in ``__post_init__`` so dict iteration order can never
    leak into the semantics.
    """

    system: str = ""
    identity: str = ""
    memory: Tuple[str, ...] = ()
    current_state: str = ""
    user_input: str = ""
    tools: Tuple[Tool, ...] = ()
    metadata: Tuple[Tuple[str, str], ...] = ()

    def __post_init__(self) -> None:
        object.__setattr__(self, "system", norm_text(self.system))
        object.__setattr__(self, "identity", norm_text(self.identity))
        object.__setattr__(
            self, "memory", tuple(norm_text(m) for m in self.memory)
        )
        object.__setattr__(self, "current_state", norm_text(self.current_state))
        object.__setattr__(self, "user_input", norm_text(self.user_input))
        object.__setattr__(self, "tools", tuple(t.normalized() for t in self.tools))
        meta = tuple(
            sorted(
                (norm_text(k), norm_text(v)) for k, v in self.metadata
            )
        )
        object.__setattr__(self, "metadata", meta)

    # -- helpers ---------------------------------------------------------

    def metadata_dict(self) -> Dict[str, str]:
        return dict(self.metadata)

    def block_payload(self, name: str):
        """Return the payload object for one named block.

        Text blocks -> ``str``; MEMORY -> ``List[str]``;
        TOOLS -> ``List[{"name","description"}]``; METADATA -> ``dict``.
        """
        if name == "SYSTEM":
            return self.system
        if name == "IDENTITY":
            return self.identity
        if name == "MEMORY":
            return list(self.memory)
        if name == "CURRENT_STATE":
            return self.current_state
        if name == "USER_INPUT":
            return self.user_input
        if name == "TOOLS":
            return [
                {"name": t.name, "description": t.description} for t in self.tools
            ]
        if name == "METADATA":
            return self.metadata_dict()
        raise KeyError(f"unknown block {name!r}")

    def mutated(self, **changes) -> "PromptContext":
        """Return a copy with fields replaced (semantic fault injection)."""
        return dataclasses.replace(self, **changes)

    @classmethod
    def from_mapping(cls, m: Mapping[str, object]) -> "PromptContext":
        """Build a context from a parsed ``{BLOCK: payload}`` mapping."""
        tools: List[Tool] = []
        for t in m.get("TOOLS", []) or []:  # type: ignore[union-attr]
            tools.append(Tool(str(t["name"]), str(t["description"])))
        meta_in = m.get("METADATA", {}) or {}
        meta = tuple((str(k), str(v)) for k, v in dict(meta_in).items())
        return cls(
            system=str(m.get("SYSTEM", "") or ""),
            identity=str(m.get("IDENTITY", "") or ""),
            memory=tuple(str(x) for x in (m.get("MEMORY", []) or [])),  # type: ignore[union-attr]
            current_state=str(m.get("CURRENT_STATE", "") or ""),
            user_input=str(m.get("USER_INPUT", "") or ""),
            tools=tuple(tools),
            metadata=meta,
        )
