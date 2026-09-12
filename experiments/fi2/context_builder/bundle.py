"""ContextBundle — what a condition actually places in front of the model."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import List

from ..store.events import Span


@dataclass
class ContextBundle:
    condition: str
    spans: List[Span]
    budget: int
    token_count: int
    truncated: bool = False
    meta: dict = field(default_factory=dict)

    def render_text(self) -> str:
        return "\n".join(s.text for s in self.spans)

    def claims(self) -> List[Span]:
        return [s for s in self.spans if s.claim is not None]
