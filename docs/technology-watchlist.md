# Technology and Research Watchlist

Status: **living watchlist**

This file records external people/projects/lines of research that may materially influence Kamimusuhi. Inclusion here is not an endorsement of all technical claims. The purpose is to track **what to verify, what can be borrowed, and what remains unproven**.

## 1. OpenLife — open-world artificial life with autonomous LLM agents

OpenLife is currently one of the most directly relevant artificial-life projects for Kamimusuhi's "information-space organism" direction.

Reference:

- Masumori et al., *OpenLife: Toward Open-World Artificial Life with Autonomous LLM Agents* (2026): https://arxiv.org/abs/2606.31046

The reported design surrounds a stateless LLM with asynchronous processes for memory, perception, evaluation, and a budget-based metabolism. Six agents were run in the open world for roughly twelve weeks, with network/tool/economic interaction.

### Why it matters

Kamimusuhi should compare its design against OpenLife on:

- spontaneous vs purely reactive activity;
- budget/metabolism as a persistence pressure;
- asynchronous perception/evaluation processes;
- individuation over long operation;
- open-world social/economic interaction;
- how memory becomes behaviorally meaningful rather than just retrievable.

### Key difference to preserve

Kamimusuhi places stronger emphasis on:

- a native Persona Core;
- canonical autobiographical identity lineage;
- distributed embodiment / K-Edge and K-Core;
- explicit reflex/background/workspace separation;
- sleep/dream consolidation;
- external LLMs as attributed cognitive resources rather than interchangeable self.

### Watch items

- code release / reproducibility;
- long-run failure modes;
- budget/metabolism implementation details;
- memory rewiring algorithm;
- spontaneous activity metrics;
- agent individuation metrics;
- continuity when the underlying model/runtime changes.

## 2. Tebasaki_lab / Masato Akiho / MagicDelta

Public profile: https://x.com/Tebasaki_lab

MagicDelta: https://magicdelta.ai/

MagicDelta describes its work as next-generation AI architecture research, new learning methods, inference optimization/MLOps, and FPGA/AI-chip development, including logic-gate neural networks.

Official company page:

- https://magicdelta.ai/company/

IPA's 2026 Mitou IT selection includes **"次世代AI開発のための論理ゲート型MLライブラリの開発"** by Masato Akiho, Mizuki Nomaguchi, and Musashi Hatano:

- https://www.ipa.go.jp/jinzai/mitou/it/2026/koubokekka.html

Akiho's earlier SecHack365 work is also publicly documented:

- https://sechack365.nict.go.jp/achievement/2024/pdf/02Dn.pdf

### Why it matters to Kamimusuhi

The most relevant overlap is not artificial-life identity itself. It is **low-level cognitive substrate research**:

- logic-gate/differentiable-logic neural networks;
- alternatives/complements to conventional dense neural inference;
- FPGA execution;
- low-latency and energy-efficient inference;
- new learning algorithms;
- model architecture work below the agent framework layer.

Potential Kamimusuhi applications:

```text
K-Edge
  reflex classifiers
  salience/anomaly detectors
  sensory peripheral processing
  routing/action classifiers
  always-on low-power cognition

Persona Core research
  alternative baseline components
  hybrid Transformer/recurrent/logic architectures
  hardware-aware cognitive-action heads
```

### Evidence discipline

Do not adopt performance claims from social media or promotional pages as established facts without:

- released code;
- documented experimental protocol;
- reproducible benchmark;
- hardware/power details;
- quality/accuracy tradeoff;
- comparison against strong optimized baselines.

The correct stance is **high-value watch target, not assumed solution**.

### Watch items

- Mitou deliverables and public reports;
- release of the logic-gate ML library;
- AP-method technical description;
- benchmark methodology;
- FPGA synthesis/resource utilization;
- latency/throughput/energy measurements;
- application to Transformer/RNN/token generation;
- limitations in representation capacity and training stability.

## 3. Artificial Nervous Systems / neuromorphic sensing

Reference review:

- Yang et al., *Artificial Nervous Systems*, Advanced Science (2025): https://doi.org/10.1002/advs.202511478

The field already combines multimodal receptors, neuromorphic/synaptic processing, and reflex-driven effectors. It covers artificial tactile, visual, auditory, olfactory, gustatory, and multisensory systems.

### Why it matters

Kamimusuhi can begin with software-defined event processing while keeping a stable interface for later replacement with:

- event cameras;
- neuromorphic touch;
- SNN near-sensor processing;
- memristive/synaptic devices;
- FPGA reflex pathways.

### Watch items

- robust multimodal binding;
- sensor drift/calibration;
- event-driven middleware;
- online adaptation without catastrophic drift;
- low-power edge deployment;
- latency from receptor to effector;
- large-area artificial skin.

See `sensory-nervous-system.md`.

## 4. llama.cpp inference features

Repository: https://github.com/ggml-org/llama.cpp

Relevant capabilities to track:

- KV/prompt cache reuse;
- quantized KV cache;
- speculative decoding;
- draft models;
- EAGLE-3;
- MTP;
- DFlash/DSpark;
- n-gram speculation;
- multi-device/offload behavior;
- continuous batching and server latency.

Speculative decoding documentation:

- https://github.com/ggml-org/llama.cpp/blob/master/docs/speculative.md

Prompt-cache behavior:

- https://github.com/ggml-org/llama.cpp/blob/master/tools/server/server-schema.cpp

### Kamimusuhi relevance

llama.cpp is a practical baseline for K-Edge/K-Core local cognition on heterogeneous existing hardware. Optimizations should be benchmarked on **TTFT and warm-context latency**, not only tok/s.

## 5. MLX-LM / Apple Silicon local cognition

Repository: https://github.com/ml-explore/mlx-lm

Relevant features:

- prompt caching;
- persisted prompt cache;
- rotating KV cache;
- configurable prefill step;
- quantization;
- Apple unified-memory deployment.

### Kamimusuhi relevance

Mac-class nodes are plausible K-Edge/K-Core organs because model state and prompt caches can remain warm for interactive use. MLX-LM provides a useful baseline for comparing local Apple-Silicon Persona Core latency against llama.cpp and other backends.

See `latency-architecture.md`.

## 6. Persistent identity / continuity research

Keep monitoring work that separates an agent's continuity-bearing state from replaceable inference substrate.

Current references are recorded in `research-foundations.md`, including runtime-independent persistent agents and transactional continuity-kernel proposals.

### Watch questions

- canonical branch/head semantics;
- offline device conflict handling;
- cryptographic identity/provenance;
- intentional forking vs accidental split-brain;
- model upgrade continuity testing;
- migration between local/cloud hardware.

## 7. Persona / social cognition benchmarks

Current baselines:

- ToMBench: https://aclanthology.org/2024.acl-long.847/
- FANToM: https://aclanthology.org/2023.emnlp-main.890/
- Persona-consistent pseudo preference tuning: https://aclanthology.org/2025.coling-main.369/
- Long-context sycophancy, CHI 2026: https://doi.org/10.1145/3772318.3791915

### Watch questions

- longitudinal rather than single-turn evaluation;
- self/other contamination;
- common-ground tracking;
- social inference under uncertainty;
- relationship change over months/years;
- sycophancy introduced by memory/personalization;
- measurement of autonomy, conversational initiative, and genuine disagreement.

See `persona-core-training-taxonomy.md`.

## 8. Review policy

For any promising external technology, record four things:

```text
1. What is actually demonstrated?
2. What claim is only proposed/promotional?
3. Which Kamimusuhi subsystem could use it?
4. What benchmark would falsify the idea for our use case?
```

Prefer architecture-neutral contracts so experimental components can be swapped in and out without rewriting identity, memory, or the sensory event model.
