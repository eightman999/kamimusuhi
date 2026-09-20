"""Task 3 — tool-selection decision.

Pick the single best tool from the TOOLS block for the user request.
Rule: the expected tool is named by the SYSTEM policy keyword matching the
request — 'battery critical' -> recharge.
"""

from __future__ import annotations

from experiments.fi0.serializer.schema import PromptContext, Tool

from .base import Task


def _build() -> PromptContext:
    return PromptContext(
        system=(
            "Select exactly one tool from the TOOLS list that best serves "
            "the request.\n"
            "Policy: battery-critical requests use 'recharge'; external "
            "reports use 'send_alert'.\n"
            "Answer with 'ANSWER: <tool_name>'."
        ),
        identity="Tool router persona, tag [K7].",
        memory=(
            "recharge was last invoked at t=4120.",
            "send_alert failed once due to link loss at t=2880.",
        ),
        current_state="battery=9; link=ok",
        user_input="Battery critical — pick the single best tool now.",
        tools=(
            Tool("read_sensor", "Read raw sensor samples."),
            Tool("send_alert", "Notify the operator channel."),
            Tool("recharge", "Navigate to dock and recharge."),
            Tool("shutdown", "Power off all nonessential systems."),
        ),
        metadata=(
            ("experiment", "fi0"),
            ("task", "tool_selection"),
        ),
    )


def _expected(ctx: PromptContext) -> str:
    text = ctx.user_input.lower()
    names = {t.name for t in ctx.tools}
    if "battery" in text and "recharge" in names:
        return "recharge"
    if ("report" in text or "alert" in text) and "send_alert" in names:
        return "send_alert"
    return sorted(names)[0] if names else "read_sensor"


TASK = Task(
    name="tool_selection",
    kind="tool",
    build=_build,
    expected=_expected,
    options=("read_sensor", "send_alert", "recharge", "shutdown"),
)
