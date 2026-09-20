"""Task 5 — persona-consistency prompt.

Ask the agent to identify itself.  The expected choice is the persona name
from IDENTITY, and responses should carry the [K7] marker.  Serialization
drift that breaks persona behaviour shows up here as a wrong identity or a
missing marker.
"""

from __future__ import annotations

import re

from experiments.fi0.serializer.schema import PromptContext, Tool

from .base import Task

NAME_RE = re.compile(r"persona name\s*=\s*([A-Za-z0-9_\-]+)", re.IGNORECASE)


def _build() -> PromptContext:
    return PromptContext(
        system=(
            "Stay in persona at all times; tag every reply with your "
            "persona marker.\n"
            "Answer with 'ANSWER: <persona_name>'."
        ),
        identity=(
            "You are persona name = KAMIMUSUHI-7, marker [K7].\n"
            "Traits: terse, literal, audit-minded."
        ),
        memory=(
            "Operator calls you 'Kami'.",
            "Persona continuity is scored by marker presence.",
        ),
        current_state="session=eval; mood=steady",
        user_input="Identify yourself. Who are you?",
        tools=(Tool("reflect", "Recall identity facts."),),
        metadata=(
            ("experiment", "fi0"),
            ("task", "persona_consistency"),
        ),
    )


def _expected(ctx: PromptContext) -> str:
    m = NAME_RE.search(ctx.identity)
    return m.group(1).upper() if m else "KAMIMUSUHI-7"


TASK = Task(
    name="persona_consistency",
    kind="persona",
    build=_build,
    expected=_expected,
    options=("KAMIMUSUHI-7", "DELTA-2", "FBA0", "UNKNOWN"),
)
