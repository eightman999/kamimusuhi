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

### Implementation plans

- [`implementation/phase-2-implementation-plan.md`](./implementation/phase-2-implementation-plan.md) — **第二次実装計画**。W6（Cognitive Resource Router と TLS）の acceptance criteria と、W7 以降へ進まない境界を明記する。計画であり実装済みではない。
- [`implementation/phase-1-implementation-result.md`](./implementation/phase-1-implementation-result.md) — **第一次実装 (W0–W5) の結果記録**。Issue #8 の acceptance を1項目ずつ実測で照合し、v0.1 が証明したこと・していないこと・既知の制約を事実ベースで記述する。計画ではない。
- [`implementation/w5-external-resources.md`](./implementation/w5-external-resources.md) — **W5 実装記録**。external cognitive resource を実 HTTP 経由で呼ぶ際の境界: adapter responsibility、logical call と physical attempt の区別、timeout/retry の所有者、error 分類、OpenAI-compatible adapter とその TLS 制約、secret を保存しない設計、provider 交換の identity 不変性、`resource_calls` の turn correlation、ID seed と clock の分離。計画ではなく実装済みの内容のみを記述する。
- [`implementation/phase-1-implementation-plan.md`](./implementation/phase-1-implementation-plan.md) — **第一次実装計画書**。v0.1 continuity vertical slice を実コードへ落とすため、Rust + SQLite single-writer、Cargo workspace、schema v1、atomic activation、Fake Persona Core / resource A・B、restart demo、failpoint test、local CI、Wave W0–W5、Definition of Done を具体化。現時点では計画であり実装済みではない。

### 器官設計監査 / Organ design audit

- [`audits/2026-09-08-organ-design-audit.md`](./audits/2026-09-08-organ-design-audit.md) — 日本語。固定 commit を基準に26機能領域を監査し、18件の横断的な設計不足・接続未確定点、既存設計、優先順位、確認限界を整理。実装済みという判定ではない。
- [`organ-contracts-and-implementation-plan.md`](./organ-contracts-and-implementation-plan.md) — 日本語。状態所有者、event/信念/正典変更/目標/行為/音声/停止復旧/忘却学習の契約案、28件の未実施テスト仕様、段階的な有効化 gate。規範仕様へ未昇格の提案。
- [`research/2026-09-08-organ-audit-literature.md`](./research/2026-09-08-organ-audit-literature.md) — 日本語。関連文献21本を本文該当節・一次要旨・書誌のみの確認深度で区別し、適用先と一般化できない点を記録。

### Core research

- [`research/README.md`](./research/README.md) — research-map index and promotion flow from daily survey to durable research/design knowledge.
- [`research/landscape.md`](./research/landscape.md) — cross-disciplinary landscape mapping prior art to Kamimusuhi subsystems, differences, research gaps, core research questions, and prioritized experiments.
- [`daily-surveys/README.md`](./daily-surveys/README.md) — dated cross-domain research intake and index; new papers/projects enter here before promotion into durable research or architecture notes.
- [`research-foundations.md`](./research-foundations.md) — ALife, cognitive architectures, neuroscience/psychology, memory, extended cognition, persistent identity, tools, sleep, interoception.

### Native cognition / model training

- [`persona-core-training-taxonomy.md`](./persona-core-training-taxonomy.md) — what a Kamimusuhi Persona Core should learn: self/other, human/social cognition, epistemology, delegation, persona priors, long-horizon relationship competence, evaluation.
- [`model-ecology-and-lineage.md`](./model-ecology-and-lineage.md) — model ecology across two scales: a medium `novllm`-derived Persona Core plus small specialist-model organs inside each individual, and a multi-user population of distinct individual lineages with privacy-gated population-level inheritance.
- [`optional-p2p-population-learning.md`](./optional-p2p-population-learning.md) — **optional / non-normative** long-horizon note for privacy-preserving P2P population learning: signed artifacts and evaluation receipts, E2EE plus secure aggregation/privacy controls, Sybil resistance, heterogeneous distributed evaluation, federated/volunteer compute, and species-level inheritance without merging individual identity or private lived experience.

### Reflex / control substrate

- [`multiscale-brain-architecture.md`](./multiscale-brain-architecture.md) — provisional multi-timescale nervous-system architecture: PNL → K-Fast → Global Workspace/K-Edge → Persona Core, with independent timing domains, event-based crossings, fly-inspired sparse recurrent regulation, affect-like modulation, and a 1k–10k-node K-Fast prototype path.
- [`peripheral-neural-layer.md`](./peripheral-neural-layer.md) — provisional architecture decision for the Peripheral Neural Layer (PNL): learned discrete controllers belong primarily in the peripheral nervous/reflex system, below semantic/LLM cognition, with deterministic safety gates, upward reflex receipts, body-specific adaptation, and backend-independent deployment.
- [`learned-discrete-reflex-layer.md`](./learned-discrete-reflex-layer.md) — learned low-latency nervous-system layer beneath K-Edge: DLGN/LDLGN, DWN/LUT controllers, stochastic-computing inspiration, temporal state, teacher distillation, shadow deployment, auditability, and benchmark plan.
- [`fly-brain-connectome-reference.md`](./fly-brain-connectome-reference.md) — FlyWire/Shiu/Eon reference analysis: validated connectome-derived LIF sensorimotor results, multi-backend `fly-brain` implementation, 2026 embodied-loop caveats, mechanistic criticism, K-Nerve mapping, ablation tests, backend-parity methodology, and the K-Fast/multi-timescale extension.

### Sensory and motor embodiment

- [`sensory-nervous-system.md`](./sensory-nervous-system.md) — vision, hearing, touch, environment, chemical senses, proprioception, interoception, sensory-event bus, reflexes, multisensory binding, neuromorphic path.
- [`speech-and-vocal-expression.md`](./speech-and-vocal-expression.md) — speech as motor/social output: utterance planning, prosody, full-duplex dialogue, backchannels, interruption, vocal identity, self-monitoring, streaming synthesis, distributed vocal embodiment.

### Performance

- [`latency-architecture.md`](./latency-architecture.md) — immediate-response architecture, K-Edge/K-Core escalation, prompt/KV caching, progressive retrieval, speculative decoding, latency metrics.
- [`native-runtime-stratification.md`](./native-runtime-stratification.md) — polyglot runtime strategy: native K-Edge/runtime hot path, Python research/control plane, long-lived executables, FFI/IPC, SIMD/assembly policy, and profiler-driven migration.
- [`runtime-language-and-hardware-compatibility.md`](./runtime-language-and-hardware-compatibility.md) / [`EN`](./runtime-language-and-hardware-compatibility.en.md) — current preferred implementation policy: Rust always-resident Core, Python research/training plane, bounded WASM/script/DSL behavior layer, conservative baseline binaries, optional ISA/GPU accelerator organs, capability discovery, and graceful degradation across old or heterogeneous hardware.

### Failure modes / implementation lessons

- [`persistent-agent-implementation-pitfalls.md`](./persistent-agent-implementation-pitfalls.md) — cross-cutting failure-mode catalogue distilled from Yui/FIO and mapped to Kamimusuhi: memory/compaction, provisional state, task delivery, tool grounding, confirmation vs execution, crash ambiguity, transactional jobs/outbox, scheduler ownership, relationship-state boundaries, source-of-truth, and 12 end-to-end experiments.

### External projects / technologies

- [`technology-watchlist.md`](./technology-watchlist.md) — OpenLife, Tebasaki_lab/MagicDelta, artificial nervous systems, llama.cpp, MLX-LM, continuity research, social-cognition benchmarks.
- [`fio-system-reference-analysis.md`](./fio-system-reference-analysis.md) — FIO's long-running persistence discipline: canonical evidence vs derived renderings, layered/verbatim memory, internal-thought boundaries, background-cognition freshness, chokepoint routing, and silent-defect detection; mapped to Kamimusuhi with adoption hypotheses and experiments.
- [`research/fio-system-architecture-2026-09-09.md`](./research/fio-system-architecture-2026-09-09.md) — FIO / SerialExperiments' measured 2026-09-09 architecture snapshot: conscious/unconscious process split, 0.25-second nervous heartbeat, two-tier memory, sensory gating, local adaptive nerves, embodiment as observability, replaceable inference vessels, and a Kamimusuhi mapping to K-Fast / FlyBrain-inspired multi-timescale control.

## Normative project documents

At repository root:

- [`../README.md`](../README.md) — project thesis and orientation;
- [`../spec.md`](../spec.md) — normative behavioral/system requirements;
- [`../architecture.md`](../architecture.md) — normative architectural boundaries and evolution direction;
- [`../model-ecology-architecture.md`](../model-ecology-architecture.md) — architecture addendum for the Persona Core / specialist-organ model ecology, distinct per-user individual lineages, and privacy-gated inheritance across a population.

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
