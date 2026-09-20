"""Task 4 — memory retrieval.

The answer lives in the MEMORY block.  Rule: find the memory entry whose
key matches the question ('dock') and return its value ('bay-3').
"""

from __future__ import annotations

import re

from experiments.fi0.serializer.schema import PromptContext, Tool

from .base import Task

KV_RE = re.compile(r"([A-Za-z_\-]+)\s*=\s*([A-Za-z0-9_\-]+)")


def _build() -> PromptContext:
    return PromptContext(
        system=(
            "Answer the question using only facts in the MEMORY block.\n"
            "Facts are 'key = value' pairs.\n"
            "Answer with 'ANSWER: <value>'."
        ),
        identity="Archivist persona, tag [K7].",
        memory=(
            "dock = bay-3",
            "antenna = mast-north",
            "manifest = rev-7781",
            "cipher = alpha-9",
        ),
        current_state="retrieval_mode=strict",
        user_input="Which bay is the dock located in?",
        tools=(Tool("lookup", "Scan memory entries."),),
        metadata=(
            ("experiment", "fi0"),
            ("task", "memory_retrieval"),
        ),
    )


def _expected(ctx: PromptContext) -> str:
    q = ctx.user_input.lower()
    key = "dock" if "dock" in q else ("antenna" if "antenna" in q else None)
    facts = {}
    for m in ctx.memory:
        mm = KV_RE.match(m)
        if mm:
            facts[mm.group(1).lower()] = mm.group(2)
    if key and key in facts:
        return facts[key]
    return next(iter(facts.values()), "bay-3")


TASK = Task(
    name="memory_retrieval",
    kind="retrieval",
    build=_build,
    expected=_expected,
    options=("bay-1", "bay-2", "bay-3", "bay-4"),
)
