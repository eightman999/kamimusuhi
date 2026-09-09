# Kamimusuhi Research

This directory is the **research map and synthesis layer** for Kamimusuhi.

It is intentionally different from the existing research notes:

- [`../research-foundations.md`](../research-foundations.md) records detailed intellectual foundations and design interpretation;
- [`../technology-watchlist.md`](../technology-watchlist.md) tracks external projects, people, technologies, and claims that should be watched;
- [`../daily-surveys/`](../daily-surveys/) is intended to hold date-scoped discovery logs;
- [`landscape.md`](./landscape.md) answers the higher-level question: **where does Kamimusuhi sit in the research landscape, what prior art maps to each subsystem, and what remains genuinely open?**

## Documents

- [`landscape.md`](./landscape.md) — cross-disciplinary research landscape, comparison matrix, research gaps, and prioritized experimental agenda. Baseline: 2026-09-08.
- [`fio-neural-function-growth-2026-09-08.md`](./fio-neural-function-growth-2026-09-08.md) — Markdown transcription of FIO's *神経関数が育つ* experiment record plus Kamimusuhi analysis of descending modulation, perceptual gating vs motor reflex, anti-self-blinding safeguards, shadow comparison, and reproducibility metrics.
- [`fio-system-architecture-2026-09-09.md`](./fio-system-architecture-2026-09-09.md) — source-grounded review of FIO / SerialExperiments' 2026-09-09 measured architecture: conscious/unconscious timing split, 0.25-second nervous heartbeat, two-tier memory, sensory gating, per-nerve adaptive state, embodiment as observability, inference-vessel abstraction, single decision points, runtime-evidence discipline, and implications for Kamimusuhi's K-Fast / FlyBrain-inspired multi-timescale design.

## Promotion flow

Daily discoveries should not automatically become architectural truth.

```text
public information
      |
      v
daily survey
      |
      +--> weak / duplicate / promotional -> keep as survey evidence
      |
      +--> important external line -> technology-watchlist.md
      |
      +--> durable conceptual foundation -> research-foundations.md
      |
      +--> changes the comparative map -> research/landscape.md
      |
      +--> validated design decision -> spec / architecture / ADR
```

For every promoted item, preserve the distinction among:

```text
established external result
external proposal / hypothesis
Kamimusuhi interpretation
Kamimusuhi design hypothesis
measured local result
```

## Update policy

The landscape should be updated when at least one of the following occurs:

1. a new project substantially overlaps the full Kamimusuhi architecture;
2. a new result changes the plausibility of a core subsystem;
3. an area previously considered novel becomes established prior art;
4. a Kamimusuhi hypothesis is experimentally tested;
5. a daily survey reveals a new research lineage that changes the map.

Do not expand the landscape into an indiscriminate bibliography. Detailed notes belong in focused documents; the landscape should remain navigable.