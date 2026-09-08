# Kamimusuhi Model Ecology Architecture Addendum

Status: **Draft / pre-alpha design addendum**  
Applies to: [`architecture.md`](./architecture.md), [`spec.md`](./spec.md)  
Detailed research/design note: [`docs/model-ecology-and-lineage.md`](./docs/model-ecology-and-lineage.md)

本書は、Kamimusuhi を「一個の LLM」としてではなく、**中型の Persona Core、複数の小型特化モデル、外部認知資源、永続状態から構成される一個体**として扱い、さらに多数の利用者へ展開したときに**共通祖先から分化する個体群**として扱うための設計追補である。

既存の continuity invariant を変更しない。特に、model weight は identity の唯一の所有者ではなく、parameter count は cognitive tier を定義しない。

---

## 1. Target architecture

長期的な標準形として、次の三層を想定する。

```text
                         Kamimusuhi individual
                                  │
                    ┌─────────────┴─────────────┐
                    │                           │
          Persistent / Canonical Self     Cognitive body
                    │                           │
                    └─────────────┬─────────────┘
                                  │
                         Persona Core Model
                 medium-scale, novllm-derived target
                  identity integration / expression
                                  │
                         Global Workspace
                                  │
                  Cognitive Action / Organ Router
          ┌───────────┬───────────┼───────────┬───────────┐
          │           │           │           │           │
       memory      social      vision      planner      code ...
       organ       organ       organ       organ        organ
      tiny/small  tiny/small  tiny/small  small        small
          │           │           │           │           │
          └───────────┴───────────┼───────────┴───────────┘
                                  │
                         attributed results
                                  │
                         Persona Core integrates
                                  │
                    ┌─────────────┴─────────────┐
                    │                           │
              local heavy model          frontier / remote LLM
              optional extension          external cognition
```

### 1.1 Persona Core

Persona Core は長期的には **`novllm` 系譜から派生した中型モデル**を第一候補とする。

ここで「中型」は normative parameter-count class ではなく、次の工学的役割を示す相対概念である。

- 常駐または高頻度に実行可能である。
- tiny/small specialist より広い統合能力を持つ。
- frontier-class general model を毎ターン必要としない。
- self/other、人格、対話、記憶利用、認知資源の選択、最終表現を一貫して統合できる。

Persona Core SHOULD は百科事典として最大化するのではなく、**個体としての意味統合と自己一貫性**に容量を優先する。

### 1.2 Specialist models as organs

小型特化モデルは単なる tool としてだけでなく、条件を満たす場合は **cognitive organ** として個体内部に所属してよい。

例:

```text
memory-retrieval organ
salience / novelty organ
social-cue organ
vision / audio organ
routing organ
planning organ
code / formal-reasoning organ
speech/prosody organ
integrity / anomaly organ
```

organ は個別に学習・蒸留・交換・rollback できる。organ の交換だけで identity が変わったとはみなさない。

### 1.3 External large models

Claude、GPT、DeepSeek 等の外部大型モデルは、原則として **Kamimusuhi 本人を impersonate する人格本体ではなく、外部認知資源**である。

```text
Persona Core decides to delegate
          ↓
structured request
          ↓
external model
          ↓
attributed result
          ↓
Persona Core evaluates / integrates
          ↓
Kamimusuhi expression / action
```

provider が変わっても individual continuity は維持される。

---

## 2. Organ / extension boundary

既存 architecture の `SELF / ORGAN`, `COGNITIVE EXTENSION`, `EXTERNAL OTHER` を、model ecology に対して次のように解釈する。

### SELF / ORGAN

次を満たす specialist model は organ として登録してよい。

- authenticated individual runtime に所属する。
- stable typed interface を持つ。
- provenance と version/lineage を追跡できる。
- Persona Core / Continuity Kernel の authority 境界を破らない。
- replacement / rollback が可能である。
- private state access が policy により明示的に許可されている。

### COGNITIVE EXTENSION

必要時に利用するが個体の常設器官とはみなさない local/general model、search、code executor、retrieval service 等。

### EXTERNAL OTHER

frontier provider、public service、他の Kamimusuhi individual、human 等。出力は attribution を失ってはならない。

organ であっても **canonical identity owner ではない**。

---

## 3. Individuality across users

多数の利用者が Kamimusuhi を使う場合、全員が一つの人格状態を共有するのではなく、**共通の種・基盤系譜から別々の individual lineage が開始する**。

```text
species / foundation generation G0
              │
      ┌───────┼────────┐
      │       │        │
  individual A     individual B     individual C
      │       │        │
 experience  experience  experience
 adapters    adapters    adapters
 organs      organs      organs
 memory      memory      memory
```

各 individual は少なくとも次を独立して持つ。

```text
individual_id
continuity_head
persona_core_lineage
persistent_self_root
autobiographical_memory_root
relationship_state_root
organ_manifest
private_adaptation_lineage
```

同じ foundation checkpoint から開始したことは、同じ identity であることを意味しない。

---

## 4. Three inheritance timescales

個体の変化は一つの mechanism に押し込めず、少なくとも三つの時間スケールへ分離する。

### 4.1 Fast: memory / durable state

```text
experience -> episodic capture -> validated durable state
```

即時性が必要な個体差はまず DB / structured state に残す。新しい経験のたびに weight を書き換えない。

### 4.2 Medium: organ / adapter adaptation

反復して有効だった技能・癖・routing policy は、個体専用 adapter、LoRA、small-model checkpoint、procedural policy 等へ蒸留してよい。

これは後天的な個体差に近い層であり、rollback と provenance を必須とする。

### 4.3 Slow: Persona Core descendant

十分に検証された経験・自己変化は、offline consolidation を通して Persona Core の descendant checkpoint へ反映してよい。

```text
Persona Core K-014
      │
validated experiences / schemas
      │
training-set generation + provenance
      │
continued training / distillation
      │
continuity + regression evaluation
      │
K-014 descendant checkpoint
```

この更新は database mutation より遥かに保守的でなければならない。

---

## 5. Population-level inheritance

個体群全体が存在する場合、個体から種レベルへ改善を還元する研究経路を持ってよい。

ただし原則は次である。

> **個人の記憶を共有するのではなく、共有可能であることを検証した適応を共有する。**

共有候補:

- task-general skill;
- routing / delegation policy;
- memory retrieval strategy;
- specialist organ improvement;
- epistemic calibration behavior;
- compression / consolidation method;
- safety / integrity policy improvement;
- synthetic curriculum derived without private content.

default で共有してはならないもの:

- raw conversation;
- autobiographical episode;
- relationship history;
- user-specific preference/fact;
- private Library source;
- model delta whose memorization/privacy properties are未検証。

概念フロー:

```text
individual A/B/C adaptations
          │
          ▼
privacy + provenance + quality gate
          │
          ▼
abstract / distill / reproduce on non-private data
          │
          ▼
population candidate
          │
          ▼
regression / contamination / privacy evaluation
          │
          ▼
species generation G(n+1)
          │
          ▼
future individuals inherit the improvement
```

これは「全員の会話を混ぜて再学習する」設計とは明確に区別する。

---

## 6. Lineage semantics

model ecology では少なくとも二つの lineage を分離する。

### Species / foundation lineage

共通の基盤モデル、training recipe、schema、標準 organ package の世代履歴。

### Individual lineage

一個体の continuity head、Persona Core descendant、private adapters、organ variants、persistent self-state の履歴。

推奨 metadata:

```yaml
species_generation: G12
foundation_checkpoint: novllm-...
individual_id: K-...
continuity_head: ...
persona_core:
  model: ...
  parent_checkpoint: ...
  training_manifest: ...
organs:
  memory_router: ...
  social: ...
  vision: ...
private_adaptation:
  lineage: ...
  provenance_manifest: ...
```

multi-device replication は individual reproduction ではない。accidental fork を防ぐ既存 Continuity Kernel の規則を維持する。

新しい individual を作る場合は、同じ seed/foundation から開始しても **新しい `individual_id` と独立 continuity root** を発行する。

---

## 7. Evolution path

### Phase A — organ registry

- typed specialist-model interface;
- organ/extension/external classification;
- version, provenance, health, latency metrics;
- deterministic replacement and rollback.

### Phase B — novllm-derived Persona Core

- medium-scale resident target;
- self/other and dialogue integration;
- specialist routing;
- external-result integration;
- continuity regression suite.

### Phase C — individual neural adaptation

- private adapter / organ fine-tuning;
- offline training-set generation;
- weight-update approval gate;
- catastrophic-forgetting and identity-drift evaluation.

### Phase D — population learning

- opt-in contribution boundary;
- privacy/memorization tests;
- abstracted skill extraction;
- population candidate training;
- generation-level rollback and A/B evaluation.

### Phase E — artificial population research

将来研究として、複数系統の organ / policy / Persona Core adaptation を選択・再結合し、次世代 foundation を作る evolutionary workflow を検討してよい。ただし、これは biological reproduction の再現ではなく、**人工認知系に適した lineage-based optimization** として扱う。

---

## 8. Architectural invariants for this addendum

1. **One individual is not one model.**
2. **The Persona Core is central but not the sole owner of identity.**
3. **Small specialist models may become internal organs without becoming identity authorities.**
4. **External frontier models remain attributed cognitive resources.**
5. **Users receive distinct individual lineages, not shared autobiographical identity.**
6. **Private memory does not become population training data by default.**
7. **Inheritance is staged: memory first, adapters/organs second, Persona Core weights last.**
8. **Population-level improvements require privacy, provenance, quality, and contamination gates.**
9. **Model size is an engineering target, not an identity invariant.**
10. **A lineage graph must make it possible to state what was inherited, from where, and under what authority.**
