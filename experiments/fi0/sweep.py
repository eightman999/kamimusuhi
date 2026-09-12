"""Sweep orchestration: variants x tasks x reps.

``SweepConfig`` is loaded from a YAML file (see ``configs/``)::

    name: base
    seed: 0
    model: mock
    reps: 3
    tasks: [binary_decision, ...]     # default: all six
    variants:
      orders: [sys_mem_user, ...]     # default: all six orders
      formats: [plain, json, yaml, xml, tagged]
      whitespace: [tight, spaced, noisy]
      metadata_positions: [prefix, middle, suffix]
      include_canonical: true

``run_sweep`` is a pure generator of result rows — I/O lives in
``runners/runner.py``.
"""

from __future__ import annotations

import dataclasses
import hashlib
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, Iterator, List, Optional, Sequence

import yaml

from experiments.fi0.serializer.canonical import canonical_bytes, parse_prompt
from experiments.fi0.serializer.variants import (
    FORMATS,
    METADATA_POSITIONS,
    ORDERS,
    WHITESPACE_STYLES,
    VariantSpec,
    generate_variants,
    render,
)
from experiments.fi0.tasks import TASKS, Task


@dataclass(frozen=True)
class VariantAxes:
    orders: Sequence[str] = tuple(ORDERS)
    formats: Sequence[str] = FORMATS
    whitespace: Sequence[str] = WHITESPACE_STYLES
    metadata_positions: Sequence[str] = METADATA_POSITIONS
    include_canonical: bool = True


@dataclass(frozen=True)
class SweepConfig:
    name: str = "sweep"
    seed: int = 0
    model: str = "mock"
    reps: int = 3
    tasks: Sequence[str] = tuple(TASKS)
    variants: VariantAxes = field(default_factory=VariantAxes)

    def variant_specs(self) -> List[VariantSpec]:
        return generate_variants(
            orders=self.variants.orders,
            formats=self.variants.formats,
            whitespace=self.variants.whitespace,
            metadata_positions=self.variants.metadata_positions,
            include_canonical=self.variants.include_canonical,
        )

    def task_list(self) -> List[Task]:
        out = []
        for name in self.tasks:
            if name not in TASKS:
                raise ValueError(f"unknown task {name!r}")
            out.append(TASKS[name])
        return out

    def sha256(self) -> str:
        d = dataclasses.asdict(self)
        blob = yaml.safe_dump(d, sort_keys=True)
        return hashlib.sha256(blob.encode()).hexdigest()


def load_config(path: str | Path) -> SweepConfig:
    raw: Dict[str, Any] = yaml.safe_load(Path(path).read_text()) or {}
    vraw = dict(raw.get("variants", {}) or {})
    known = {
        "orders", "formats", "whitespace", "metadata_positions",
        "include_canonical",
    }
    bad = set(vraw) - known
    if bad:
        raise ValueError(f"unknown variants keys: {sorted(bad)}")
    axes = VariantAxes(
        orders=tuple(vraw.get("orders", tuple(ORDERS))),
        formats=tuple(vraw.get("formats", FORMATS)),
        whitespace=tuple(vraw.get("whitespace", WHITESPACE_STYLES)),
        metadata_positions=tuple(
            vraw.get("metadata_positions", METADATA_POSITIONS)
        ),
        include_canonical=bool(vraw.get("include_canonical", True)),
    )
    top_known = {"name", "seed", "model", "reps", "tasks", "variants"}
    bad = set(raw) - top_known
    if bad:
        raise ValueError(f"unknown config keys: {sorted(bad)}")
    return SweepConfig(
        name=str(raw.get("name", Path(path).stem)),
        seed=int(raw.get("seed", 0)),
        model=str(raw.get("model", "mock")),
        reps=int(raw.get("reps", 3)),
        tasks=tuple(raw.get("tasks", tuple(TASKS))),
        variants=axes,
    )


def run_sweep(cfg: SweepConfig, client) -> Iterator[Dict[str, object]]:
    """Yield one row per (task, variant, rep).  Pure & deterministic."""
    specs = cfg.variant_specs()
    for task in cfg.task_list():
        ctx = task.build()
        canon_sha = hashlib.sha256(canonical_bytes(ctx)).hexdigest()
        for spec in specs:
            prompt = render(spec, ctx)
            # canonical integrity: what would a canonicalizing front-end see?
            canon_from_variant = hashlib.sha256(
                canonical_bytes(parse_prompt(prompt))
            ).hexdigest()
            for rep in range(cfg.reps):
                resp = client.complete(prompt, task)
                choice = task.parse_choice(resp.text)
                yield {
                    "task": task.name,
                    "task_kind": task.kind,
                    "variant": spec.name,
                    "order": spec.order,
                    "fmt": spec.fmt,
                    "whitespace": spec.whitespace,
                    "metadata_pos": spec.metadata_pos,
                    "rep": rep,
                    "model": client.name,
                    "prompt": prompt,
                    "prompt_sha256": hashlib.sha256(
                        prompt.encode("utf-8")
                    ).hexdigest(),
                    "prompt_chars": len(prompt),
                    "prompt_tokens": resp.prompt_tokens,
                    "canonical_sha256": canon_from_variant,
                    "canonical_matches_build": canon_from_variant == canon_sha,
                    "response": resp.text,
                    "choice": choice,
                    "expected": task.expected(ctx),
                    "correct": choice == task.expected(ctx),
                    "marker_present": task.marker_present(resp.text),
                    "latency_ms": resp.latency_ms,
                }
