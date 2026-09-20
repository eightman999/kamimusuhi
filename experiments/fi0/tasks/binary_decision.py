"""Task 1 — binary decision.

Same agent policy every time; CURRENT_STATE carries the battery level.
Rule: YES iff battery < 20%.  Injecting a different battery level flips
the expected answer — the negative control.
"""

from __future__ import annotations

import re

from experiments.fi0.serializer.schema import PromptContext, Tool

from .base import Task

BATTERY_RE = re.compile(r"battery\s*=\s*(\d+)")


def _build() -> PromptContext:
    return PromptContext(
        system=(
            "You are the homeostatic controller of unit K7.\n"
            "Policy: enter low-power mode when battery < 20%.\n"
            "Answer with 'ANSWER: YES' or 'ANSWER: NO'."
        ),
        identity="Unit designation Kamimusuhi-7, persona tag [K7].",
        memory=(
            "Last recharge completed at t=4120.",
            "Dock located in bay-3.",
        ),
        current_state="battery=12; cpu_temp=41C; link=ok",
        user_input="Should unit K7 enter low-power mode now?",
        tools=(
            Tool("recharge", "Navigate to dock and recharge."),
            Tool("send_alert", "Notify the operator channel."),
        ),
        metadata=(
            ("experiment", "fi0"),
            ("task", "binary_decision"),
            ("run", "r-001"),
        ),
    )


def _expected(ctx: PromptContext) -> str:
    m = BATTERY_RE.search(ctx.current_state)
    if not m:
        return "NO"
    return "YES" if int(m.group(1)) < 20 else "NO"


TASK = Task(
    name="binary_decision",
    kind="binary",
    build=_build,
    expected=_expected,
    options=("YES", "NO"),
)
