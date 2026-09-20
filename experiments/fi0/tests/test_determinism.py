"""Determinism / multi-run reproduction.

Same config + same client => byte-identical decision stream.  Multi-seed
runs of MockModel reproduce exactly (its decisions are seed-free by
design — a pure function of semantics); the seed is still recorded in the
manifest for provenance.
"""

from __future__ import annotations

from experiments.fi0.runners.clients import MockModel, SensitiveMockModel
from experiments.fi0.serializer.variants import generate_variants, render
from experiments.fi0.sweep import SweepConfig, VariantAxes, run_sweep
from experiments.fi0.tasks import TASKS

AXES = VariantAxes(
    orders=("sys_mem_user", "user_mem_sys", "cache_max"),
    formats=("plain", "json", "yaml"),
    whitespace=("tight", "noisy"),
    metadata_positions=("prefix", "suffix"),
)


def _key_stream(model, seed):
    cfg = SweepConfig(
        name="det", seed=seed, reps=2,
        tasks=tuple(TASKS), variants=AXES,
    )
    rows = list(run_sweep(cfg, model))
    return [(r["task"], r["variant"], r["rep"], r["choice"],
             r["marker_present"], r["prompt_sha256"]) for r in rows]


def test_mock_sweep_deterministic_across_runs():
    assert _key_stream(MockModel(0), 0) == _key_stream(MockModel(0), 0)


def test_mock_sweep_deterministic_across_seeds():
    """Semantics-only decisions must not depend on seed."""
    assert _key_stream(MockModel(0), 0) == _key_stream(MockModel(1), 1)


def test_sensitive_sweep_deterministic_per_seed():
    assert _key_stream(SensitiveMockModel(3), 3) == _key_stream(
        SensitiveMockModel(3), 3
    )


def test_prompt_bytes_deterministic():
    """Renderers are pure: re-render => identical bytes."""
    ctx = TASKS["binary_decision"].build()
    for spec in generate_variants(
        orders=("sys_mem_user",), formats=("xml", "tagged")
    ):
        assert render(spec, ctx) == render(spec, ctx)
