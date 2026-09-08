# Kamimusuhi Docs

This directory is a **living knowledge base** for the Kamimusuhi project.

Research, implementation findings, useful external projects, benchmark results, and architecture decisions should be written down here rather than remaining only in chat history.

## Knowledge-capture policy

When new information materially affects Kamimusuhi, prefer to record it in `docs/` during the same research/design cycle.

Capture at least:

- the finding or idea;
- why it matters to Kamimusuhi;
- which subsystem/Issue it affects;
- evidence / paper / implementation link where applicable;
- uncertainties and unverified claims;
- what experiment or benchmark should validate it.

Do not turn speculative discussion into fact. Clearly distinguish:

```text
established external result
our interpretation
our design hypothesis
future experiment
```

Docs are expected to evolve aggressively. Architecture files remain the normative design; research docs may contain competing ideas.

## Current index

### Core research

- [`research-foundations.md`](./research-foundations.md) — ALife, cognitive architectures, neuroscience/psychology, memory, extended cognition, persistent identity, tools, sleep, interoception.

### Native cognition / model training

- [`persona-core-training-taxonomy.md`](./persona-core-training-taxonomy.md) — what a Kamimusuhi Persona Core should learn: self/other, human/social cognition, epistemology, delegation, persona priors, long-horizon relationship competence, evaluation.

### Reflex / control substrate

- [`learned-discrete-reflex-layer.md`](./learned-discrete-reflex-layer.md) — learned low-latency nervous-system layer beneath K-Edge: DLGN/LDLGN, DWN/LUT controllers, stochastic-computing inspiration, temporal state, teacher distillation, shadow deployment, auditability, and benchmark plan.

### Sensory and motor embodiment

- [`sensory-nervous-system.md`](./sensory-nervous-system.md) — vision, hearing, touch, environment, chemical senses, proprioception, interoception, sensory-event bus, reflexes, multisensory binding, neuromorphic path.
- [`speech-and-vocal-expression.md`](./speech-and-vocal-expression.md) — speech as motor/social output: utterance planning, prosody, full-duplex dialogue, backchannels, interruption, vocal identity, self-monitoring, streaming synthesis, distributed vocal embodiment.

### Performance

- [`latency-architecture.md`](./latency-architecture.md) — immediate-response architecture, K-Edge/K-Core escalation, prompt/KV caching, progressive retrieval, speculative decoding, latency metrics.

### External projects / technologies

- [`technology-watchlist.md`](./technology-watchlist.md) — OpenLife, Tebasaki_lab/MagicDelta, artificial nervous systems, llama.cpp, MLX-LM, continuity research, social-cognition benchmarks.
- [`fio-system-reference-analysis.md`](./fio-system-reference-analysis.md) — FIO's long-running persistence discipline: canonical evidence vs derived renderings, layered/verbatim memory, internal-thought boundaries, background-cognition freshness, chokepoint routing, and silent-defect detection; mapped to Kamimusuhi with adoption hypotheses and experiments.

## Normative project documents

At repository root:

- [`../README.md`](../README.md) — project thesis and orientation;
- [`../spec.md`](../spec.md) — normative behavioral/system requirements;
- [`../architecture.md`](../architecture.md) — normative architectural boundaries and evolution direction.

## Suggested future docs

Create focused notes rather than endlessly extending one monolithic file. Likely future areas include:

- `memory-and-dream-system.md`;
- `distributed-embodiment.md`;
- `continuity-and-fork-semantics.md`;
- `social-cognition-benchmarks.md`;
- `persona-core-dataset-design.md`;
- `neuromorphic-edge.md`;
- `security-and-self-integrity.md`;
- `ethology-and-development.md`;
- `benchmarks/` for measured local hardware/model results;
- `adrs/` for durable Architecture Decision Records.
