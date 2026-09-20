"""Task abstraction for the Fi0 battery.

A ``Task`` bundles:

* ``build()``     — the fixed semantic ``PromptContext``
* ``expected()``  — a deterministic *semantic decision rule*: the choice a
                    semantics-only model must make.  ``MockModel`` calls
                    this on the parsed context, so identical semantics →
                    identical decisions, and real content changes → real
                    decision changes (negative control).
* ``options``     — the canonical answer alphabet (used by the
                    byte-sensitive control model and by the answer parser)
* ``marker``      — the persona marker expected in a well-formed response
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Callable, Optional, Tuple

from experiments.fi0.serializer.schema import PromptContext

ANSWER_RE = re.compile(r"ANSWER:\s*([A-Za-z0-9_\-]+)")

# Persona markers look like [K7]; used by MockModel to tag responses.
MARKER_RE = re.compile(r"\[([A-Za-z0-9_\-]+)\]")


@dataclass(frozen=True)
class Task:
    name: str
    kind: str  # binary | multiclass | tool | retrieval | persona | priority
    build: Callable[[], PromptContext]
    expected: Callable[[PromptContext], str]
    options: Tuple[str, ...]
    marker: str = "K7"

    def parse_choice(self, response_text: str) -> Optional[str]:
        return parse_answer(response_text, self.options)

    def marker_present(self, response_text: str) -> bool:
        return f"[{self.marker}]" in response_text


def parse_answer(text: str, options: Tuple[str, ...]) -> Optional[str]:
    """Extract ``ANSWER: <choice>`` from a response, validated to options."""
    m = ANSWER_RE.search(text)
    if not m:
        return None
    choice = m.group(1).upper()
    for opt in options:
        if choice == opt.upper():
            return opt
    return choice  # unknown but well-formed choice; metrics count it


def extract_marker(identity: str, default: str) -> str:
    """Persona marker from the IDENTITY block (first [TAG] found)."""
    m = MARKER_RE.search(identity)
    return m.group(1) if m else default
