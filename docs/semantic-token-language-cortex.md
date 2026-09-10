# Semantic Token / Language Cortex Architecture

Status: **research/design note — non-normative**  
Date: **2026-09-10**  
Related: [`model-ecology-and-lineage.md`](./model-ecology-and-lineage.md), [`multiscale-brain-architecture.md`](./multiscale-brain-architecture.md), [`../model-ecology-architecture.md`](../model-ecology-architecture.md), [`../architecture.md`](../architecture.md)

## 0. Decision summary

Kamimusuhi SHOULD NOT assume that one tokenizer and one language model must efficiently cover Japanese, English, code, perception, and internal cognition.

The working research hypothesis is instead:

1. preserve strong **surface-language specialists** rather than forcing them into one shared surface vocabulary;
2. map their internal representations into a **shared semantic bridge**;
3. allow a bounded central Core / Global Workspace to operate on language-independent representations;
4. keep language-specific nuance in a residual representation rather than forcing every distinction into one universal discrete ID;
5. reuse strong existing English specialist models before training an English model from scratch;
6. treat code as a separate specialist path, preferably augmented by parser-derived AST / symbol / data-flow structure rather than text tokens alone.

The important distinction is:

```text
DO NOT REQUIRE:
  JP surface token id == EN surface token id

RESEARCH INSTEAD:
  JP surface tokens -> JP language cortex -> shared semantic code
  EN surface tokens -> EN language cortex -> shared semantic code
```

A shared semantic code may be discrete, continuous, or hybrid. The current preferred experiment is **discrete semantic ID + continuous residual**.

---

## 1. Empirical trigger: novllm Phase57

The immediate motivation is the Phase57 English/general tokenizer audit of the J-series.

Measured on the Phase57 held-out corpora:

| domain | J72 chars/token | Qwen3 chars/token | observation |
|---|---:|---:|---|
| Japanese general | 2.4005 | 1.3356 | J72 is highly efficient for this Japanese corpus |
| English general | 1.9312 | 4.8838 | J72 uses about 2.53x as many tokens as Qwen3 |
| English technical | 1.8549 | 4.9237 | strong English fragmentation |
| Python | 1.8925 | 4.6373 | code fragmentation resembles the English weakness |
| JP/EN mixed | 2.1425 | 2.1174 | mixed text is roughly comparable |

Round-trip encoding/decoding succeeded and `<unk>` was zero, so the failure is not inability to represent English bytes. It is primarily **inefficient segmentation plus weak English modeling**.

The J72 LM also showed the same specialization pattern:

```text
Japanese hardened BPB : 4.2442
English general BPB    : 7.8308
English technical BPB  : 7.7947
Python BPB             : 7.7693
```

The J64 -> J72 vocabulary delta is also informative: only **84 / 8000** added pieces were classified as pure Latin, while most additions were Japanese/mixed-script material.

Interpretation:

> J72 is better described as a promising Japanese-specialized language model/tokenizer lineage than as a failed universal tokenizer.

This suggests preserving a `J72-JP` lineage and testing a modular multilingual architecture before redesigning J72 into a single universal vocabulary.

### Measurement caveat

The Phase57 minimal-pair score used raw sequence NLL comparisons and is sensitive to unequal continuation lengths/token counts. Treat that score as diagnostic only until it is replaced by conditional continuation scoring and/or byte-normalized NLL.

---

## 2. Proposed architecture

```text
                         persistent self / memory / drives
                                      |
                                      v
                         Core / Global Workspace
                         language-independent state
                                      ^
                                      |
                         Shared Semantic Bridge
                    discrete IDs + continuous residual
                    /            |              \
                   /             |               \
          JP language cortex  EN language cortex  Code cortex
               J72-JP          existing model     parser + model
                   ^             ^               ^
                   |             |               |
             JP tokenizer   EN tokenizer      source / AST / DFG
                   ^             ^
                   |             |
               Japanese       English
```

The Core SHOULD NOT need to know the surface tokenizer IDs used by each language cortex.

A language cortex owns surface representation; the semantic bridge owns cross-language alignment.

This leaves open several implementation choices:

- a shared vector-quantized codebook;
- continuous SONAR-like embeddings;
- residual vector quantization;
- a hybrid of discrete semantic IDs and continuous residuals;
- explicit symbolic anchors for only a small subset of concepts.

---

## 3. Semantic token contract

Do not define a semantic token as merely a renamed BPE token.

A useful conceptual message is:

```yaml
semantic_codebook: ksem-v1
semantic_ids: [1842, 7331, 294]
residual: <small continuous vector or quantized residual>
source_cortex: jp-v1
confidence: 0.93
provenance: observation-...
time_scope: current_episode
```

The discrete ID answers roughly **which learned semantic prototype(s)** are active. The residual can preserve context, nuance, uncertainty, morphology, viewpoint, or language-specific information that should not be collapsed into the shared codebook.

### Important invariants

1. Semantic IDs are **codebook-version scoped**. `1842` has no stable meaning outside `ksem-v1` unless an explicit migration exists.
2. Do not use semantic IDs as permanent autobiographical/canonical identifiers by themselves.
3. The bridge is a cognitive representation layer, not an identity authority.
4. Surface token IDs need not match between languages.
5. A concept code need not correspond one-to-one with an English dictionary word.
6. Polysemy must be resolved by context; `bank(finance)` and `bank(river)` should not be forced into one shared code merely because the spelling is identical.
7. Human-readable labels such as `CAT` are debugging annotations, not necessarily the actual learned representation.

---

## 4. Prior art and what it contributes

### 4.1 Large Concept Models / SONAR

Meta's **Large Concept Models** explicitly separate token-level language from a higher-level, language- and modality-agnostic concept space. The LCM paper uses SONAR sentence embeddings and also explores models operating in a quantized SONAR space.

- Paper: https://arxiv.org/abs/2412.08821
- Japanese implementation-oriented reading: https://zenn.dev/if001/articles/1144c3ccee9cc7
- SONAR hands-on note: https://zenn.dev/mozuku55/articles/d4a494f790a447

**Kamimusuhi lesson:** language-independent central cognition is plausible, but LCM's sentence-scale continuous concept is not the only possible granularity. Kamimusuhi should experimentally compare continuous concept vectors against shorter learned discrete semantic sequences.

### 4.2 DCMA: shared discrete virtual tokens

**Discrete Cross-Modal Alignment Enables Zero-Shot Speech Translation** (EMNLP 2022) uses vector quantization to map speech and text into the same finite shared vocabulary of virtual tokens, explicitly training corresponding speech/text representations to select the same shared codebook entries.

- https://aclanthology.org/2022.emnlp-main.354/

**Kamimusuhi lesson:** the mechanism needed for `different surface encoders -> same learned discrete codebook` already has strong precedent. The novel question is not whether VQ alignment is possible, but whether it is useful as a persistent cognitive workspace representation across language organs.

### 4.3 Q-BridgeNet: shared base + language-specific residual

**Q-BridgeNet** (2026) uses residual vector quantization for cross-lingual sign-language translation: a shared base codebook represents language-agnostic semantic primitives while language-specific residual codebooks preserve heterogeneous language-specific semantics.

- https://arxiv.org/abs/2607.11215

**Kamimusuhi lesson:** this is a particularly strong precedent for **shared semantic ID + language-specific residual**. Avoid forcing every nuance into a universal codebook.

### 4.4 CILI / WordNet-style interlingual concept identifiers

The Collaborative Interlingual Index maintains a single interlingual index of concepts across wordnets.

- https://aclanthology.org/2016.gwc-1.9/
- https://github.com/globalwordnet/cili

**Kamimusuhi lesson:** explicit concept IDs can be useful as bootstrap/debug anchors, evaluation labels, or grounding resources. They should not automatically become the entire learned internal language because real contextual meaning is more fine-grained than dictionary synsets.

### 4.5 Universal Networking Language

UNL is an older symbolic interlingua that represents sentence meaning as language-independent concepts/relations.

**Kamimusuhi lesson:** the idea of an artificial machine-facing interlingua is not new. The research opportunity is a **learned, compact, replaceable neural interlingua integrated with persistent cognition**, not the existence of an interlingua itself.

---

## 5. Existing English specialists: do not pretrain E-EN from zero first

There are already useful compact English-focused models. The first experiment should reuse them and train only a semantic bridge/adapter where possible.

| candidate | role | size / training | license | Kamimusuhi use |
|---|---|---|---|---|
| `HuggingFaceTB/SmolLM2-135M` | causal LM | 135M; 2T tokens; primarily English; includes The Stack | Apache-2.0 | **first generative EN-cortex candidate** |
| `EleutherAI/pythia-70m` | causal LM | ~70M class; English-only; Pile; full training checkpoints | Apache-2.0 | smallest clean research/control candidate |
| `facebook/MobileLLM-125M` | causal LM | 124.6M; English; 1T tokens; on-device optimized | fair-noncommercial-research | performance/on-device benchmark, license prevents default product dependency |
| `answerdotai/ModernBERT-base` | bidirectional encoder | 149M; 2T English+code tokens; 8k context | Apache-2.0 | **strong EN -> semantic-code encoder candidate**, not a text generator |
| `roneneldan/TinyStories-33M` | causal LM | 33M; synthetic simple English stories | MIT | tiny bridge proof-of-concept only; too narrow for a real English cortex |

Primary sources:

- SmolLM2-135M: https://huggingface.co/HuggingFaceTB/SmolLM2-135M
- Pythia-70M: https://huggingface.co/EleutherAI/pythia-70m
- MobileLLM-125M: https://huggingface.co/facebook/MobileLLM-125M
- ModernBERT-base: https://huggingface.co/answerdotai/ModernBERT-base
- TinyStories-33M: https://huggingface.co/roneneldan/TinyStories-33M

### Current recommendation

Do **not** spend the next training budget on an English-from-scratch sibling of J72.

Start with:

```text
JP side: J72-JP checkpoint/tokenizer, frozen initially
EN side: SmolLM2-135M base, frozen initially
control: Pythia-70M
optional EN encoder baseline: ModernBERT-base
bridge: newly trained projection + VQ/RVQ codebook
core: tiny semantic-sequence model or controlled passthrough in the first experiment
```

The first scientific question is whether two independently pretrained language specialists can be aligned into a useful compact common representation without erasing their native strengths.

If that works, only then consider training a native `E-EN` model for architectural symmetry, licensing, latency, or lineage-control reasons.

---

## 6. Code is not merely a third natural language

Phase57 shows that J72 fragments code similarly to English. That does not imply the fix is only a better code tokenizer.

For code, use deterministic structure when available:

```text
source
  -> parser
  -> AST / symbol table / type information / CFG / data-flow
  -> code encoder
  -> semantic bridge
  -> Core
```

A code LM remains useful for naming, intent, incomplete programs, natural-language comments, repair hypotheses, and probabilistic semantics. But parser-derived facts SHOULD NOT be relearned unreliably if a compiler/front-end can provide them exactly.

Code therefore fits the model ecology as a specialized cognitive organ whose interface may contain both symbolic structure and learned latent state.

---

## 7. Minimal experiment: K-SemBridge v0

### Goal

Test whether independently pretrained JP and EN specialists can communicate through a learned shared representation without natural-language translation at the Core boundary.

### Frozen components

- J72-JP
- SmolLM2-135M
- optionally ModernBERT-base as an encoder baseline

### Trainable components

- JP projection head
- EN projection head
- shared VQ or RVQ codebook
- reconstruction/contrastive heads
- optional small residual quantizer

### Candidate codebook sizes

```text
4K / 8K / 16K
```

Do not assume that a larger codebook is better. Measure semantic collision, utilization, entropy, and reconstruction cost.

### Training signals

Use a mixture of:

- JP/EN parallel or semantically equivalent sentences;
- contrastive positives/negatives;
- reconstruction within each language;
- cross-language reconstruction;
- codebook commitment/diversity objectives;
- optional concept/synset labels only as weak anchors.

### Required baselines

1. continuous shared latent without quantization;
2. shared discrete codebook without residual;
3. shared discrete base + language-specific residual;
4. ordinary JP -> text translation -> EN path;
5. no shared bridge / separate models.

### Evaluation

Measure at least:

- cross-lingual retrieval accuracy;
- semantic equivalence clustering;
- homonym/polysemy separation;
- round-trip semantic preservation;
- JP reconstruction quality;
- EN reconstruction quality;
- codebook utilization / dead codes;
- bits or IDs per source byte/semantic unit;
- latency and resident memory;
- whether Core task accuracy is preserved when surface language changes;
- whether language-specific style/nuance survives the shared bottleneck.

The bridge is successful only if it improves modularity without turning into an information-destroying bottleneck.

---

## 8. Promotion gates into normative architecture

This document does not yet replace the existing Persona Core design.

Promote a language-cortex / semantic-bridge architecture into `architecture.md` / `spec.md` only after demonstrating:

1. JP and EN specialists can share a representation with measurable semantic alignment;
2. the shared representation beats or matches a continuous-only baseline for at least one relevant Kamimusuhi task;
3. language switching does not require canonical identity/persona state migration;
4. a specialist can be replaced/rolled back without changing semantic-codebook ownership or canonical continuity;
5. codebook version migration is explicit and testable;
6. loss of one language cortex degrades expression/understanding in that language without destroying the persistent individual;
7. semantic bridge provenance and confidence can be represented in the Global Workspace.

---

## 9. Architectural interpretation

If the experiment succeeds, the model ecology changes from:

```text
one Persona Core that directly owns all natural-language competence
```

toward:

```text
persistent individual
  + language-independent cognitive Core / Workspace
  + language cortices with independent model/tokenizer lineages
  + a versioned semantic bridge
  + structural/code/sensory organs
  + external cognitive extensions
```

This is compatible with the existing continuity invariant:

> **one individual is not one model.**

Language competence becomes another replaceable cognitive organ/cortex rather than the sole substrate of identity.
