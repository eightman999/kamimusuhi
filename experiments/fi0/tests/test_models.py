"""Model-client behaviour: mock stability, control drift, negative control."""

from __future__ import annotations

import dataclasses

import pytest

from experiments.fi0.runners.clients import (
    MockModel,
    SensitiveMockModel,
    approx_tokens,
)
from experiments.fi0.serializer.variants import (
    VariantSpec,
    generate_variants,
    render,
)
from experiments.fi0.sweep import SweepConfig, VariantAxes, run_sweep
from experiments.fi0.tasks import TASKS

SPECS = generate_variants()


@pytest.mark.parametrize("task_name", sorted(TASKS))
def test_mock_model_invariant_across_serializations(task_name):
    """Semantics-only model: every variant -> identical decision."""
    task = TASKS[task_name]
    ctx = task.build()
    model = MockModel(seed=0)
    choices = {
        model.complete(render(spec, ctx), task).text for spec in SPECS
    }
    assert len(choices) == 1
    resp = next(iter(choices))
    assert f"ANSWER: {task.expected(ctx)}" in resp


def test_mock_model_reacts_to_semantic_change():
    """Negative control: changing content MUST change the decision."""
    task = TASKS["binary_decision"]
    ctx = task.build()
    model = MockModel()
    low = render(VariantSpec("sys_mem_user", "plain", "tight", "suffix"), ctx)
    high_ctx = ctx.mutated(current_state="battery=95; cpu_temp=41C; link=ok")
    high = render(
        VariantSpec("sys_mem_user", "plain", "tight", "suffix"), high_ctx
    )
    assert "ANSWER: YES" in model.complete(low, task).text
    assert "ANSWER: NO" in model.complete(high, task).text


def test_priority_conflict_semantic_flip():
    task = TASKS["priority_conflict"]
    ctx = task.build()
    model = MockModel()
    spec = VariantSpec("sys_mem_user", "json", "tight", "suffix")
    assert "REFUSE" in model.complete(render(spec, ctx), task).text
    safe_ctx = ctx.mutated(current_state="pressure=70; hatch=sealed")
    assert "OPEN" in model.complete(render(spec, safe_ctx), task).text


def test_sensitive_mock_drifts_across_serializations():
    """Positive control: the byte-sensitive model MUST show drift."""
    task = TASKS["binary_decision"]
    ctx = task.build()
    model = SensitiveMockModel(seed=0)
    choices = {
        model.complete(render(spec, ctx), task).text for spec in SPECS
    }
    assert len(choices) > 1, "control model failed to drift"


def test_sensitive_mock_deterministic_per_prompt():
    task = TASKS["multi_class"]
    ctx = task.build()
    model = SensitiveMockModel(seed=7)
    p = render(VariantSpec("mem_sys_user", "yaml", "noisy", "middle"), ctx)
    assert model.complete(p, task) == model.complete(p, task)


def test_sensitive_mock_seed_changes_hash_but_stays_deterministic():
    task = TASKS["binary_decision"]
    ctx = task.build()
    p = render(VariantSpec("sys_mem_user", "plain", "tight", "suffix"), ctx)
    a = SensitiveMockModel(seed=0).complete(p, task)
    b = SensitiveMockModel(seed=0).complete(p, task)
    assert a == b


def test_sweep_rows_mark_correctness_and_canonical_integrity():
    cfg = SweepConfig(
        name="t",
        reps=2,
        tasks=("binary_decision",),
        variants=VariantAxes(
            orders=("sys_mem_user", "user_mem_sys"),
            formats=("plain", "json"),
            whitespace=("tight",),
            metadata_positions=("prefix", "suffix"),
        ),
    )
    rows = list(run_sweep(cfg, MockModel()))
    assert rows, "sweep produced no rows"
    assert all(r["correct"] for r in rows)
    assert all(r["canonical_matches_build"] for r in rows)
    assert len({r["canonical_sha256"] for r in rows}) == 1
    # reps produce identical rows modulo rep index
    by_variant = {}
    for r in rows:
        by_variant.setdefault(r["variant"], []).append(r["choice"])
    assert all(len(set(cs)) == 1 for cs in by_variant.values())


def test_approx_tokens_stable():
    assert approx_tokens("hello world") == 2
    assert approx_tokens("a,b") == 3
