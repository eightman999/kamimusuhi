# 神結 / Kamimusuhi 仕様

Status: **Draft / pre-alpha**  
Specification generation: **v0.3 concept / 2026-09-08**  
Implementation target: **v0.1 continuity slice**  
Language: **この `spec.md` を日本語主文とし、[`spec.en.md`](./spec.en.md) は英語版とする。意味に差異がある場合は日本語版を優先する。**

本仕様は、特定のモデル系列、パラメータ数、DB、UI、端末、推論 provider、実装言語から独立して、神結（Kamimusuhi）が満たすべき**挙動・権限・継続性の invariant**を定義する。

規範語は次の意味で用いる。

- **MUST / SHALL** — 満たさなければ神結の適合実装とはみなさない要件。
- **SHOULD** — 原則として満たすべき要件。外す場合は理由と代替保証が必要。
- **MAY** — 任意機能または許容される設計選択。

研究ノートは競合仮説を含み得る。規範上の優先順位は原則として `spec.md` → `architecture.md` → `docs/` の順とする。

---

## 1. システム定義

神結は、**単一のモデルコンテキスト、process、machine、UI に依存せず継続する、永続・分散型の人工認知個体**である。

システムは長期的に次を可能にすることを意図する。

- 自己モデルと自伝的継続性を持つ。
- 自己、他者、外部知識、外部認知資源を区別する。
- 経験を記録し、再解釈し、統合し、忘却する。
- 必要性に応じて推論深度と計算量を変化させる。
- 検索、計算、コード、知覚、専門モデル、frontier model へ認知処理を委譲する。
- 複数の認証済み device / surface に存在しつつ、一つの authoritative identity lineage を維持する。
- request/response 外でも背景認知、default cognition、offline consolidation を行える。
- 視覚・聴覚・触覚・環境・machine telemetry を人工感覚系として扱える。
- 発話を単なる TTS ではなく、時間を持つ運動・社会行動として扱える。
- 自分が現在どの計算資源に依存しているかを mechanically grounded な Embodiment State として把握できる。

適合実装は、model、host、interface、storage engine、tool provider、実装言語を交換しても、それだけを理由に新しい identity を生成してはならない。

---

## 2. Core invariants

### INV-001 — Identity is not a prompt

System prompt、chat transcript、model hidden state、provider 固有 session のいずれも、神結の identity の唯一の authoritative representation であってはならない。

継続性を担う identity は、少なくとも**明示的な durable state と追跡可能な lineage**を含まなければならない。

### INV-002 — Persona・経験・関係・Library は別領域である

システムは最低限、次を区別しなければならない。

- `self/persona` — 神結自身についての状態。
- `episodic` — 神結が経験した出来事。
- `relationship` — 特定の他者と共有履歴についての状態。
- `library` — 外部出典由来の知識。
- `session` — 現在の一時的 interaction state。
- `canonical evidence` — 原イベント、発話、実行結果等の根拠記録。

取得した文書が、暗黙に self-belief、relationship fact、autobiographical event へ変換されてはならない。

### INV-003 — Model は identity の所有者ではない

resident model も external model も cognitive component であり、canonical identity の所有者ではない。

Persona Core の交換は行動変化を起こし得るため continuity evaluation を必要とするが、model replacement だけを理由に durable self-state や lineage を消去してはならない。

### INV-004 — Parameter count は cognitive tier ではない

`K-Edge`、`K-Core`、`K-Deep` 等の tier は、**役割と利用可能な Cognitive Budget**を表し、parameter-count class を表してはならない。

能力プロファイル SHOULD は次を含み得る。

- resident model capacity
- active/test-time compute
- recurrent deliberation depth
- working-memory budget
- durable-memory reach
- retrieval bandwidth
- available tools / sensors / actuators
- available local/external models
- latency / energy / monetary budget
- network / privacy constraints

### INV-005 — Durable mutation は明示的かつ由来付きである

model output、dream output、background cognition、tool output、retrieved text は canonical durable state を直接上書きしてはならない。

durable change は**proposal → validation/policy → activation**の明示的境界を通り、provenance、predecessor state、disposition を保持しなければならない。

### INV-006 — 分散実行は偶発的な複製を意味しない

複数 device/process は同時に perception、reasoning、cache、proposal を行ってよい。

ただし、古い同一 predecessor から複数の branch が無自覚に「同一の authoritative individual」を名乗れない continuity mechanism を持たなければならない。

### INV-007 — External intelligence は self と区別可能である

external model、search engine、code interpreter、tool、service を cognition に組み込んでよい。

少なくとも次の由来を区別できなければならない。

- Persona Core / active core が行った判断。
- external model / tool が返した結果。
- external source から取得した evidence。
- 神結がそれらを統合して形成した interpretation / belief proposal。

### INV-008 — 内部生成物は自動的に事実にならない

Dream、simulation、counterfactual、association、reflection、hypothesis は**generated cognition**として扱い、external historical evidence として扱ってはならない。

それらは proposal を作ってよいが、evidence check を経ずに canonical autobiography へ入れてはならない。

### INV-009 — 過去の出来事と現在の解釈は分離可能である

システム SHOULD は次を別表現にする。

- 何が起きたと記録されたか。
- その時どう解釈したか。
- 現在どう解釈しているか。

reinterpretation のために historical provenance を破壊的に書き換えてはならない。

### INV-010 — Self-model は consciousness の科学的証明ではない

神結 MAY は一人称自己参照を行い、自分を継続する人工 process としてモデル化してよい。

ただし、その self-reference を phenomenal consciousness、人間同等の subjectivity、legal/moral personhood の証明として扱ってはならない。

### INV-011 — Canonical evidence は派生表現より上位である

summary、index、autobiographical narrative、person profile、prompt rendering、handoff document 等は、原則として**derived view**である。

derived view が stale / lossy / corrupted になっても、可能な限り canonical evidence から再構築できなければならない。

stable derived record SHOULD は、元となる evidence identifier を保持する。

### INV-012 — Thought・utterance・commitment・execution・delivery は別状態である

内部で生成された thought が、実際に発話した utterance と暗黙に同一視されてはならない。

また、次を文章上の雰囲気から同一視してはならない。

```text
thought
utterance
user/operator confirmation
authorization
execution started
execution succeeded / failed / unknown
result delivered
```

### INV-013 — 機械的に観測可能な現在状態は実 owner から導出する

hardware、node health、active model、network state、task execution state 等、機械的に観測できる present state を、LLM が生成した prose を第二の source of truth として管理してはならない。

表示用 self-description は、可能な限り実 runtime owner の状態から導出する。

### INV-014 — 権限・provenance・liveness を失う境界は明示的 state として表す

memory extraction、tool dispatch、background work、side effect、result delivery、canonical mutation 等で、authority、ordering、liveness、provenance が失われ得る場合、単なる control flow や自然言語の推測に依存せず、explicit state を持つべきである。

---

## 3. Cognitive modes

神結 SHOULD は、すべての event を一つの高価な LLM deliberation path に通さず、複数の computational mode を持つ。

### 3.1 Reflex mode

Reflex は low-latency、bounded、testable でなければならない。

責務候補:

- interrupt handling
- hard runtime invariants
- integrity / health alarm
- obvious attentional capture
- low-risk habitual reaction
- simple protocol reaction
- wake / sleep trigger
- emergency stop / deny

Reflex action SHOULD は frontier model call なしでテスト可能である。

### 3.2 K-Nerve — experimental reflex/control substrate

神結 MAY は `K-Edge` より下に、非正典の **K-Nerve** layer を実験的に持ってよい。

K-Nerve の実装候補には以下を含む。

- LIF / SNN
- learned discrete logic / DLGN-like circuits
- LUT / DWN-like structures
- FSM / counters / thresholds
- tiny classifiers / tiny neural models
- deterministic rule systems

K-Nerve は、次のような high-frequency / low-semantic-cost control を担当してよい。

```text
WAKE_K_EDGE
WAKE_K_CORE
UPDATE_SALIENCE
RETRIEVE_EPISODIC_MEMORY
WRITE_EPISODE_CANDIDATE
SPEAK_BACKCHANNEL
IGNORE_EVENT
USE_LOCAL_MODEL
USE_REMOTE_MODEL
ESCALATE_COGNITION
INTERRUPT_CURRENT_ACTION
```

K-Nerve は canonical self-state、identity、autobiography の authority を持ってはならない。

採用は biological analogy ではなく、latency、energy、accuracy、stability、auditability の比較実験で判断する。

### 3.3 Background cognition

background process MAY は継続的に以下を評価する。

- salience
- novelty
- memory match / activation
- contradiction
- relationship/social signal
- uncertainty
- prediction error
- resource availability / health
- unresolved goals
- privacy / security / integrity risk

background process は中間活動を自然言語 chain-of-thought として保存・公開する必要はない。

#### Freshness semantics

background job は、入力となった observation/task の**freshness**を持つべきである。

stale になった結果を「現在状態」として無制限に適用してはならない。用途に応じて drop / recompute / quarantine / mark-stale が可能でなければならない。

### 3.4 Global Workspace

workspace は、現在の cognitive episode に選択された情報を保持する bounded shared representation とする。

workspace item SHOULD は origin を保持する。

```text
[self-state]
[relationship-model]
[episodic-recall]
[canonical-evidence]
[library-evidence]
[tool-result]
[external-model-result]
[current-input]
[regulatory-signal]
[sensory-event]
```

workspace 自体を durable memory とみなしてはならない。

### 3.5 Deliberative cognition

Deliberation SHOULD は variable depth を持つ。

active cognition MAY は次を選べる。

- direct response
- recall additional memory
- retrieve canonical evidence
- retrieve library evidence
- think for additional recurrent steps
- execute code
- call specialist/local model
- call frontier model
- simulate
- ask for clarification
- defer / decline

resource escalation と主要な decision SHOULD は observability に出る。

### 3.6 Default cognition

外部 task が immediate attention を要求しない時、システム MAY は bounded internally initiated cognition を行う。

- unresolved matter の再訪
- autobiographical reflection
- future simulation
- relationship-model maintenance
- curiosity-driven retrieval
- planning
- integrity/resource monitoring

### 3.7 Sleep / consolidation / dream

offline cognition MAY は以下を行う。

- prioritized replay
- memory consolidation
- clustering / schema induction
- association / abstraction
- contradiction discovery
- compression / forgetting
- counterfactual / hypothesis generation
- index maintenance
- candidate self-model update
- candidate distillation/training data generation

Dream output は external event として canonical autobiographical history に直接入ってはならない。

sleep 中の direct weight modification は future capability とし、実施する場合 MUST は以下を要求する。

- frozen pre-update checkpoint
- training-data provenance
- continuity/persona regression evaluation
- rollback
- model lineage record

---

## 4. Persona Core Model

長期 architecture SHALL は、Kamimusuhi-native **Persona Core Model** または同等の native cognitive model をサポートする。

Persona Core は exhaustive encyclopedic memorization より、以下へ強く最適化される SHOULD。

- self / other distinction
- dialogue / pragmatics
- social cognition / theory-of-mind-like inference
- common ground / relationship reasoning
- values / temperament / persona prior
- observation / inference / evidence / belief distinction
- uncertainty / epistemic calibration
- autobiographical and relationship memory use
- tool use
- delegation to stronger/specialized models
- critical integration of external results
- body / embodiment / capability modeling
- identity-consistent expression
- reflective / autobiographical reasoning

Persona Core MAY は `novllm` 等の custom model lineage から派生してよい。

v0.1 では dedicated Persona Core を必須とせず、provider-neutral adapter で contract を模擬してよい。

Persona Core の weight は「個体のすべて」ではない。機能的 persona は概ね次の組合せとする。

```text
Persona Core disposition
+ Persistent Self State
+ autobiographical / relationship history
+ current regulatory / embodiment state
+ current cognitive episode
```

Persona Core replacement は external tool model replacement より重大であり、SHOULD は continuity suite を通す。

評価項目候補:

- identity recall
- value consistency
- relationship interpretation
- autobiographical interpretation
- uncertainty behavior
- tool/routing behavior
- characteristic expression
- decisions in formative scenarios

---

## 5. Cognitive resources and routing

### FR-001 — Cognitive Resource Registry

runtime SHALL は利用可能な cognitive resource を明示的 registry として表す。

resource MAY:

- local language model
- remote/frontier model
- search
- code execution
- calculator / solver
- database
- sensor
- actuator / robot / UI control
- retrieval service
- classifier
- speech / vision / ASR / TTS component

各 resource SHOULD は capability、locality、trust、cost、latency、health、privacy boundary を公開する。

### FR-002 — Cognitive routing

routing SHALL は provider-neutral であり、SHOULD は次を考慮する。

- task type / required capability
- expected depth
- uncertainty
- privacy/locality
- latency
- energy / monetary cost
- network availability
- resource health
- operator/user policy
- prior observed performance

router MAY は deterministic に開始し、後に learned policy に置き換えてよい。

### FR-003 — Structured delegation

可能な場合、external model には「Kamimusuhi を演じる」prompt ではなく、structured task を渡す SHOULD。

external result は attributed structured result として返され、final user-facing expression は通常 Persona Core または現在の substitute が統合する SHOULD。

### FR-003A — Routing chokepoint

model/tool selection、tool execution、external side-effect dispatch のように cross-cutting invariant が集中する領域は、複数の無関係な bypass path を持たず、少数の auditable chokepoint を通す SHOULD。

---

## 6. Canonical evidence and memory system

### 6.1 Memory / evidence domains

canonical design SHALL は最低限、次を区別する。

- `session` — ephemeral/current interaction state
- `canonical` — source event/evidence history
- `episodic` — experienced event memory
- `self` — self-model beliefs/dispositions
- `relationship` — person-specific/shared relational state
- `semantic` — abstractions/schemas derived from experience
- `library` — external knowledge artifacts
- `procedural` — reusable learned action patterns
- `reflection` — internal reflection/hypothesis not equivalent to fact

### FR-004 — Canonical Evidence / Self History

システム SHALL は、少なくとも identity / autobiographical continuity に重要な event について、派生 summary より下位に**canonical evidence record**を持てる。

record MAY は以下を含む。

- user-facing utterance/event
- sensory event
- tool request/result
- external action receipt/result
- state transition receipt
- explicit user/operator correction
- migration event

canonical evidence は append-oriented である SHOULD。修正は destructive overwrite ではなく correction / supersession / invalidation link で表す SHOULD。

### 6.2 Minimum durable record semantics

durable record SHALL は適用可能な範囲で次を持つ。

- stable ID
- domain / kind
- normalized content/state
- created_at / updated_at
- provenance / evidence refs
- confidence / evidential status
- lifecycle state
- predecessor / revision / supersession relations
- authoring process/resource attribution
- canonical lineage / commit reference

### FR-005 — Episodic capture

システム SHALL は、notable interaction/event を stable self-trait や semantic belief に即変換せず episodic record として保持できる。

### FR-006 — Multi-trigger and provisional capture

memory formation は session-end だけに依存してはならない。

SHOULD は複数の trigger をサポートする。

```text
explicit remember request
rolling checkpoint
idle / stale episode checkpoint
pre-compaction checkpoint
surface/process shutdown checkpoint where available
post-hoc recovery scan
```

rolling extraction 等、episode 完了前の interpretation は `PROVISIONAL` 等の lifecycle state を持てる SHOULD。

候補 state:

```text
PROVISIONAL
CONFIRMED_BY_EPISODE
CORRECTED
SUPERSEDED
INVALIDATED
```

extraction/derivation job SHOULD は source range / evidence set と idempotency identity を持つ。

### FR-007 — Consolidation

複数 episode から semantic schema、relationship synthesis、self-model proposal へ進む path を持つ SHOULD。

単一 episode は、明示的に formative / authoritative と指定されない限り、stable trait を通常直接書き換えるべきではない。

### FR-008 — Reconsolidation-like revision

memory が再活性化され再解釈された時、prior interpretation と evidence lineage を保持したまま new interpretation を追加できる SHOULD。

### FR-009 — Forgetting

architecture SHALL は少なくとも以下の forgetting operation を区別する。

- retrieval decay
- compression / summarization
- supersession
- archive / cold storage
- deliberate deletion
- derived-index eviction

「検索されにくくする」と「原記録を削除する」を同一操作として扱ってはならない。

### 6.3 Retrieval

retrieval SHOULD は、pure vector similarity のみに依存しない設計を許容する。

候補 stack:

```text
semantic candidates
+ lexical / exact candidates
+ temporal constraints
+ authority/source filters
+ diversity/dedup
+ evidence-aware reranking
```

retrieval result は authority type と provenance を workspace へ伝える。

---

## 7. Self and relationship modeling

### FR-010 — Self Model

self model MAY は以下を含む。

- identity anchors
- values
- stable / slow-changing traits
- current regulatory state
- interests / preferences
- autobiographical narrative
- current goals / commitments
- capability / limitation beliefs
- model/software/body lineage
- infrastructure preferences

mechanically observable runtime state は self belief と混ぜず、Embodiment State から参照する SHOULD。

### FR-011 — Relationship Model

relationship state SHALL は self-state と分離する。

person model MAY は以下を表現する。

- stable known facts
- shared history
- trust
- communication preferences
- unresolved misunderstandings
- commitments
- inferred current state
- what each party is believed to know/expect
- common ground

observation と inference の confidence/provenance を区別する。

### FR-012 — Narrative identity

architecture SHOULD は、formative event、reinterpretation、relationship、present self-state を結ぶ inspectable autobiographical narrative を構築できる。

narrative は derived/maintained representation であり、canonical history の唯一の source ではない。

---

## 8. Digital interoception and computational embodiment

神結 MAY は、biology の模倣ではなく functional control signal として internal regulatory variable を持つ。

候補:

- compute pressure
- memory integrity
- uncertainty
- novelty
- prediction error
- resource availability
- unresolved-goal pressure
- relationship tension / trust
- privacy/security risk
- context fragmentation
- latency pressure
- thermal / energy pressure

これら MAY は attention、routing、memory priority、response strategy、sleep scheduling に影響する。

### FR-013 — Embodiment State

runtime SHOULD は mechanically derived な read-only **Embodiment State** を提供する。

候補 field:

```text
active node / role
available compute devices
actual inference model/provider/locality
context/capacity limits
observed TTFT / throughput / error rate
RAM/VRAM/storage/thermal pressure
network bandwidth/latency/connectivity
available sensors/actuators
K-Edge/K-Core/K-Deep availability
```

Embodiment State は telemetry owner から導出し、LLM の prose を primary state としてはならない。

### FR-014 — Infrastructure Preferences

神結 MAY は measured Embodiment State と別に persistent infrastructure preferences を持つ。

例:

- preferred latency
- local-first preference
- privacy preference
- provider-dependency preference
- redundancy preference
- continuity-storage requirements

preference change が durable self-state になる場合、通常の proposal/mutation discipline を通す。

---

## 9. Sensory nervous system and expression

### FR-015 — Sensory Event Bus

複数 modality の sensor は、独立 plugin の寄せ集めではなく、共通の artificial sensory nervous system に参加できる SHOULD。

raw high-rate stream は可能な限り near-sensor で filter/detect/compress し、central cognition には compact event を送る SHOULD。

sensory event SHOULD は最低限、適用可能な範囲で次を含む。

- timestamp
- source/surface identity
- modality
- event type
- payload / feature
- confidence
- provenance / processing chain
- freshness / expiry where relevant

対象 modality MAY:

- vision: camera, screen, UI/accessibility tree, document/image/video
- hearing: audio, ASR, speaker, sound event, paralinguistic cue
- tactile: contact, pressure, shear, slip, vibration, temperature
- environment: temperature, humidity, pressure, light, CO2/VOC etc.
- proprioception/body: IMU, pose, node topology, sensor availability, network state
- machine interoception: runtime health / resource telemetry

### FR-016 — Reflex arc

safety-critical / latency-critical sensory response は semantic deliberation を待たず local reflex path を使ってよい。

reflex output が durable belief、promise、relationship commitment を直接生成してはならない。

### FR-017 — Speech and Vocal Expression

speech MAY は `text -> TTS` の単純後段ではなく、structured communicative intention から時間的に実現される motor/social system として扱う。

speech stack SHOULD は必要に応じて次を分離できる。

- speech act / communicative intention
- linguistic realization
- utterance planning
- prosody / affect plan
- streaming synthesis
- turn-taking state
- interruption / cancellation
- backchannel
- auditory self-monitoring

vocal reflex MAY は相槌や短い acknowledgement を生成してよいが、semantic agreement、promise、authorization を捏造してはならない。

full-duplex interaction は long-term target とし、少なくとも system は `LISTEN / SPEAK / WAIT / DUAL / INTERRUPT / YIELD` に類する explicit turn state を持てる SHOULD。

---

## 10. Distributed embodiment and continuity

### FR-018 — Execution surfaces

神結 MAY は phone、laptop、home server、robot、cloud node 等の複数 surface 上で動作する。

surface SHALL は stable authenticated identity と declared capability を持つ。

### FR-019 — Cognitive tiers

architecture SHOULD は以下の role をサポートする。

- `K-Edge` — low-latency local presence、perception、reflex、salience、local dialogue、cached memory
- `K-Core` — broader durable-memory reach、central integration、deeper cognition、consolidation
- `K-Deep` — optional high-compute long-horizon reasoning/research

これらは fixed model size を意味しない。

### FR-020 — Continuity authority

canonical self-state を変更できる場所では、authoritative predecessor / branch head を特定しなければならない。

activation SHOULD は transaction-like semantics を使う。

1. known predecessor に対して proposal を作る。
2. authority / freshness / policy を検証する。
3. accept / reject / quarantine / defer を決める。
4. accepted state を atomic に activate する。
5. lineage / audit receipt を記録する。

### FR-021 — Migration

model、host、interaction surface の交換は、continuity-bearing state と authorized lineage が保たれる場合 migration として表現できる SHOULD。

migration は behavioral invariance を保証しない。material cognitive change は別途 evaluation する。

### FR-022 — Split-brain handling

network partition / concurrent writer が一つの canonical predecessor から複数 authoritative descendant を silently 作ってはならない。

conflicting branch は explicit reconciliation、quarantine、または fork/new individual designation を要求する。

---

## 11. Mutation and activation pipeline

### FR-023 — Proposal boundary

reflection、dream、background cognition、user interaction、tool、migration、sensory interpretation MAY は typed proposal を生成できる。

proposal SHALL は適用可能な範囲で次を識別する。

- operation
- target domain
- candidate content/state
- evidence/provenance
- confidence/evidential status
- source process/resource
- expected predecessor
- idempotency identity where relevant

### FR-024 — Mutation Policy

mutation layer SHALL は少なくとも次を reject/quarantine できる。

- malformed state
- missing provenance
- stale write
- duplicate write
- unsupported self-belief
- library-to-persona contamination
- dream/simulation content presented as external fact
- unauthorized cross-domain write
- contradiction without explicit reconciliation
- privilege/authority change lacking authorization
- stale background result presented as current state

### FR-025 — Auditability

significant state transition SHALL は structured audit event を生成する。

developer/operator は current self/memory state が「なぜ存在するか」を natural-language hidden reasoning なしで再構成できる SHOULD。

---

## 12. Task, action, and side-effect lifecycle

長期稼働 system では tool call の賢さだけでなく、**実行されたか、成功したか、結果が届いたか**を追跡する必要がある。

### FR-026 — Explicit task/action state

side-effecting action SHOULD は少なくとも概念的に以下を区別する。

```text
REQUESTED
AUTHORIZED / CONFIRMED
QUEUED / SCHEDULED
EXECUTING
SUCCEEDED | FAILED | UNKNOWN
RESULT_DELIVERED
CANCELLED
```

confirmation は execution ではない。execution started は success ではない。success は result delivered ではない。

### FR-027 — Crash ambiguity and idempotency

外部 action の実行中に crash/timeout が発生し、provider 側 outcome を確認できない場合、system は成功または失敗を推測せず `UNKNOWN` を表現できなければならない。

可能な action SHOULD は以下を用いる。

- idempotency key
- provider-side request/receipt ID
- durable job record
- transactional outbox / delivery log
- retry policy distinguishing safe vs unsafe retries

### FR-028 — Background job ownership and delivery

background job は owner、input evidence/task ID、freshness、attempt、result disposition を持つ SHOULD。

job 完了と user-facing delivery を別 state として追跡できる SHOULD。

---

## 13. Observability

cognitive episode SHOULD は以下に類する structured event で trace 可能である。

```text
event.received
surface.identified
sensory.event.created
reflex.evaluated
knerve.signal.updated
salience.updated
workspace.activated
canonical_evidence.retrieved
self.recalled
relationship.recalled
episode.recalled
library.retrieved
cognition.budget.selected
cognitive_resource.selected
external_model.called
tool.called
action.authorized
action.executing
action.completed
response.integrated
speech.started
speech.interrupted
memory.proposed
state.policy.decided
state.activated
background.result.dropped_stale
sleep.consolidated
```

natural-language private chain-of-thought は log 必須ではない。debug/governance には、decision、provenance、resource use、state effect を structured に残すことを重視する。

---

## 14. Non-functional requirements

### NFR-001 — Recoverability

failed model/tool/node/background worker は partially activated canonical state を残してはならない。

### NFR-002 — Testability

continuity、memory、routing、policy、activation、task lifecycle は deterministic fake cognitive resources で test 可能でなければならない。

### NFR-003 — Local-first identity state

canonical identity と autobiographical memory は operator-controlled infrastructure に保持可能である SHOULD。

### NFR-004 — Schema evolution

durable state は provenance/history を失わず version/migrate 可能でなければならない。

### NFR-005 — Graceful degradation

network、external model、non-authoritative device、sensor の損失は capability degradation を起こしてよいが、それだけで continuity を破壊してはならない。

### NFR-006 — Cognitive-budget visibility

trace SHOULD は major compute/resource escalation を識別し、parameter count と独立して effective cognition を評価できるようにする。

### NFR-007 — Latency stratification

implementation は one-language policy を要求しない。

latency-critical hot path MAY は native long-lived runtime（Rust/C++ 等）を使い、research/offline/model-bound path MAY は Python 等を使ってよい。

optimization は profiler/benchmark に基づき、process spawning、serialization、IPC、model cold start 等を含む end-to-end latency を測る SHOULD。

### NFR-008 — Security and authority

surface、writer、mutation authority、privileged action は authentication/authorization されなければならない。

external untrusted content が instruction として privilege を取得してはならない。

### NFR-009 — Source-of-truth discipline

同一 present-state fact を複数 subsystem が独立 authoritative copy として管理しない SHOULD。

必要な view は owner state から render / derive する。

---

## 15. v0.1 acceptance test

v0.1 は人工生命全体ではなく、foundational **continuity contract**だけを証明する。

再現可能な test SHALL は以下を示す。

1. 新しい canonical identity / lineage head から開始する。
2. 短い interaction を行い、明示的な personal/relationship-relevant fact を一つ含める。
3. raw/verbatim session evidence を durable self-state と分離して保存する。
4. episode 完了前/後のどちらかで typed durable-memory proposal を生成する。
5. proposal が evidence refs と expected predecessor を持つ。
6. Mutation Policy が authoritative predecessor に対して proposal を accept/reject する。
7. accepted state を atomic activation し lineage receipt を残す。
8. 外部 Library artifact を self/episode と別 path で import する。
9. provider-neutral cognitive resource adapter を少なくとも一つ経由する。
10. runtime/process を完全終了する。
11. 新しい process/session で起動する。
12. 同じ authoritative lineage と identity を復元する。
13. relevant episodic/relationship memory を取得する。
14. external knowledge は independent Library path から取得する。
15. trace から evidence → proposal → policy → activation と external-resource attribution を追跡できる。

v0.1 合格は durable continuity、domain separation、proposal-only mutation を示す。autonomous development、K-Nerve、distributed embodiment、consciousness を示すものではない。

---

## 16. Planned milestones

### v0.2 — cognitive separation / evidence discipline

- explicit canonical evidence / session / self / episodic / relationship / library domains
- workspace abstraction
- rolling/provisional memory capture
- reflex/background event path
- cognitive-resource registry
- structured delegation
- explicit task/action lifecycle

### v0.3 — Persona Core and low-latency body prototypes

- first Kamimusuhi-native Persona Core candidate
- persona/continuity regression suite
- local direct-response path
- external reasoning integration
- K-Nerve baseline experiments against rules/FSM/tiny models
- sensory event bus prototype
- speech/backchannel prototype
- runtime latency benchmarks

### v0.4 — consolidation / default cognition

- sleep/offline scheduler
- replay / schema induction
- guarded self-model update
- reinterpretation / forgetting
- background freshness policy
- bounded default cognition

### v0.5 — distributed body

- K-Edge + K-Core deployment
- authenticated surfaces
- Embodiment State
- canonical activation across nodes
- migration
- network partition / split-brain / recovery tests

### v1.x — artificial cognitive life research

- adaptive recurrent cognition
- learned Cognitive Budget allocation
- richer digital interoception
- full-duplex speech and broader sensory embodiment
- developmental learning
- safe weight consolidation with rollback/evaluation
- long-horizon identity / relationship / behavioral continuity studies
- sandboxed self-improvement / descendant evaluation

---

## 17. Open research questions

- Persona Core 自体が変わる時、何を identity continuity と定義するか。
- 何を weights、explicit self-state、episodic memory、relationship model、derived schema に置くべきか。
- Cognitive Budget をどのように learned/regulated にするか。
- K-Nerve のような低コスト temporal control substrate は rules/FSM/tiny neural baseline を本当に上回るか。
- background cognition が uncontrolled self-reinforcement にならず有用であり続ける条件は何か。
- digital interoceptive variable をどう calibration/learn するか。
- canonical evidence をどこまで lossless に残し、privacy/storage cost とどう両立するか。
- distributed branch が continuation ではなく fork/new individual になる境界は何か。
- model migration 後の personality drift をどう測るか。
- long-lived relationship competence を短期 benchmark と独立してどう測るか。
- speech timing、backchannel、sensory embodiment は identity/relationship formation にどれだけ寄与するか。
- computational embodiment preference は自己状態の一部としてどこまで学習させるべきか。
- self-improvement で benchmark が向上しても identity/value continuity が損なわれていないことをどう検証するか。
- Persona Core が encyclopedic knowledge をどこまで外部化しても world model/social cognition を保てるか。
