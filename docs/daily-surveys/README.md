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

## Index

| Date | Highlights |
|---|---|
| [2026-09-10 19:00 delta](./2026-09-10-19-00-51.md) | explicit-state vs generative-render separation, typed control/data separation, semantic embodiment ABI, runtime agent principals, relationship-hypothesis adjudication, lifecycle trace audits, direct memory ingestion, trace-based skill contracts; Zenn/note/Qiita discovery validation |
| [2026-09-10 13:00 delta](./2026-09-10-13-00-55.md) | capability-scoped authority, enforced memory revocation, evidence-preserving hypergraph memory, predictive-coding internal/external regulation, experience-learning benchmarks, grounded skill synthesis and consolidation |
| [2026-09-10 07:01 delta](./2026-09-10-07-01-34.md) | lease/fencing continuity, relational-belief grounded social cognition, co-located assistance triggers, uncertainty-aware group dialogue, temporal representation-loss auditing, curiosity-driven coverage |
| [2026-09-10 01:01 delta](./2026-09-10-01-01-27.md) | information-evolution replay, action-level memory utility, model–harness compatibility, independent evaluator separation, metanorm social cognition, sensor-organ adaptation, event-driven distributed compute |
| [2026-09-10](./2026-09-10.md) | procedural graphs, model-upgrade memory portability, negative-utility memory clearance, fast-state/slow-policy consolidation, repeated-run consistency, environmental copying, adaptive visuo-tactile gating |
| [2026-09-09](./2026-09-09.md) | cross-substrate authority enforcement, full-duplex Cerebellum/Brain interaction, stable skill evolution, harness-mediated model self-improvement, biometric memory isolation, minimal-sufficient personalization |
| [2026-09-08](./2026-09-08.md) | epistemic assertion guardrails, authorization laundering, security-context consequence integrity, continuous cognitive coverage, identity-centric multimodal memory, drift-vs-poisoning, adaptive routing, KV workspace virtualization |
