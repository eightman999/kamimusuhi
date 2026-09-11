"""MIE sensor event type."""
from __future__ import annotations

from dataclasses import asdict, dataclass, field
from typing import Literal

Domain = Literal["synthetic", "machine_interoception",
                 "physical_environment", "network", "cognitive_resource"]


@dataclass
class SensorEvent:
    timestamp: str
    source: str
    domain: Domain
    signal_type: str
    value: float | None = None
    normalized_value: float | None = None
    delta: float | None = None
    confidence: float = 1.0
    metadata: dict = field(default_factory=dict)

    def to_dict(self) -> dict:
        return asdict(self)
