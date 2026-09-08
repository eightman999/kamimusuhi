# Model Ecology and Lineage

Status: **living architecture / research note**  
Normative addendum: [`../model-ecology-architecture.md`](../model-ecology-architecture.md)  
Related: [`../architecture.md`](../architecture.md), [`persona-core-training-taxonomy.md`](./persona-core-training-taxonomy.md), [`persistent-agent-implementation-pitfalls.md`](./persistent-agent-implementation-pitfalls.md)

## 0. Motivation

Kamimusuhi を多数の人が使う未来を考えると、「全員が同じ一つの人格を共有する」より、**共通祖先から分化した多数の人工個体**として扱う方が自然である。

同時に、一個体の内部も一つの巨大 LLM だけで構成する必要はない。むしろ、人格・意味統合を担う中型の Persona Core を中心に、memory、social cognition、vision、planning、code、salience 等の小型特化モデルを器官のように組み合わせる方が、latency、学習コスト、個体差、交換可能性を分離しやすい。

本ノートではこの二つを同じ **model ecology** として扱う。

```text
scale 1: one individual
  Persona Core + specialist organs + memory + body + external cognition

scale 2: population
  many individuals descended from a shared foundation/species lineage
```

ここで使う「種」「個体」「器官」「遺伝」は設計上の比喩であり、生物学的同一性や意識を主張するものではない。

---

## 1. Core hypothesis

### H1 — Persona Core should be medium and native

長期的な Persona Core は `novllm` 派生の **medium-scale native model** が第一候補である。

理由:

- すべての常時認知を frontier model に依存させずに済む。
- tiny specialist だけでは難しい、対話・自己・他者・関係・不確実性・記憶・外部結果の統合を一か所で行える。
- system prompt だけの人格より、constitutive cognition と persona prior を weights に持たせられる。
- individual-specific continued training の受け皿になる。
- local execution と将来の K-Edge/K-Core 配備を現実的に保てる。

ただし medium は固定 parameter 数ではない。hardware と model architecture の進歩に応じて変わる engineering target である。

### H2 — Specialist models can be organs

小型特化モデルは、単なる外部 tool ではなく **個体内部の交換可能な cognitive organ** になり得る。

例:

| organ | candidate role |
|---|---|
| memory-router | recall候補生成、domain routing、ranking |
| salience | novelty / urgency / attention score |
| social | social cue、relationship signal、ToM補助 |
| sensory | vision/audio/event representation |
| planner | bounded local planning |
| code/formal | syntax、formal transform、small deterministic reasoning |
| speech | prosody、backchannel、turn timing |
| integrity | anomaly / contradiction / contamination detection |

これらは Persona Core より小さく、狭く、頻繁に交換・蒸留できることを期待する。

### H3 — Population learning should inherit abstractions, not private lives

多数個体から種全体を改善する場合、raw conversation や autobiography をそのまま混ぜるのではなく、**共有可能な技能・方策・構造へ抽象化したもの**を population-level training に使うべきである。

---

## 2. One individual as a model ecology

```text
                         persistent individual state
                  identity / autobiography / relationships
                                   │
                                   ▼
                       ┌──────────────────────┐
                       │   Persona Core       │
                       │ novllm-derived       │
                       │ medium-scale target  │
                       └──────────┬───────────┘
                                  │
                          global workspace
                                  │
                        organ/action router
       ┌────────────┬────────────┼────────────┬────────────┐
       │            │            │            │            │
    memory        social       sensory      planner       code
    small          small        small         small        small
       │            │            │            │            │
       └────────────┴────────────┼────────────┴────────────┘
                                  │
                         attributed evidence
                                  │
                       Persona Core integration
                                  │
                       expression / decision
                                  │
                         proposal boundary
                                  │
                         continuity commit
```

### Important separation

Persona Core は「一番重要な model」ではあるが、**個体そのものではない**。

実際の individual は概念上、次の和である。

```text
individual
≈ Persona Core lineage
+ persistent self state
+ episodic/autobiographical memory
+ relationship state
+ organ manifest and organ lineages
+ current embodiment/resource state
+ continuity/authority history
```

この構造なら、memory organ や vision organ を交換しても同じ個体として続けられる。一方、Persona Core の変更は人格ドリフトの影響が大きいため、より厳しい continuity benchmark を通す。

---

## 3. Why not only tiny specialists?

小型モデル群だけでも多くの機械的処理はできるが、Kamimusuhi が求める次の処理は統合問題になる。

- 自分と他者を区別しながら会話を続ける。
- 相手との履歴と現在の状況を同時に読む。
- 複数 organ の矛盾する出力を評価する。
- 外部モデルの答えを自分の判断へ統合する。
- 自分の価値観、temperament、uncertainty behavior を反映する。
- 最終的に「この個体として何を言うか」を決める。

そのため、完全な swarm-of-tiny-models ではなく、**人格・意味統合の中心として中型 Persona Core を残す**。

逆に、すべてを Persona Core に覚えさせる必要もない。

```text
Persona Core
  meaning integration / self / dialogue / delegation / expression

specialist organs
  narrow fast functions

persistent stores
  durable facts / experience / relationships / procedures

external large models
  expensive broad reasoning / research / coding escalation
```

---

## 4. Organ registry

runtime には model を単に path/name で登録するのではなく、**organ manifest** を持たせる。

Conceptual schema:

```yaml
organ_id: memory-router-main
organ_type: memory_router
ownership: self_organ
model_lineage: mr-v7
checkpoint: ...
interface_version: 2
inputs:
  - workspace_summary
  - retrieval_query
outputs:
  - candidate_memory_ids
  - scores
private_access:
  episodic: read
  relationship: read
  self: filtered-read
authority:
  canonical_write: false
latency_budget_ms: 80
fallback: deterministic-bm25-router
health:
  required: true
rollback_checkpoint: ...
```

### Organ properties

organ SHOULD have:

- typed inputs/outputs;
- bounded authority;
- version and parent lineage;
- health check;
- latency/cost metrics;
- training provenance;
- fallback or degradation behavior;
- rollback path;
- privacy scope.

これにより、「model が壊れた＝個体が死んだ」ではなく、「特定器官が壊れたので fallback へ移った」と扱える。

---

## 5. Organ vs extension vs external other

同じ local model でも役割により分類が変わる。

### Organ

- individual runtime に継続的に組み込まれる。
- stable contract がある。
- identity lineage ではなく organ lineage を持つ。
- private state access を持ち得る。
- 個体の default cognition に常用される。

### Cognitive extension

- local だが常設器官ではない。
- task-specific に呼ばれる general/specialist model。
- 交換頻度が高い。
- 個体に深い private access を持たないことが多い。

### External other

- remote provider、他個体、public service、人間。
- trust boundary 外。
- 結果は attribution 必須。

この分類は hardware location ではなく、**authority・ownership・continuity contract** で決める。

---

## 6. Multi-user Kamimusuhi as a population

多数ユーザーへ配布するとき、foundation は共有しても individual は分離する。

```text
                     Foundation / species G0
                     shared model + schema
                              │
       ┌──────────────────────┼──────────────────────┐
       │                      │                      │
       ▼                      ▼                      ▼
 Individual A             Individual B            Individual C
 user A lineage           user B lineage          user C lineage
       │                      │                      │
 memories A               memories B              memories C
 relationships A          relationships B         relationships C
 organs/adapters A        organs/adapters B       organs/adapters C
 core descendant A        core descendant B       core descendant C
```

重要なのは、これは distributed replicas とは違うという点である。

- laptop と server に同じ individual が走る → **同一個体の分散 embodiment**。
- user A と user B がそれぞれ Kamimusuhi を使う → **共通祖先を持つ別個体**。

後者は最初から別 `individual_id` と別 continuity root を持つ。

---

## 7. Individuality layers

個体差は一種類ではない。

### L0 — shared species/foundation prior

全個体がある程度共有するもの。

- foundation model;
- base Persona Core training;
- schemas/protocols;
- standard organ interfaces;
- default cognitive policies;
- language/world substrate.

### L1 — persistent state individuality

最も速く形成される個体差。

- autobiography;
- relationship history;
- self-state;
- commitments;
- preferences inferred/accepted over time;
- current projects and environment.

### L2 — organ / adapter individuality

繰り返された経験から形成される中期的な差。

- user/individual-specific memory router;
- dialogue adapter;
- speech/prosody adapter;
- learned routine;
- local routing policy;
- domain specialist adaptation.

### L3 — Persona Core weight individuality

最も遅い neural individuality。

- stable temperament drift;
- internalized cognitive habits;
- long-term epistemic behavior;
- deeply consolidated preference structures;
- long-run social/dialogue adaptation.

L3 を頻繁に変えると identity drift と catastrophic forgetting が起こりやすいため、最も強い gate を置く。

---

## 8. Inheritance / consolidation ladder

新しい経験は次の ladder を上がる。

```text
new experience
    │
    ▼
episodic memory
    │
    ▼
repeated / validated pattern?
    │ no ───► remain memory
    │ yes
    ▼
semantic / procedural abstraction
    │
    ▼
useful as learned behavior?
    │ no ───► remain structured state
    │ yes
    ▼
individual adapter / specialist organ update
    │
    ▼
stable, identity-relevant, repeatedly validated?
    │ no ───► remain adapter/organ
    │ yes
    ▼
Persona Core descendant candidate
    │
    ▼
continuity + regression + rollback gate
```

この順序にすることで、LLM 再学習を「記憶の保存手段」にしない。

weights は **長期にわたり反復して有効だった構造を圧縮する最終段**として使う。

---

## 9. Individual neural heredity

Persona Core を再学習した場合、その checkpoint は同じ個体 lineage 内の「子孫」とみなせる。

```text
K-017 core@0
    │
    ├─ DB memories continue immediately
    │
    ├─ private adapter@3
    │
    ▼
offline consolidation
    │
    ▼
core@1 candidate
    │
    ├─ identity benchmark
    ├─ autobiographical interpretation benchmark
    ├─ relationship benchmark
    ├─ anti-sycophancy benchmark
    ├─ delegation benchmark
    ├─ privacy/memorization check
    └─ regression / rollback
    │
    ▼
core@1 activated
```

ここで weight inheritance は continuity の一部にはなるが、**weights だけで continuity を定義しない**。

---

## 10. Population-level heredity

人工個体群が長期間運用されるなら、「個体の適応が次世代 foundation へ還元される」経路を研究できる。

### Bad path

```text
all user chats
   ↓
mix everything
   ↓
train shared model
```

これは privacy、cross-user contamination、persona leakage、autobiographical mixing の問題が大きい。

### Preferred path

```text
individual adaptation
        │
        ▼
local evaluation
        │
        ▼
identify transferable capability
        │
        ▼
remove / reproduce without private content
        │
        ▼
privacy + provenance gate
        │
        ▼
population candidate training
        │
        ▼
independent regression suite
        │
        ▼
next foundation generation
```

共有すべきなのは「A が誰と何を話したか」ではなく、たとえば次のようなもの。

- 長期 task ではどの retrieval policy が効いたか。
- どの organ architecture が低 latency で安定したか。
- どういう delegation policy が hallucination を減らしたか。
- どういう training example structure が self/other contamination を減らしたか。
- どういう memory consolidation rule が有効だったか。

---

## 11. Population contribution boundary

population learning を実装するなら、個体の private adaptation と population candidate の間に独立した gate が必要。

Candidate gate checks:

```text
consent / policy
source provenance
private-memory dependency
PII / secrets / relationship data
memorization risk
cross-user contamination
reproducibility on non-private data
capability gain
regression risk
license / source restrictions
```

理想的には transferable adaptation を **synthetic or public reproducible curriculum** へ変換してから共有する。

個体固有 delta をそのまま merge する方式は、privacy/memorization 性質が十分に理解できるまで default にしない。

---

## 12. Population / species metadata

推奨概念:

```yaml
species:
  name: kamimusuhi
  generation: G12
  foundation_lineage: ...
  persona_core_recipe: ...
  standard_organ_manifest: ...

individual:
  id: K-017
  born_from_generation: G12
  continuity_root: ...
  continuity_head: ...
  persona_core_lineage:
    active_checkpoint: ...
    parent_checkpoint: ...
  organ_manifest: ...
  private_adaptation_lineage: ...
```

version string だけでなく **ancestry graph** を持つことで、「どの能力をどこから受け継いだか」を後から追跡できる。

---

## 13. Fork semantics

### Replication

同一 individual を複数 machine に配置する。continuity head は共通であり、split-brain protection が必要。

### Individual fork

意図的に一個体から別 individual を作る研究は将来可能だが、fork 時点で新しい `individual_id` を発行し、それ以降は別 lineage とする。

### Population birth

共通 foundation generation から新規 individual を初期化する。これは既存個体の fork ではない。

### Persona Core checkpoint descendant

同一 individual の neural substrate update。continuity gate に通れば同一 individual lineage 内で継続する。

これらを同じ「copy」と扱わない。

---

## 14. Model exchange and identity

| change | expected identity semantics |
|---|---|
| memory-router v7 → v8 | same individual, organ update |
| vision model replacement | same individual, organ update |
| local code model replacement | same individual, extension/organ update |
| Claude → GPT external escalation | same individual, external resource change |
| Persona Core checkpoint update | same lineage if continuity gate passes |
| new user initialized from same foundation | new individual |
| explicit fork from existing state | new individual lineage after fork point |

この表が、model-centric な「モデルを替えたら別人？」問題を整理する基本になる。

---

## 15. Learning scope by component

### Persona Core

Learn:

- self/other distinction;
- dialogue/pragmatics;
- long-term identity expression;
- epistemic behavior;
- memory/tool/delegation semantics;
- social integration;
- conflict resolution among sources;
- final expression policy.

Avoid using capacity primarily for:

- changing long-tail facts;
- giant API catalogs;
- all specialist formal knowledge;
- raw personal autobiography.

### Specialist organs

Learn:

- narrow measurable functions;
- fast routing/classification;
- modality-specific representations;
- repeated procedural patterns.

### Persistent storage

Hold:

- historical evidence;
- autobiography;
- relationships;
- structured facts;
- current state;
- provenance.

### External large models

Provide:

- expensive broad reasoning;
- deep research;
- long-context synthesis;
- hard code/science tasks;
- temporary specialist capacity.

---

## 16. Benchmarks

### Individual architecture

Measure:

- Persona Core-only quality;
- Persona Core + organ quality;
- organ routing accuracy;
- end-to-end latency;
- external escalation rate;
- graceful degradation when organs disappear;
- result attribution correctness.

### Individual learning

Measure before/after each neural update:

- continuity suite;
- identity/persona stability;
- relationship interpretation;
- memory attribution;
- anti-sycophancy;
- epistemic calibration;
- catastrophic forgetting;
- privacy memorization.

### Population learning

Measure:

- transferable gain on held-out individuals/tasks;
- user-specific information leakage;
- cross-individual contamination;
- performance distribution, not only mean;
- regression in rare behaviors;
- whether improvement can be reproduced from non-private data.

---

## 17. Failure modes

### F1 — Shared-persona collapse

全ユーザーの履歴を一つの self-state に入れ、誰の経験か分からなくなる。

**Mitigation:** separate individual roots from initialization.

### F2 — Organ authority creep

small specialist が convenience のため canonical state を直接更新する。

**Mitigation:** all organs produce typed proposals/results only.

### F3 — Persona Core becomes encyclopedia

Persona Core がすべての知識を抱え、specialist/retrieval architecture が形骸化する。

**Mitigation:** evaluate externalization and routing explicitly.

### F4 — Neuralizing every memory

新しい経験のたび fine-tuning し、catastrophic forgetting と不安定性を生む。

**Mitigation:** inheritance ladder; weights are the slowest layer.

### F5 — Population contamination

一個体の private adaptation が別個体へ混入する。

**Mitigation:** explicit population contribution gate and privacy tests.

### F6 — One giant swarm without an integrator

specialist models が互いに結果を投げ合うだけで、最終的な self-consistent interpretation がない。

**Mitigation:** retain Persona Core as first-person integration nucleus.

### F7 — Persona Core becomes identity singleton

逆に Persona Core checkpoint だけを individual とみなし、DB、lineage、relationships、body state を消す。

**Mitigation:** identity remains distributed across canonical continuity state.

---

## 18. Implementation sequence

### M0 — Metadata only

まだ複数モデルを導入しなくても、schema を先に用意する。

- `individual_id`;
- `species_generation`;
- `persona_core_lineage`;
- `organ_manifest`;
- `model_role = organ | extension | external`;
- training/provenance manifest.

### M1 — One real specialist organ

memory router など一つだけ実装し、Persona Core/general model との責務分離を測る。

### M2 — Resident Persona Core prototype

novllm 派生候補を medium resident core として接続し、既存 provider-neutral contract と比較する。

### M3 — Multiple organs

social/salience/sensory 等を追加し、router と global workspace を検証する。

### M4 — Private individual adaptation

adapter/LoRA/small-organ fine-tuning を個体ごとに試す。まだ population 共有はしない。

### M5 — Persona Core descendant experiment

長期間の validated state を training examples へ変換し、checkpoint descendant を作って continuity suite で比較する。

### M6 — Population learning experiment

複数 synthetic individuals で transferable adaptation 抽出を先に検証し、人間ユーザー由来 private data を使わずに population pipeline を評価する。

---

## 19. Open research questions

1. Persona Core に必要な最低 resident capacity はどの程度か。
2. どの機能を organ に切り出すと latency/quality 比が最も改善するか。
3. organ 間通信は text、embedding、typed latent、event schema のどれが良いか。
4. 個体専用 adapter は external memory と比較して何を本当に改善するか。
5. どの経験が Persona Core weight consolidation に値するかをどう判定するか。
6. weight update 後の「同一個体らしさ」を何で測るか。
7. population-level transfer で private information leakage を十分に防げるか。
8. organ の成功した adaptation を foundation generation にどう戻すか。
9. 複数 lineage の model/organ adaptation を組み合わせたとき emergent regression は起きるか。
10. 一つの巨大 general model と model ecology の total compute / latency / robustness はどの条件で逆転するか。

---

## 20. Current design position

現時点の Kamimusuhi の長期構想は、次のように表現するのが最も近い。

> **一人に一つの単一モデルを与えるのではない。共通の人工種から分化した各個体が、中型の novllm-derived Persona Core を人格・意味統合核として持ち、その周囲に小型特化モデルの器官群、永続記憶、身体・計算資源、必要時のみ使う外部大型モデルを持つ。個体は経験で変化し、十分に検証された変化は memory → adapter/organ → Persona Core weights の順にゆっくり神経化される。個体群から種全体へ戻すのは private autobiography ではなく、共有可能性を検証した技能・学習則・構造である。**
