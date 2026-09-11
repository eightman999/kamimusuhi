# Experiments

This directory contains **plans and reports for concrete Kamimusuhi experiments**. It is an evidence layer, not the normative architecture.

## Naming

Prefer stable, descriptive names. If an experiment belongs to a numbered series, preserve that identifier in the filename/directory so later papers can cite it unambiguously.

For substantial experiments, use a directory:

```text
docs/experiments/<experiment-id>/
├── README.md        # question, plan, result, interpretation
├── config/          # small reproducibility configs
├── results/         # compact machine-readable summaries
└── figures/         # generated plots used in reports
```

Large raw outputs belong outside Git or in an archival artifact store; record checksums and provenance here.

## Minimum experiment record

```markdown
# <Experiment ID>: <title>

Status: planned | running | result | replicated | superseded
Date:
Code commit/tag:

## Research question

## Hypothesis
A falsifiable statement written before looking at the result.

## Setup
- hardware/runtime
- model/data versions
- seeds/repetitions
- independent/dependent variables

## Baselines and controls

## Metrics

## Predeclared success/failure criteria

## Procedure

## Results
Include failures, exclusions, uncertainty, and sample counts.

## Interpretation
Separate measured observations from explanations.

## Limitations / alternative explanations

## Reproduction
Exact commands and config paths.

## Provenance and licenses
Datasets, models, external APIs, citations, and applicable terms.
```

## Rules

1. Do not silently replace a failed result with a later successful rerun.
2. Distinguish the best checkpoint/run from the final checkpoint/run when they differ.
3. Record exclusions and stopping decisions.
4. Keep observations separate from causal interpretation.
5. Link the exact Git commit/tag used.
6. For third-party datasets, follow [`../../data/README.md`](../../data/README.md) and [`../../LICENSING.md`](../../LICENSING.md).
7. Promotion to architecture/specification requires explicit review; an experiment does not automatically become a design requirement.

When a narrow claim becomes strong enough for publication, create a corresponding package under [`../../papers/`](../../papers/).
