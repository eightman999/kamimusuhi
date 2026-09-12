# Kamimusuhi research workflow

Kamimusuhi is developed as a **living open-research program**, not as one monolithic paper. The repository should make it possible to distinguish vision, normative design, implementation, observations, hypotheses, experiments, and publication claims.

## Distilled findings / 蒸留した知見

[`RESEARCH_SYNTHESIS.md`](./RESEARCH_SYNTHESIS.md) is the top-level, non-normative synthesis of surveyed findings for Kamimusuhi. It connects continuity, authority, memory, fast/slow cognition, learning, and development to concrete uses, limitations, and prioritized controlled experiments. Read it before turning an intake note into a new implementation task; the source surveys and adoption ledger remain the evidence and decision history.

## Repository map

```text
README.md / README.en.md      project overview
RESEARCH_SYNTHESIS.md         distilled findings and experiment priorities (non-normative)
spec.md / spec.en.md          normative requirements
architecture.md               target architecture
model-ecology-architecture.md normative/design addendum
crates/                       executable implementation
scripts/                      demos, checks, research utilities
docs/                         knowledge base and working research record
  research/                   literature synthesis and research landscape
  experiments/                experiment plans and reports
  implementation/             implementation plans, audits, measured results
  daily-surveys/              dated broad surveys
  hourly-blindspot-surveys/   dated adjacent-field intake
  field-notes/                observations and external-system notes
papers/                       claim-centered manuscripts/publication packages
data/                         dataset provenance boundary; raw data is local by default
```

The root normative documents stay intentionally easy to find. Historical research notes do not become normative merely because they are public.

## Research states

Use explicit status labels where practical:

- `idea` — a question or conjecture, not yet an experiment;
- `planned` — method and success/failure criteria are written before execution;
- `running` — data collection or training is in progress;
- `result` — execution completed and raw/derived evidence is recorded;
- `replicated` — repeated independently enough to support a stronger claim;
- `paper-candidate` — a narrow claim has sufficient evidence for a manuscript;
- `published` — archived preprint or peer-reviewed publication exists;
- `superseded` — retained for history but replaced by a newer result/design.

Negative and null results are part of the research record and should not be silently deleted.

## Experiment → paper boundary

An experiment report answers **what happened under a specified setup**. A paper makes a narrower, defensible **claim** from one or more experiments and related work.

Before an experiment is promoted to `paper-candidate`, record at least:

1. research question and falsifiable hypothesis;
2. baseline/control conditions;
3. seeds/repetitions and stopping rule;
4. metrics and success/failure criteria;
5. exact code/config/version information;
6. results including failed runs and exclusions;
7. limitations and alternative explanations;
8. data/model provenance and licenses.

See [`docs/experiments/README.md`](./docs/experiments/README.md).

## Public priority and citation

Git history is useful provenance, but important research milestones should be frozen as versioned releases. [`CITATION.cff`](./CITATION.cff) provides machine-readable citation metadata; when an archival DOI is created for a release, add it there and prefer that DOI in papers.

A public note is not treated as proof merely because it predates later work. Priority should be backed by a sufficiently specific, inspectable artifact: method, result, version, and date.

## Third-party research material

Open research does not mean relicensing upstream material. Dataset/model/service terms remain independent of the Kamimusuhi MIT license. See [`LICENSING.md`](./LICENSING.md), [`data/README.md`](./data/README.md), and [`THIRD_PARTY_NOTICES.md`](./THIRD_PARTY_NOTICES.md).
