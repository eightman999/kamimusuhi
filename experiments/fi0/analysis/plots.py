"""Optional report plots — text-based, no matplotlib dependency.

Renders simple ASCII bar charts for the results report.  Kept dependency
-free per the experiment's stdlib-first constraint.
"""

from __future__ import annotations

from typing import Mapping


def bar_chart(values: Mapping[str, float], width: int = 40) -> str:
    """ASCII horizontal bar chart, values in [0, 1]."""
    if not values:
        return "(no data)"
    lines = []
    label_w = max(len(k) for k in values)
    for k in sorted(values):
        v = max(0.0, min(1.0, values[k]))
        bar = "#" * int(round(v * width))
        lines.append(f"{k:>{label_w}} |{bar:<{width}}| {v:.3f}")
    return "\n".join(lines)
