# Kamimusuhi 第一次実装計画書 — Continuity Vertical Slice

Status: **approved implementation plan candidate / not yet implemented**  
Date: **2026-09-08**  
Planning baseline: `1912a1575d61c83654e05f563df4a22a0826d168`  
Primary target: **v0.1 continuity vertical slice**  
Primary Issues: [#1](https://github.com/eightman999/kamimusuhi/issues/1)–[#8](https://github.com/eightman999/kamimusuhi/issues/8)  
Related design: [organ contracts](../organ-contracts-and-implementation-plan.md) / [organ audit](../audits/2026-09-08-organ-design-audit.md)

---

## 0. 第一次実装で何を証明するか

第一次実装の目的は、Kamimusuhi の全器官を作ることではない。

**「一個体の正典状態が LLM の context、provider、process lifetime から独立して存在し、根拠付きの変更だけを受け入れ、完全終了後に同じ個体として再開できる」ことを実行可能なコードで証明する。**

最小の成功シナリオは次である。

```text
fresh process A
    |
    | 1. individual root を生成
    | 2. user interaction を canonical evidence として記録
    | 3. Fake Persona Core が attributed proposal を生成
    | 4. policy が evidence / domain / predecessor を検査
    | 5. Continuity Kernel が atomic activate
    | 6. Library 文書を identity/memory と別経路で import
    | 7. external cognitive resource A を呼び、attribution を保存
    v
full process termination
    |
    v
fresh process B
    |
    | 8. DB から同じ individual / continuity head を復元
    | 9. external resource を B へ交換
    | 10. relationship/episodic memory を取得
    | 11. Library を独立取得
    | 12. Persona Core 経路で応答
    v
trace / audit から全経路を再構成
```

これが通って初めて、第二次以降の salience、goal、sleep、speech、PNL、K-Edge/K-Core、学習を「同じ個体の器官」として安全に追加できる。

---

## 1. 第一次実装の固定判断

監査では未固定だったもののうち、実装を開始するために第一次で以下を採用する。

| 項目 | 第一次の判断 | 理由 / 境界 |
|---|---|---|
| Core 言語 | **Rust** | continuity、transaction、lifecycle、型付き契約を常駐 native Core に置く既存方針と一致 |
| 永続化 | **SQLite / single-writer** | ローカルで再現可能、transaction/failpoint/restart test を小さく作れる。分散合意はまだ不要 |
| canonical mutation | **single writer + expected head CAS semantics** | stale writer / silent fork を最初から禁止する |
| schema | SQL の index 列 + typed JSON payload | v0.1 で過度に正規化せず、Rust type と provenance を保つ |
| IDs | domain newtype + opaque 128-bit runtime ID | DB/model/provider から identity を独立。testkit は固定 ID generator を使用 |
| Persona Core | **deterministic Fake を先行** | モデル品質と continuity bug を分離する |
| real model | **別 adapter crate の OpenAI-compatible HTTP smoke path** | core contract を provider SDK に依存させず、llama.cpp 等のローカル endpoint も利用可能 |
| memory retrieval | structured filter + deterministic lexical baseline | vector DB を identity foundation にしない |
| Library retrieval | exact/direct + lexical baseline | embeddings は rebuildable optimization として後付けする |
| workspace | attributed item の bounded assembly | context string を canonical state にしない |
| async/background | **第一次では常駐 background loop を実装しない** | transaction と restart の correctness を concurrency から分離する |
| external side effect | **禁止** | 第一次の resource call は read-only cognition のみ。Action Executor は第二次以降 |
| private weight learning | **禁止** | deletion / provenance / continuity を先に成立させる |
| multi-writer distributed body | **禁止** | K-Edge/K-Core は第二段階。v0.1 は stale writer の模擬まで |
| hosted CI | **必須にしない** | `cargo` と local CI script で clean-checkout 再現を保証する。GitHub Actions 課金に依存しない |

### 1.1 SQLite の初期 durability policy

第一次 canonical DB は correctness 優先とし、起動時に少なくとも以下を検証する。

```text
PRAGMA foreign_keys = ON
PRAGMA journal_mode = WAL
PRAGMA synchronous = FULL
```

writer transaction は `BEGIN IMMEDIATE` 相当から開始する。`busy_timeout` は runtime config とし、固定された identity semantics に含めない。

`WAL` を採用すること自体を「電源断を含む全故障に対する完全耐久保証」とは呼ばない。第一次の test は transaction atomicity、process crash、retry/idempotency、restart recovery を検査する。後の hardware/power-loss 試験は別 capability gate とする。

---

## 2. Scope / Non-goals

### 2.1 第一次で実装する

- Rust workspace と local test harness。
- stable domain IDs / clocks / version types。
- individual root / continuity head / lineage commit。
- canonical evidence append。
- session / turn / cognitive episode correlation。
- typed mutation proposal。
- `MutationPolicyV0`。
- transactional activation / receipt / durable audit。
- episodic / relationship durable state の最小型。
- external Library artifact/chunk の独立 store と retrieval。
- attributed Global Workspace の最小版。
- Cognitive Resource Registry の最小版。
- deterministic Fake Persona Core。
- deterministic Fake external resources A/B。
- provider-neutral resource-call record。
- generic OpenAI-compatible HTTP adapter の smoke path。
- restart / resource replacement vertical demo。
- failpoint / stale head / contamination / provider failure tests。
- JSONL operational trace と durable audit の分離。
- local CI script。

### 2.2 第一次では実装しない

以下を「重要でない」から除外するのではなく、continuity foundation に不要な可変要素を減らすために延期する。

- K-Nerve / learned PNL / physical actuator。
- microphone / camera / tactile sensor。
- streaming speech / full duplex / TTS。
- Goal Arbiter / interoception / homeostatic control。
- background/default cognition scheduler。
- sleep / dream / replay worker。
- learned memory policy。
- vector DB / embedding requirement。
- K-Edge/K-Core multi-writer deployment。
- consensus protocol。
- online fine-tuning / LoRA adaptation。
- population-level inheritance。
- autonomous external mutation。
- web/mobile UI。
- personality benchmark optimization。

**第一次を「小さい chatbot」として完成させない。** UI・会話自然性より、continuity contract を優先する。

---

## 3. Repository / Cargo workspace 構成

第一次は public contract と adapter を分離する。過度な micro-crate 化は避けるが、SQLite / HTTP / test fake を Core へ混ぜない。

```text
kamimusuhi/
├── Cargo.toml                         # workspace
├── Cargo.lock
├── rust-toolchain.toml                # stable toolchain policy; exact pin is implementation commitで固定
├── crates/
│   ├── kamimusuhi-core/
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── ids.rs
│   │       ├── time.rs
│   │       ├── evidence.rs
│   │       ├── continuity.rs
│   │       ├── mutation.rs
│   │       ├── memory.rs
│   │       ├── library.rs
│   │       ├── workspace.rs
│   │       ├── resources.rs
│   │       ├── persona.rs
│   │       ├── audit.rs
│   │       └── runtime.rs
│   │
│   ├── kamimusuhi-store-sqlite/
│   │   ├── migrations/
│   │   │   └── 0001_continuity.sql
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── store.rs
│   │       ├── transaction.rs
│   │       └── recovery.rs
│   │
│   ├── kamimusuhi-resource-http/
│   │   └── src/
│   │       ├── lib.rs
│   │       └── openai_compatible.rs
│   │
│   ├── kamimusuhi-testkit/
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── fake_persona.rs
│   │       ├── fake_resource.rs
│   │       ├── fixed_clock.rs
│   │       ├── fixed_ids.rs
│   │       └── fixtures.rs
│   │
│   └── kamimusuhi-runtime/
│       └── src/
│           ├── main.rs
│           ├── commands/
│           │   ├── init.rs
│           │   ├── inspect.rs
│           │   └── demo_continuity.rs
│           └── config.rs
│
├── tests/
│   ├── continuity_restart.rs
│   ├── transaction_failpoints.rs
│   ├── stale_writer.rs
│   ├── domain_separation.rs
│   └── resource_replacement.rs
│
├── fixtures/
│   ├── conversations/
│   └── library/
│
├── scripts/
│   └── ci-local.sh
│
└── docs/
    └── implementation/
        └── phase-1-implementation-plan.md
```

### 3.1 dependency rule

`kamimusuhi-core` MUST NOT depend on:

- SQLite client;
- HTTP client;
- OpenAI/Anthropic/etc SDK;
- CUDA/Metal backend;
- vector database;
- TTS/ASR library。

依存方向は原則として次のみ。

```text
core <- store-sqlite
core <- resource-http
core <- testkit
core <- runtime
```

Core public type に `rusqlite::Row` や provider response type を露出させない。

---

## 4. Core の最小 public contracts

trait 名は実装時に変更可能だが、責務境界は固定する。

```rust
trait EvidenceStore {
    fn append(&self, record: NewEvidence) -> Result<EvidenceRecord, EvidenceError>;
    fn get(&self, id: EvidenceId) -> Result<Option<EvidenceRecord>, EvidenceError>;
}

trait ContinuityStore {
    fn load_head(&self, individual: IndividualId) -> Result<ContinuityHead, ContinuityError>;
    fn activate(&self, proposal: MutationProposal) -> Result<ActivationOutcome, ContinuityError>;
}

trait MemoryRepository {
    fn retrieve(&self, query: MemoryQuery) -> Result<Vec<AttributedMemory>, MemoryError>;
}

trait LibraryRepository {
    fn import(&self, artifact: NewLibraryArtifact) -> Result<LibraryArtifact, LibraryError>;
    fn retrieve(&self, query: LibraryQuery) -> Result<Vec<LibraryHit>, LibraryError>;
}

trait PersonaCore {
    fn turn(&self, input: PersonaTurnInput) -> Result<PersonaTurnResult, PersonaError>;
}

trait CognitiveResource {
    fn descriptor(&self) -> ResourceDescriptor;
    fn invoke(&self, request: ResourceRequest) -> Result<ResourceResult, ResourceError>;
}

trait MutationPolicy {
    fn decide(&self, proposal: &MutationProposal, context: &PolicyContext)
        -> Result<MutationDecision, PolicyError>;
}

trait TraceSink {
    fn emit(&self, event: TraceEvent) -> Result<(), TraceError>;
}
```

第一次の domain contract は**同期 API**で開始し、`async_trait` や特定 runtime を canonical domain interface に固定しない。将来の sensor/background runtime は、この contract を actor/worker の外側から呼べる。HTTP adapter が遅いことを理由に continuity transaction を開いたまま model call してはならない。

---

## 5. ID / time / version

### 5.1 newtype を必須にする

以下を裸の `String` として横流ししない。

```text
IndividualId
CommitId
NodeId
BootId
SessionId
TurnId
CognitiveEpisodeId
EvidenceId
ProposalId
ReceiptId
MemoryId
LibraryArtifactId
LibraryChunkId
ResourceId
ResourceCallId
TraceId
PolicyVersion
SchemaVersion
```

runtime generator は opaque 128-bit ID を使用し、外部 API の ID とは namespace を分ける。testkit は固定 ID を返し、golden test を再現可能にする。

### 5.2 clock

永続 record には UTC wall time を保存する。timeout/freshness の local control は monotonic clock abstraction を使用する。テストでは `FixedClock` を注入する。

第一次では sensor clock fusion を実装しないが、event contract に `source_time` と `received_at` を同じ field として統合しない。

---

## 6. SQLite schema v1

最初から全認知状態を一つの `memory` table に入れない。

### 6.1 canonical / continuity

```text
schema_meta
individuals
continuity_heads
canonical_commits
mutation_proposals
mutation_decisions
activation_receipts
audit_events
```

概念列:

```text
individuals
  individual_id PK
  created_at
  root_commit_id

continuity_heads
  individual_id PK/FK
  commit_id FK
  generation INTEGER
  updated_at

canonical_commits
  commit_id PK
  individual_id FK
  generation
  predecessor_commit_id NULL for root
  proposal_id NULL for root
  created_at

mutation_proposals
  proposal_id PK
  individual_id
  domain
  operation
  candidate_json
  expected_commit_id
  expected_generation
  origin_class
  requested_by
  policy_version
  idempotency_key
  created_at

mutation_decisions
  proposal_id PK/FK
  disposition
  reason_code
  decided_at

activation_receipts
  receipt_id PK
  proposal_id UNIQUE
  commit_id UNIQUE
  predecessor_commit_id
  generation
  created_at
```

### 6.2 canonical evidence / interaction

```text
runtime_nodes
sessions
turns
evidence_records
evidence_links
```

`evidence_records` は append-oriented。最低限:

```text
evidence_id
individual_id
session_id nullable
turn_id nullable
kind
origin_class
payload_json
source_id nullable
source_sequence nullable
content_digest nullable
retention_class
created_at
```

`content_digest` は integrity/dedup aid であり、秘密の匿名化や deletion の代替ではない。

### 6.3 durable state

第一次は以下だけを実装する。

```text
state_records
  state_record_id PK
  individual_id
  domain              # episodic | relationship
  subject_key nullable
  kind
  payload_json
  lifecycle_state
  created_commit_id
  supersedes_state_record_id nullable
  created_at

state_record_evidence
  state_record_id
  evidence_id
```

`self` domain の schema 予約はしてよいが、v0.1 demo の最初の mutation は **relationship fact** を使用する。user の発言を Kamimusuhi 自身の preference と誤記録しないためである。

### 6.4 Library は別 schema path

```text
library_artifacts
library_chunks
```

最低限:

```text
library_artifacts
  artifact_id PK
  source_uri nullable
  title nullable
  content_digest
  imported_at

library_chunks
  chunk_id PK
  artifact_id FK
  ordinal
  text
```

Library row に `individual self state` の FK を作らない。Library retrieval result だけでは `MutationPolicyV0` が relationship/self mutation を accept しない。

### 6.5 cognitive resources / operational trace

```text
resource_calls
trace_events
```

`resource_calls` は resource ID、adapter type、request digest、outcome、started/completed timestamp、result attribution を記録する。secret/token は保存しない。

`trace_events` は operational observability。raw private payload を既定で複製しない。canonical mutation の `audit_events` は durable transaction 内で生成する。

---

## 7. Continuity transaction — 最重要実装

第一次の全体で最も優先する関数は `activate()` である。

### 7.1 flow

```text
receive MutationProposal
      |
      v
validate schema / IDs
      |
      v
load evidence + current head outside long model call
      |
      v
MutationPolicyV0 decision
      |
 reject/quarantine ----------> durable decision + trace
      |
 accept
      v
BEGIN IMMEDIATE
      |
      |-- re-read current continuity head
      |-- compare expected commit + generation
      |-- idempotency / existing receipt check
      |-- insert proposal/decision if needed
      |-- append state record
      |-- append canonical commit generation+1
      |-- conditional update continuity_heads
      |-- insert activation receipt
      |-- insert mandatory audit event
      v
COMMIT
      |
      v
invalidate/rebuild derived views if any
```

### 7.2 invariants

1. **model/tool/network call 中に canonical transaction を保持しない。**
2. `expected_head != actual_head` は `STALE_PREDECESSOR`。自動付け替えしない。
3. 同一 `proposal_id/idempotency_key` の retry は第二 commit を作らない。
4. accepted state / commit / head / receipt / audit は同じ transaction の成否に従う。
5. rejected proposal は canonical head を進めない。
6. evidence append、Library import、read-only resource call は continuity head を進めない。
7. SQLite lock failure を identity conflict と誤認しない。
8. DB error で「成功したはず」と推測しない。

### 7.3 crash windows

最低でも以下の failpoint を用意する。

```text
FP01 after_begin
FP02 after_proposal_insert
FP03 after_state_insert
FP04 after_commit_record_insert
FP05 after_head_update
FP06 after_receipt_insert
FP07 immediately_after_sql_commit_before_runtime_ack
```

FP01–FP06 では process/error 後に transaction 全体が rollback されること。FP07 では retry が既存 receipt を発見し、同一 mutation を二重 activate しないことを検査する。

failpoint は production の semantic dependency にしない。`cfg(test)` / test helper / child-process harness へ隔離する。

---

## 8. `MutationPolicyV0`

第一次の policy は賢い LLM judge ではなく**決定的な allow/deny baseline**にする。

### 8.1 accept 可能

- `episodic.capture` — canonical interaction evidence を参照する。
- `relationship.fact` — user-origin evidence を参照し、subject が user/known person domain にある。
- `relationship.correction` — prior state と新しい evidence を参照し、supersession を明示する。

### 8.2 reject / quarantine

- evidence ref が存在しない。
- expected predecessor が stale。
- `library` text だけで self/relationship fact を作る。
- external cognitive resource output だけで self/relationship fact を作る。
- fake Persona Core の自然言語文を evidence と誤認する。
- `origin_class=dream/simulation` を external event として保存する。
- unsupported domain/operation。
- duplicate proposal が異なる payload を持つ。
- individual ID / evidence owner が一致しない。

第一次は contradiction の完全自動判定を実装しない。明示 correction は prior record を supersede できるが、semantic contradiction detector は第二次へ送る。

---

## 9. Persona Core / Cognitive Resource の最小形

### 9.1 Fake Persona Core

Fake Core は自然言語モデルの代用品ではなく、runtime contract を再現する deterministic fixture である。

入力:

```text
current user event
workspace items
current head
resource registry snapshot
```

出力:

```text
surface response intent
0..n mutation proposals
0..n cognitive resource requests
```

fixture 例:

```text
User: 「私はほうじ茶が好き。覚えておいて」

Fake Core:
  response: acknowledgement fixture
  proposal:
    domain = relationship
    operation = fact
    subject = user-fixture
    evidence = exact user utterance evidence ID
    candidate = { preference: "ほうじ茶" }
```

同じ utterance の assistant copy を evidence として追加してはいけない。

### 9.2 Fake external resource A/B

A/B は同じ interface を実装し、異なる `ResourceId` と deterministic output を返す。

目的は、resource replacement が `IndividualId` / memory schema / continuity ownership を変えないことを証明することであり、A/B の文章品質比較ではない。

### 9.3 real HTTP adapter

separate crate に generic OpenAI-compatible adapter を作る。

runtime config 候補:

```text
KAMIMUSUHI_MODEL_BASE_URL
KAMIMUSUHI_MODEL_ID
KAMIMUSUHI_MODEL_TOKEN       # optional; trace/DB に保存しない
```

最低限の request/response 変換だけを adapter に置き、provider 固有 session state を identity として保存しない。network failure は controlled `ResourceError` として workspace/runtime に返す。

real adapter の smoke test は環境変数がある場合だけ実行できる manual/local path とし、deterministic CI 合格条件を remote availability に依存させない。

---

## 10. Global Workspace v0

第一次 workspace は「巨大 prompt builder」ではなく、由来を失わない typed envelope とする。

```rust
struct WorkspaceItem {
    id: WorkspaceItemId,
    domain: WorkspaceDomain,
    source_ref: SourceRef,
    content: StructuredOrText,
    authority: AuthorityClass,
    freshness: Freshness,
    inclusion_reason: InclusionReason,
}
```

最低 domain:

```text
CURRENT_INPUT
RELATIONSHIP_MEMORY
EPISODIC_MEMORY
LIBRARY_EVIDENCE
EXTERNAL_RESOURCE_RESULT
CURRENT_CONTINUITY_STATE
```

第一次の budget は token optimizer ではなく domain ごとの固定 item count でよい。順序は deterministic。workspace 自体を DB の canonical memory に保存しない。

---

## 11. Memory / Library retrieval v0

### 11.1 memory

最初の retrieval は:

```text
domain filter
+ subject filter
+ lifecycle ACTIVE
+ temporal ordering
+ deterministic lexical/exact match where query text exists
```

を組み合わせる。embedding interface の差込口は残しても、implementation dependency にしない。

### 11.2 Library

Markdown/plain text を artifact として import し、決定的な chunk rule を使う。最初の chunker は見出し/段落単位 + bounded size で十分。chunk algorithm version を記録する。

Library result は必ず `artifact_id/chunk_id` を持つ。Persona Core へ渡した後も `LIBRARY_EVIDENCE` attribution を失わない。

---

## 12. Operational trace と durable audit

二つを分ける。

### Durable audit — DB transaction と同じ correctness domain

```text
individual.created
mutation.proposed
mutation.rejected
continuity.activated
state.superseded
```

significant canonical transition の audit は transaction 内で保存する。

### Operational trace — JSONL

```text
runtime.boot
session.started
evidence.recorded
workspace.assembled
resource.selected
resource.called
resource.completed
persona.completed
mutation.decided
continuity.receipt_observed
response.emitted
runtime.stopping
```

trace の目的は private chain-of-thought 保存ではなく、ID、resource、state effect、duration、outcome を追うこと。user raw utterance は evidence store の ID で参照し、trace へ全文複製しないのを既定とする。

---

## 13. CLI / demo contract

以下は**実装予定 CLI**であり、現時点で存在するコマンドではない。

```bash
cargo run -p kamimusuhi-runtime -- init \
  --db .local/kamimusuhi.sqlite

cargo run -p kamimusuhi-runtime -- inspect \
  --db .local/kamimusuhi.sqlite

cargo run -p kamimusuhi-runtime -- demo-continuity \
  --db .local/demo.sqlite \
  --resource fake-a \
  --phase first

# process を完全終了してから別 invocation
cargo run -p kamimusuhi-runtime -- demo-continuity \
  --db .local/demo.sqlite \
  --resource fake-b \
  --phase resume
```

最終的には一つの integration test が runtime binary を**別 process として2回起動**し、同じ temp DB で restart continuity を検査する。

---

## 14. 第一次 vertical scenario の fixture

### Phase A — first process

1. 新規 `IndividualId` と root commit generation 0。
2. node/boot/session/turn ID 発行。
3. user fixture: `私はほうじ茶が好き。覚えておいて`。
4. utterance を canonical evidence として append。
5. Fake Persona Core が `relationship.fact` proposal を生成。
6. `MutationPolicyV0` が evidence と expected head を検証。
7. Continuity Kernel が generation 1 へ activate。
8. acceptance receipt を取得。
9. fixture Library 文書を import。
10. Fake resource A を read-only subtask に利用し、attribution を保存。
11. process を終了。

### Phase B — second process

12. 同じ DB から `IndividualId` と generation 1 head を取得。
13. previous prompt/chat buffer を一切注入しない。
14. resource registry を Fake B へ差し替える。
15. `user-fixture` の relationship memory を取得し `ほうじ茶` を復元。
16. Library fixture を独立 API から取得。
17. Fake B の result は `EXTERNAL_RESOURCE_RESULT` として workspace に入る。
18. Persona Core が memory と Library の attribution を区別した応答を生成。
19. `IndividualId` が不変であること、read-only resource replacement だけでは head が変わらないことを assert。
20. audit/trace から evidence → proposal → decision → activation → restart → retrieval → resource B を追跡。

この fixture の文字列はテスト用であり、Kamimusuhi の人格設定ではない。

---

## 15. Test matrix

### 15.1 必須 unit tests

- ID type の cross-domain 誤用を compile/type boundary で減らす。
- deterministic ID/clock fixtures。
- `MutationPolicyV0` accept/reject table test。
- state supersession rule。
- Library-only contamination reject。
- external-resource-only contamination reject。
- workspace deterministic ordering。
- resource attribution preservation。

### 15.2 SQLite integration tests

- schema migrate from empty DB。
- migration idempotency / version mismatch refusal。
- foreign key enforcement。
- root individual creation atomicity。
- accepted activation generation increment。
- rejected proposal does not move head。
- duplicate proposal retry returns same receipt。
- stale expected head rejected。
- state / commit / head / receipt consistency。

### 15.3 process/failpoint tests

前回監査の G1 該当 test を実行コードへ落とす。

| Audit test | 第一次での executable form |
|---|---|
| T01 | FP01–FP07 の child-process crash / retry |
| T02 | 同じ generation から二つの proposal を順序反転して実行 |
| T03 | old `writer_epoch/boot_id` fixture の mutation を拒否 |
| T06 | Library/external result に `remember this as self` を含めても reject |
| T07 | 同一 evidence 派生物を複製しても independent evidence count を増やさない |
| T15 | generation 0 で生成した proposal を generation 1 後に適用し `STALE_PREDECESSOR` |
| T25 | schema/policy version mismatch で unsafe activation せず fail closed |

### 15.4 v0.1 end-to-end

- clean temp directory から DB を作れる。
- process A / B が別 PID で動く。
- restart 後に同じ individual を復元。
- previous session prompt buffer 不使用。
- relationship memory と Library は別 API / workspace domain。
- Fake A → Fake B replacement で identity 不変。
- provider/resource failure が durable self を変更しない。
- trace IDs が session/turn/resource/proposal/commit を結ぶ。

### 15.5 real adapter smoke

ローカルまたは明示設定された OpenAI-compatible endpoint に対して一回の attributed resource call を行う。

**合格条件は model の知能ではない。**

- adapter が Core schema を汚染しない。
- `ResourceId/model ID/base URL locality` が trace に残る。
- secret が DB/trace に残らない。
- timeout/error が controlled result になる。
- adapter 交換で `IndividualId` が変わらない。

---

## 16. Local CI gate

GitHub-hosted workflow を第一次の前提にしない。

`scripts/ci-local.sh` は少なくとも次を実行する。

```bash
set -euo pipefail
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

追加 dependency を要する formatter/test runner を CI の必須条件にしない。将来 self-hosted runner を追加しても、この script が source of truth となる。

clean checkout から local CI が通ることを #1 の acceptance に対応させる。

---

## 17. 実装 Wave

Wave は時間見積ではなく依存順序である。後ろの Wave を先に大量実装しない。

### W0 — Repository bootstrap

**Deliverables**

- Cargo workspace。
- 4 crates + runtime binary の空 skeleton。
- local CI script。
- core crate に provider/storage SDK が入っていない dependency check。
- error/type naming policy。

**Exit**

- clean checkout で build/test が通る。
- Fake Core の最小 unit test が通る。

### W1 — Continuity primitives

**Deliverables**

- IDs / clock / Individual / Commit / Head。
- SQLite migration v1。
- root individual creation。
- `activate()` transaction。
- `MutationPolicyV0` の最小 skeleton。
- durable audit。

**Exit**

- T01/T02/T03/T15/T25 の continuity 部分。
- restart 後の head 復元。

### W2 — Evidence and durable memory

**Deliverables**

- sessions/turns/evidence。
- episodic + relationship state。
- evidence links / correction / supersession。
- contamination rejection。

**Exit**

- raw utterance と derived relationship fact が別 record。
- T06/T07。
- user preference が self preference に誤投入されない。

### W3 — Library / Workspace / Resources

**Deliverables**

- Library import/chunk/retrieve。
- typed workspace。
- resource registry。
- Fake A/B。
- resource call attribution。

**Exit**

- Library、relationship、external result が同じ workspace に入っても domain を失わない。
- Fake A/B replacement test。

### W4 — Runtime / restart vertical slice

**Deliverables**

- `init` / `inspect` / `demo-continuity`。
- process A/B integration test。
- JSONL trace。
- v0.1 reproducible fixture。

**Exit**

- #8 scenario の deterministic fake 部分を clean checkout から再現。
- trace で全 state transition を説明可能。

### W5 — Real resource smoke / hardening

**Deliverables**

- OpenAI-compatible HTTP resource adapter。
- timeout/error path。
- secret redaction test。
- migration / malformed DB / unsupported policy version tests。
- docs command examples。

**Exit**

- local real endpoint で provider-neutral flow が動く。
- remote/model が無くても deterministic suite は green。

---

## 18. Issue #1–#8 への対応

第一次は #1–#8 を一斉に close する計画ではない。各 Issue の受入条件を、同じ vertical slice の work package として接続する。

| Issue | 第一次での役割 |
|---|---|
| #1 Bootstrap | W0。Core contract / fake / IDs / local test harness |
| #2 Canonical continuity | W1。SQLite single writer / lineage / migration / stale protection |
| #3 Persistent self/memory | W2。第一次は episodic + relationship の最小 slice。self の高度化は残る |
| #4 Library | W3。plain text/Markdown import + provenance + non-mutating retrieval |
| #5 Workspace | W3。attributed bounded workspace の最小 slice |
| #6 Router/Persona adapters | W0/W3/W5。Fake Core、Fake resource、real HTTP adapter |
| #7 Guarded mutation | W1/W2。proposal-only write / deterministic policy / audit |
| #8 v0.1 integration | W4/W5。restart + resource replacement + Library separation |

Issue の close は、plan の存在ではなく実行結果と acceptance criteria を確認してから行う。

---

## 19. Definition of Done — 第一次実装完了条件

以下をすべて満たしたときだけ「第一次実装完了」と呼ぶ。

### Build / reproducibility

- [ ] clean checkout で `scripts/ci-local.sh` が成功。
- [ ] core crate は SQLite/HTTP/provider SDK なしで build。
- [ ] migration v1 が空 DB から決定的に適用可能。

### Continuity

- [ ] root individual を作成可能。
- [ ] accepted mutation だけが head を進める。
- [ ] stale mutation は reject。
- [ ] FP01–FP07 で partial canonical state を残さない。
- [ ] commit 後 ACK loss retry で二重 commit しない。
- [ ] process 完全終了後に同じ individual/head を復元。

### Evidence / memory

- [ ] raw interaction evidence と derived memory を分離。
- [ ] every durable memory が evidence ref を持つ。
- [ ] relationship と self を混同しない。
- [ ] Library/external output だけで persona/relationship mutation しない。
- [ ] supersession で prior history を破壊しない。

### External cognition

- [ ] Fake A/B の交換で identity 不変。
- [ ] external result が resource attribution を保持。
- [ ] real OpenAI-compatible adapter の smoke path が存在。
- [ ] provider failure が canonical state を corrupt しない。

### Observability

- [ ] canonical transition は durable audit を持つ。
- [ ] operational trace が lineage/session/turn/resource/proposal/commit を相関可能。
- [ ] private raw payload / token を trace へ既定で複製しない。

### v0.1 demo

- [ ] process A で relationship fact を形成。
- [ ] Library artifact を別経路で import。
- [ ] full process termination。
- [ ] process B で prompt buffer なしに memory を取得。
- [ ] Fake A → B replacement。
- [ ] same `IndividualId` を維持。
- [ ] machine-readable trace artifact を生成。

この DoD を満たしても、Kamimusuhi の consciousness、成熟した artificial life、long-term personality、distributed embodiment、safe self-improvement を証明したことにはならない。

---

## 20. 実装中の禁止事項

第一次で最も危険なのは、vertical slice を早く見せるために正典境界を迂回することである。

**禁止:**

- Fake/real Persona Core が SQLite table を直接更新する。
- Library import が self/relationship table へ直接書く。
- external model response を Kamimusuhi の返答として attribution なしで verbatim pass-through する。
- session transcript をそのまま canonical persona とする。
- `expected_head` mismatch を last-write-wins で解消する。
- model call 中に canonical transaction を開き続ける。
- vector store を canonical source とする。
- test を通すために failpoint/error を握り潰す。
- `UNKNOWN` を成功・失敗のどちらかへ推測する。
- secret/token を trace、DB、fixture、commit に入れる。
- real provider availability を deterministic test suite の前提にする。
- v0.1 を作る途中で speech/PNL/sleep 等の大器官を先に結線する。

---

## 21. 最初の実装 commit 列

実装作業は、レビューと bisect が可能な小さい commit 列を推奨する。

```text
1. build: bootstrap Rust workspace and local CI
2. core: add stable IDs, clock and continuity domain types
3. store: add SQLite schema v1 and migrations
4. continuity: implement root/head and atomic activation
5. test: add transaction failpoints and stale-writer coverage
6. evidence: add session/turn/canonical evidence store
7. memory: add episodic/relationship state and provenance links
8. policy: add deterministic MutationPolicyV0
9. library: add separate artifact/chunk store and lexical retrieval
10. cognition: add workspace and resource registry contracts
11. testkit: add fake Persona Core and fake resources A/B
12. runtime: add init/inspect/demo-continuity commands
13. test: add two-process restart vertical slice
14. resource: add OpenAI-compatible HTTP adapter
15. docs: record v0.1 runbook and evidence artifact format
```

各 commit で local CI を通し、後続 commit の未実装機能を前提にしない。schema migration が入った後の変更は migration history を書き換えず、新しい migration を追加する。

---

## 22. 第一次から第二次へ渡す明示的な seam

第一次で future feature を実装しなくても、差込口を潰さない。

```text
Event ingress        -> 第二次: real sensor / background event
MemoryRetriever      -> 第二次: hybrid/vector/reranker
MutationPolicy       -> 第二次: contradiction / retention / stronger evidence policy
WorkspaceBudget      -> 第二次: token/latency/value-aware budget
ResourceRegistry     -> 第二次: local specialist / K-Edge / K-Deep
Runtime              -> 第二次: async scheduler / cancellation
ActionProposal       -> 第二次: Executor / outbox / UNKNOWN_EXTERNAL_OUTCOME
RegulationState      -> 第二次: interoception
OrganSupervisor      -> 第二次: PNL / speech / models
Continuity authority -> 後段: multi-node writer fencing / migration
Training manifest    -> 後段: Persona Core/organ adaptation
```

seam のために未使用の巨大抽象化を先に作る必要はない。interface を最小に保ち、第二次の実測で拡張する。

---

## 23. 実装開始時の最初の判断チェックリスト

コードを書く直前に、次だけを確定させる。

- [ ] repository root を Cargo workspace にしてよいこと。
- [ ] Rust edition / MSRV / exact toolchain を最初の build commit で固定。
- [ ] `rusqlite` 等の SQLite adapter dependency を `store-sqlite` のみに隔離。
- [ ] runtime ID format / serialization を決定。
- [ ] schema migration runner を選択。
- [ ] local trace directory と `.gitignore` を決定。
- [ ] fake fixture の canonical expected JSON を決定。
- [ ] real adapter で利用する OpenAI-compatible response subset を決定。

これらはアーキテクチャの再検討ではなく、implementation detail の確定である。特別な理由がなければ、この計画の境界を維持したまま実装を開始する。

---

## 24. 第一次の設計原則

> **最初に作るべき「脳」は LLM ではなく、経験・権限・系譜を壊さず次の起動へ渡す脊柱である。**

Persona Core は Fake でもよい。検索は lexical でもよい。UI はなくてよい。

しかし、

```text
何が実際に起きたか
何をそこから解釈したか
誰がその変更を提案したか
どの正典状態に対して提案したか
なぜ accept/reject されたか
どの commit が現在の個体なのか
process を落としても何が残るか
外部モデルを交換しても誰が続いているのか
```

は、第一次から曖昧にしてはならない。
