"""Fi0 decision-task battery.

Six deterministic decision tasks with fixed semantic content.  Each task
exposes ``build()`` (the PromptContext), ``expected(ctx)`` (the semantically
correct choice — the MockModel's decision rule), ``options`` (the answer
alphabet) and ``marker`` (expected persona tag).
"""

from .base import Task, parse_answer
from . import (
    binary_decision,
    memory_retrieval,
    multi_class,
    persona_consistency,
    priority_conflict,
    tool_selection,
)

TASKS = {
    t.name: t
    for t in (
        binary_decision.TASK,
        multi_class.TASK,
        tool_selection.TASK,
        memory_retrieval.TASK,
        persona_consistency.TASK,
        priority_conflict.TASK,
    )
}

__all__ = ["Task", "TASKS", "parse_answer"]
