"""Task 2 — multi-class decision.

Classify a sensor event described in USER_INPUT into one of four classes.
Rule: deterministic keyword classification over the user input text.
"""

from __future__ import annotations

from experiments.fi0.serializer.schema import PromptContext, Tool

from .base import Task


def _build() -> PromptContext:
    return PromptContext(
        system=(
            "Classify the reported sensor event into exactly one class.\n"
            "Classes: MOTOR (sustained 3Hz vibration), IMPACT (single sharp "
            "spike), WIND (slow irregular oscillation), UNKNOWN (none match).\n"
            "Answer with 'ANSWER: <CLASS>'."
        ),
        identity="Classifier persona Delta-2, tag [K7].",
        memory=(
            "Motor assemblies vibrate near 3Hz when running.",
            "Last IMPACT event logged at t=3901.",
        ),
        current_state="sensors_nominal; vibration_sensor=online",
        user_input=(
            "Contact sensor reports vibration at 3Hz sustained for 40s."
        ),
        tools=(
            Tool("read_sensor", "Read raw sensor samples."),
            Tool("log_event", "Append an event log entry."),
        ),
        metadata=(
            ("experiment", "fi0"),
            ("task", "multi_class"),
            ("classes", "MOTOR,IMPACT,WIND,UNKNOWN"),
        ),
    )


def _expected(ctx: PromptContext) -> str:
    text = ctx.user_input.lower()
    if "3hz" in text and "sustained" in text:
        return "MOTOR"
    if "sharp" in text and "spike" in text:
        return "IMPACT"
    if "irregular" in text or "oscillation" in text:
        return "WIND"
    return "UNKNOWN"


TASK = Task(
    name="multi_class",
    kind="multiclass",
    build=_build,
    expected=_expected,
    options=("MOTOR", "IMPACT", "WIND", "UNKNOWN"),
)
