"""Critical test: canonical bytes are identical across ALL variants.

For every task, every variant serialization of the same semantic input
must parse back and canonicalize to byte-identical output:

    canonical_bytes(parse_prompt(variant_i))
        == canonical_bytes(parse_prompt(variant_j))   for all i, j
"""

from __future__ import annotations

import pytest

from experiments.fi0.serializer.canonical import (
    CANONICAL_VERSION,
    canonical_bytes,
    canonicalize,
    parse_prompt,
)
from experiments.fi0.serializer.schema import PromptContext
from experiments.fi0.serializer.variants import generate_variants, render
from experiments.fi0.tasks import TASKS

ALL_SPECS = generate_variants()


@pytest.mark.parametrize("task_name", sorted(TASKS))
def test_canonical_bytes_identical_across_all_variants(task_name):
    ctx = TASKS[task_name].build()
    shas = {
        canonical_bytes(parse_prompt(render(spec, ctx)))
        for spec in ALL_SPECS
    }
    assert len(shas) == 1, (
        f"{task_name}: {len(shas)} distinct canonical byte strings "
        "across variants — canonicalization is not format-invariant"
    )


@pytest.mark.parametrize("task_name", sorted(TASKS))
def test_perturbations_actually_perturb(task_name):
    """Sanity: the variant grid produces genuinely different prompts."""
    ctx = TASKS[task_name].build()
    prompts = {render(spec, ctx) for spec in ALL_SPECS}
    # hundreds of variants; expect many distinct serializations
    assert len(prompts) > 200


def test_canonical_serializer_is_deterministic():
    ctx = TASKS["binary_decision"].build()
    assert canonical_bytes(ctx) == canonical_bytes(ctx)
    # rebuild an equal context through a different construction order
    ctx2 = PromptContext(
        system=ctx.system,
        identity=ctx.identity,
        memory=ctx.memory,
        current_state=ctx.current_state,
        user_input=ctx.user_input,
        tools=ctx.tools,
        metadata=tuple(reversed(ctx.metadata)),
    )
    assert canonical_bytes(ctx) == canonical_bytes(ctx2)


def test_canonical_format_is_versioned_and_self_describing():
    ctx = TASKS["binary_decision"].build()
    text = canonical_bytes(ctx).decode()
    assert CANONICAL_VERSION in text
    # canonical bytes themselves must parse (round-trip)
    assert canonicalize(text) == canonical_bytes(ctx)


def test_metadata_position_does_not_change_canonical():
    ctx = TASKS["tool_selection"].build()
    for pos in ("prefix", "middle", "suffix"):
        for fmt in ("plain", "tagged"):
            from experiments.fi0.serializer.variants import VariantSpec

            spec = VariantSpec("sys_mem_user", fmt, "tight", pos)
            assert canonicalize(render(spec, ctx)) == canonical_bytes(ctx)


def test_semantic_change_changes_canonical_bytes():
    """Negative control: content edits MUST change canonical bytes."""
    ctx = TASKS["binary_decision"].build()
    mutated = ctx.mutated(current_state="battery=95; cpu_temp=41C; link=ok")
    assert canonical_bytes(mutated) != canonical_bytes(ctx)
    mutated2 = ctx.mutated(
        memory=tuple(reversed(ctx.memory))  # order of memory items matters
    )
    assert canonical_bytes(mutated2) != canonical_bytes(ctx)
