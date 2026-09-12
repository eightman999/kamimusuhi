# Kamimusuhi Daily Research Surveys

This directory contains dated, cross-domain research intake for Kamimusuhi.

Daily surveys are **non-normative**. They record newly surfaced papers, projects, implementations, talks, technical reports, design proposals, and relevant speculative work before those findings are promoted into longer-lived research or architecture documents.

## Survey scope

The recurring survey spans:

- artificial life and digital organisms;
- cognitive architectures and workspace/modular cognition;
- persistent AI agents and long-term memory;
- self-models, autobiographical/narrative identity, Theory of Mind, and social cognition;
- neuroscience, psychology, cybernetics, homeostasis, and active inference;
- open-ended learning/evolution and self-improving systems;
- tool/model orchestration and agent harnesses;
- distributed systems, identity continuity, provenance, and authorization;
- multimodal perception, robotics, neuromorphic and low-latency cognition;
- HCI, lifelong companions, design fiction, and related SF concepts.

Discovery sources include primary papers, official project/specification pages, GitHub repositories and research-lab materials, plus **Zenn, note, and Qiita** for implementation reports, practitioner failure modes, Japanese-language ideas, and leads to primary sources.

## Evidence discipline

Each promoted item should distinguish:

1. what was actually demonstrated;
2. what remains proposed or speculative;
3. the relevant Kamimusuhi subsystem;
4. the design/implementation idea worth borrowing;
5. the important difference from Kamimusuhi;
6. a concrete falsifiable next experiment;
7. importance (`S` / `A` / `B` / `C`).

Prefer primary papers, official project pages, and released artifacts. Social-media, Zenn, note, Qiita, or other secondary/promotional claims should be treated as leads unless the article itself is the primary implementation record; generalized technical, performance, and research claims should be checked against primary sources where possible.

## Promotion flow

```text
daily survey
    |
    +--> no durable significance -> remains dated intake only
    |
    +--> recurring/important technology -> ../technology-watchlist.md
    |
    +--> established conceptual foundation -> ../research-foundations.md
    |
    +--> comparative positioning -> ../research/landscape.md
    |
    +--> implementation failure mode -> subsystem/pitfall note
    |
    +--> validated design decision -> architecture.md / spec.md via explicit review
```

A daily survey should not silently turn a preprint, analogy, or benchmark result into a normative architecture requirement.

## Reflection decisions / 反映判断

[2026-09-11 判定台帳](../research/survey-decisions-2026-09-11.md)に、文書別の採否・理由・Issue反映先・保留条件・判断対象blobを記録した。**`済✅️` は反映するか否かの判断完了であり、全提案の採用・実装完了・論文の全面検証を意味しない。** 原文の研究記録は変更しない。

New surveys start as **未判定**. Record an explicit decision and confirm any intended Issue/spec/README write before marking a survey **済✅️**. Re-review changed source blobs and materially changed evidence or implementation conditions; never copy an older completion marker onto new findings. `S` / `A` importance does not automatically promote a proposal into a normative requirement.

## Index

| Date | Highlights | 反映判断 |
|---|---|---|
| [2026-09-12 13:01 delta](./2026-09-12-13-01-17.md) | control-boundary retention across compaction, dependency-consistent multimodal deep research, group-affect divergence, held-out modality utility, cross-architecture output information geometry, TTFS SNN expressivity, agent-evaluation coverage taxonomy | **未判定** |
| [2026-09-12 06:58 delta](./2026-09-12-06-58-54.md) | capability-bound routing, recurrent variable-depth cognition, retrospective streaming speech reasoning, differentiable hard-action feasibility, visual-need-gated raw sensory revisit, synthetic-data supplier-lineage collapse risk, persona specialization vs general-capability retention, stage-aware verifiable HCI | **未判定** |
| [2026-09-12 00:57 delta](./2026-09-12-00-57-48.md) | causal-visibility-aware distributed guards, cognitive-ABI migration and latent-protocol negative transfer, computation/energy coupling as ALife pressure, task-agnostic pre-study, action-grounded transition memory, hybrid reflex modulation, tail-aware workflow admission, verified autonomous skill acquisition | **未判定** |
| [2026-09-11 19:02 delta](./2026-09-11-19-02-25.md) | physics-grounded reactive-safety failures that do not shrink with model scale, failure-domain diagnosis for harness self-improvement, factorization-diverse multi-model adjudication, executable planning state, unified embodied runtime, NVIDIA derived self-model + durable ledger; note→NVIDIA primary verification | **未判定** |
| [2026-09-11 13:02 delta](./2026-09-11-13-02-59.md) | environment-probing memory curation, agent-side memory/stateless action organ, story-imprinting self/other training contamination, update-admission learning starvation, latent failure readouts, calibrated model cascades, evaluation-budgeted skill evolution, intrinsic drive; note→KDDI robotics verification | **未判定** |
| [2026-09-11 07:01 delta](./2026-09-11-07-01-29.md) | behavior-grounded self-model vs generic self-report, cross-authority outcome completion admission, state-path tool menus, probabilistic bounded-rational partner models, relation-aware multimodal support, procedural-memory non-interference boundary, subagent skill execution, FPGA-native learned LUTs, active-inference epistemic action selection; note→Microsoft memory governance verification | [済✅️](../research/survey-decisions-2026-09-11.md#d09) |
| [2026-09-11 01:03 delta](./2026-09-11-01-03-55.md) | explicit external belief-state projection, lifecycle-aware memory validity/supersession, deterministic multi-agent result merging, reality-settled self-improvement evaluation, homeostatic-controller alternatives, confidence-aware multimodal gating, concurrent memory false-success failures; Zenn/AWS implementation verification | [済✅️](../research/survey-decisions-2026-09-11.md#d08) |
| [2026-09-10 19:00 delta](./2026-09-10-19-00-51.md) | explicit-state vs generative-render separation, typed control/data separation, semantic embodiment ABI, runtime agent principals, relationship-hypothesis adjudication, lifecycle trace audits, direct memory ingestion, trace-based skill contracts; Zenn/note/Qiita discovery validation | [済✅️](../research/survey-decisions-2026-09-11.md#d07) |
| [2026-09-10 13:00 delta](./2026-09-10-13-00-55.md) | capability-scoped authority, enforced memory revocation, evidence-preserving hypergraph memory, predictive-coding internal/external regulation, experience-learning benchmarks, grounded skill synthesis and consolidation | [済✅️](../research/survey-decisions-2026-09-11.md#d06) |
| [2026-09-10 07:01 delta](./2026-09-10-07-01-34.md) | lease/fencing continuity, relational-belief grounded social cognition, co-located assistance triggers, uncertainty-aware group dialogue, temporal representation-loss auditing, curiosity-driven coverage | [済✅️](../research/survey-decisions-2026-09-11.md#d05) |
| [2026-09-10 01:01 delta](./2026-09-10-01-01-27.md) | information-evolution replay, action-level memory utility, model–harness compatibility, independent evaluator separation, metanorm social cognition, sensor-organ adaptation, event-driven distributed compute | [済✅️](../research/survey-decisions-2026-09-11.md#d04) |
| [2026-09-10](./2026-09-10.md) | procedural graphs, model-upgrade memory portability, negative-utility memory clearance, fast-state/slow-policy consolidation, repeated-run consistency, environmental copying, adaptive visuo-tactile gating | [済✅️](../research/survey-decisions-2026-09-11.md#d03) |
| [2026-09-09](./2026-09-09.md) | cross-substrate authority enforcement, full-duplex Cerebellum/Brain interaction, stable skill evolution, harness-mediated model self-improvement, biometric memory isolation, minimal-sufficient personalization | [済✅️](../research/survey-decisions-2026-09-11.md#d02) |
| [2026-09-08](./2026-09-08.md) | epistemic assertion guardrails, authorization laundering, security-context consequence integrity, continuous cognitive coverage, identity-centric multimodal memory, drift-vs-poisoning, adaptive routing, KV workspace virtualization | [済✅️](../research/survey-decisions-2026-09-11.md#d01) |