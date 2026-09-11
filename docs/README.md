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

### Implementation plans and results

- [`implementation/2026-09-10-spec-implementation-audit.md`](./implementation/2026-09-10-spec-implementation-audit.md) — **仕様対実装の監査と修正結果**。Persona privacy、HTTP framing/size、safe diagnostics、完全な input provenance、logical-call deadline の修正。実行した CI と mutation test、実モデル未検証、残る制約を区別する。

- [`implementation/w7-persona-core-plan.md`](./implementation/w7-persona-core-plan.md) — **W7 計画と実装結果**。Persona Core 境界の強化と OpenAI-compatible backend（自動検証は scripted endpoint）。external material と user-facing expression の分離、typed input envelope、Persona backend と cognitive resource の役割分離、失敗時の canonical state 不変性。W8 以降へ進まない境界を明記する。
- [`implementation/phase-2-implementation-plan.md`](./implementation/phase-2-implementation-plan.md) — **第二次実装計画と W6 実装結果**。Cognitive Resource Router / TLS の acceptance と当時の wave 境界を記録。後半に実装結果があり、W7 は別の計画・結果文書を参照する。
- [`implementation/phase-1-implementation-result.md`](./implementation/phase-1-implementation-result.md) — **第一次実装 (W0–W5) の結果記録**。Issue #8 の acceptance を1項目ずつ実測で照合し、v0.1 が証明したこと・していないこと・既知の制約を事実ベースで記述する。計画ではない。
- [`implementation/w5-external-resources.md`](./implementation/w5-external-resources.md) — **W5 実装記録**。external cognitive resource を実 HTTP 経由で呼ぶ際の境界: adapter responsibility、logical call と physical attempt の区別、timeout/retry の所有者、error 分類、OpenAI-compatible adapter とその TLS 制約、secret を保存しない設計、provider 交換の identity 不変性、`resource_calls` の turn correlation、ID seed と clock の分離。計画ではなく実装済みの内容のみを記述する。
- [`implementation/phase-1-implementation-plan.md`](./implementation/phase-1-implementation-plan.md) — **第一次実装計画書**。v0.1 continuity vertical slice を実コードへ落とすため、Rust + SQLite single-writer、Cargo workspace、schema v1、atomic activation、Fake Persona Core / resource A・B、restart demo、failpoint test、local CI、Wave W0–W5、Definition of Done を具体化。当時の計画であり、実装の達成範囲は別の phase-1 implementation result を参照する。

### 実機実験 / Experiments

- [`experiments/k-edge-android-a523-profile-2026-09-12.md`](./experiments/k-edge-android-a523-profile-2026-09-12.md) — **K-Edge 候補となる廉価 Android/ARM64 実機の素体プロファイル**。`heterogeneous-cognitive-compute-substrate.md` §5 の capability discovery 項目を非 root の実機で埋め、K-Edge 側に実測済み body が 1 件も無い状態を解消する。全コア Cortex-A55・usable memory 2.76 GiB・感覚器は加速度計のみ・GPS 無しという非対称な organ candidate の実測値、firmware のビルド不良により swap が存在しない事実とその二重の原因（fstab 断片の未合体、`init.rc` からの `swapon_all` 欠落）、usable memory 枯渇時に `kswapd0` が CPU を奪って load 21.46 まで崩壊した実測ログを記録する。llama.cpp 公式 Android prebuilt による throughput 実測 (Qwen3-0.6B-Q8_0: `pp512` 50.58 t/s / `tg128` 4.52 t/s) では、**pp は thread 数に比例して伸びる一方 tg は全くスケールせず `t=8` でむしろ低下する**という memory-bandwidth bound を確認し、同一モデルでの M2 Max 比が pp 11.8× / tg 24.3× と負荷種別で 2 倍以上変わることを記録する。消費電力は未測定であり、**本機に cognitive role を与える根拠は含まない**。素体の記録であって登録提案ではない。
- [`experiments/multi-resource-j72-grokbot.md`](./experiments/multi-resource-j72-grokbot.md) — **二資源ルーティング実証**。J72 (150M, custom PyTorch) と Qwen2.5-3B (llama.cpp) を同時に Cognitive Resource として登録し、privacy routing matrix、routing authority をモデルへ渡さない構造、failure isolation（privacy 境界を越えた fallback をしないこと）、provenance の同一扱いを記録する。J72 が第三者 VM へ移設され両方 `external` になったため、locality でデータ境界が行き先を分ける実証は router policy の自動テストとしてのみ成立し、実機では成立していない点を明記する。latency 実測と比較評価は endpoint 認証待ちで未完。
- [`experiments/grokbot-external-cognitive-resource.md`](./experiments/grokbot-external-cognitive-resource.md) — **Grok Bot VM の 4B モデルを external cognitive resource として借りる実験記録**。Tailscale 越しの llama.cpp (Qwen2.5-3B-Instruct Q4_K_M, ctx 4096) を Persona Core ではなく交換可能な認知資源として登録し、`locality = external` とする根拠、4K capacity と privacy の境界、endpoint failure 時の canonical state 不変性、secret を保存しない設定、実機実測、既知の limitation を記述する。実装済みの内容のみ。

### 器官設計監査 / Organ design audit

- [`audits/2026-09-08-organ-design-audit.md`](./audits/2026-09-08-organ-design-audit.md) — 日本語。固定 commit を基準に26機能領域を監査し、18件の横断的な設計不足・接続未確定点、既存設計、優先順位、確認限界を整理。実装済みという判定ではない。
- [`organ-contracts-and-implementation-plan.md`](./organ-contracts-and-implementation-plan.md) — 日本語。状態所有者、event/信念/正典変更/目標/行為/音声/停止復旧/忘却学習の契約案、28件の未実施テスト仕様、段階的な有効化 gate。規範仕様へ未昇格の提案。
- [`research/2026-09-08-organ-audit-literature.md`](./research/2026-09-08-organ-audit-literature.md) — 日本語。関連文献21本を本文該当節・一次要旨・書誌のみの確認深度で区別し、適用先と一般化できない点を記録。

### Core research

- [`research/README.md`](./research/README.md) — research-map index and promotion flow from daily survey to durable research/design knowledge.
- [`research/landscape.md`](./research/landscape.md) — cross-disciplinary landscape mapping prior art to Kamimusuhi subsystems, differences, research gaps, core research questions, and prioritized experiments.
- [`daily-surveys/README.md`](./daily-surveys/README.md) — dated cross-domain research intake and index; new papers/projects enter here before promotion into durable research or architecture notes.
- [`research-foundations.md`](./research-foundations.md) — ALife, cognitive architectures, neuroscience/psychology, memory, extended cognition, persistent identity, tools, sleep, interoception.
- [`runtime-authority-learning-boundaries.md`](./runtime-authority-learning-boundaries.md) — **2026-09-09 daily survey から昇格した横断設計ノート**。K-Nerve/K-Edge の高速 interaction、person-bound / minimal-sufficient memory、tool competence、deterministic authority boundary、SkillAdam-like procedural evolution、NeoHorse-like candidate-descendant training を一つの contract map に統合し、7つの検証 benchmark と normative promotion gate を定義する。

### Native cognition / model training

- [`persona-core-training-taxonomy.md`](./persona-core-training-taxonomy.md) — what a Kamimusuhi Persona Core should learn: self/other, human/social cognition, epistemology, delegation, persona priors, long-horizon relationship competence, evaluation.
- [`model-ecology-and-lineage.md`](./model-ecology-and-lineage.md) — model ecology across two scales: a medium `novllm`-derived Persona Core plus small specialist-model organs inside each individual, and a multi-user population of distinct individual lineages with privacy-gated population-level inheritance.
- [`semantic-token-language-cortex.md`](./semantic-token-language-cortex.md) — **Phase57 の日本語特化 J72 を起点に、JP/EN の表層 tokenizer を共有せず、versioned shared semantic codebook / continuous residual を介して language cortex を接続する研究設計を蒸留**。LCM/SONAR、DCMA、Q-BridgeNet、CILI/UNL を先行研究として整理し、既存 SmolLM2/Pythia/ModernBERT 等を English cortex に再利用して semantic bridge だけを先に学習する方針と K-SemBridge v0 の評価 gate を定義する。
- [`fio-cross-review-and-runtime-countermeasures-2026-09-10.md`](./fio-cross-review-and-runtime-countermeasures-2026-09-10.md) — **FIO RV1/RV2 cross-review を Kamimusuhi の設計要件へ蒸留**。独立実装間の収束点、運用上の不一致、per-turn UUID、応答前 metadata gate、system-owned agent loop、tool/result/observation の分離、full-scratch Persona Core による assistant prior 低減、理由を必須化しない自律性、failure injection / soak test を整理する。
- [`narrative-continuity-and-metaphor-anchors-2026-09-10.md`](./narrative-continuity-and-metaphor-anchors-2026-09-10.md) — **長期個体の「累積としての連続性」と比喩アンカーを外部事例から蒸留**。モデル交換を越える continuity、bridge / sand mandala / fired pottery のような narrative anchor、物語自体の lineage、第三者 continuity test、自己物語が自己証拠化する再帰ループ、typed provenance と scratch Persona Core migration test を整理する。
- [`optional-p2p-population-learning.md`](./optional-p2p-population-learning.md) — **optional / non-normative** long-horizon note for privacy-preserving P2P population learning: signed artifacts and evaluation receipts, E2EE plus secure aggregation/privacy controls, Sybil resistance, heterogeneous distributed evaluation, federated/volunteer compute, and species-level inheritance without merging individual identity or private lived experience.

### Reflex / control substrate

- [`multiscale-brain-architecture.md`](./multiscale-brain-architecture.md) — provisional multi-timescale nervous-system architecture: PNL → K-Fast → Global Workspace/K-Edge → Persona Core, with independent timing domains, event-based crossings, fly-inspired sparse recurrent regulation, affect-like modulation, and a 1k–10k-node K-Fast prototype path.
- [`peripheral-neural-layer.md`](./peripheral-neural-layer.md) — provisional architecture decision for the Peripheral Neural Layer (PNL): learned discrete controllers belong primarily in the peripheral nervous/reflex system, below semantic/LLM cognition, with deterministic safety gates, upward reflex receipts, body-specific adaptation, and backend-independent deployment.
- [`learned-discrete-reflex-layer.md`](./learned-discrete-reflex-layer.md) — learned low-latency nervous-system layer beneath K-Edge: DLGN/LDLGN, DWN/LUT controllers, stochastic-computing inspiration, temporal state, teacher distillation, shadow deployment, auditability, and benchmark plan.
- [`fly-brain-connectome-reference.md`](./fly-brain-connectome-reference.md) — FlyWire/Shiu/Eon reference analysis: validated connectome-derived LIF sensorimotor results, multi-backend `fly-brain` implementation, 2026 embodied-loop caveats, mechanistic criticism, K-Nerve mapping, ablation tests, backend-parity methodology, and the K-Fast/multi-timescale extension.

### Sensory and motor embodiment

- [`sensory-nervous-system.md`](./sensory-nervous-system.md) — vision, hearing, touch, environment, chemical senses, proprioception, interoception, sensory-event bus, reflexes, multisensory binding, neuromorphic path.
- [`attention-sensing-and-joint-attention.md`](./attention-sensing-and-joint-attention.md) — **non-deep-first attention sensing design**: gaze as uncertain sensory evidence rather than mind reading, observation→hypothesis→belief separation, gaze/head/cursor/UI/touch/speech fusion, user/agent/shared attention, deictic grounding, joint attention, local raw-frame reduction, graceful fallback without eye tracking, and evaluation gates.
- [`speech-and-vocal-expression.md`](./speech-and-vocal-expression.md) — speech as motor/social output: utterance planning, prosody, full-duplex dialogue, backchannels, interruption, vocal identity, self-monitoring, streaming synthesis, distributed vocal embodiment.

### Performance

- [`llm-runtime-compiler-foundations.md`](./llm-runtime-compiler-foundations.md) — **2026-09-10 の LLM 実装・高速化記事群を一次ソースまで追って蒸留した durable note**。tokenizer、autoregressive/KV、prefill と decode の別プログラム化、static KV + CUDA Graph、quantization fusion、FlashAttention-2、canonical speculative decoding の exactness、same-lineage draft 仮説、DDTree/DSpark、VRAM offload、PTX と MLIR の境界、ResourceProfile 拡張、RTX 3060/P100 等での再現実験 gate を統合する。
- [`heterogeneous-cognitive-compute-substrate.md`](./heterogeneous-cognitive-compute-substrate.md) — non-normative design note for heterogeneous cognition: Boolean/LUT routing, mixed precision, attention as a scarce capability, hardware-aware native-model search, and role-specialized deployment across modern/legacy GPUs, CPU, and FPGA; includes RTX 3060/P100 limit-GPU-rack implications and benchmark hypotheses.
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