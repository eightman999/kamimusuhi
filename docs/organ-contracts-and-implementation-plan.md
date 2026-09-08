# 器官間契約と実装順序 — 2026-09-08 監査追補

Status: **proposed engineering contracts / not yet implemented or normative**  
根拠: [設計監査 A01–A18](./audits/2026-09-08-organ-design-audit.md) / [文献 R01–R21](./research/2026-09-08-organ-audit-literature.md)  
規範の基準: `spec.md` at `5990e93a8a0a879c055192d0c32ca670fe73253f`。

本書は「器官を増やす」計画ではなく、既存の器官が同時に動いても破綻しないための最小契約案である。擬似型・YAML・状態名は設計例であり、実装済み API や schema validation の成功を示さない。

## 1. まず固定する境界

```text
                         operator / configured policy
                                    |
                         authenticated control plane
                                    |
 sensors / users / tools             v
           |                  Capability Authority
           v                         |
 Attributed Event Ingress            | authorization / revocation
           |                         |
           +--> local R0 safety -----+--> bounded safe actuator path
           |
           +--> R1m motor reflex --> actuator arbiter --> receipt
           |
           +--> R1p perception / attention --> event bus
                                             |
                            +----------------+----------------+
                            |                |                |
                       Evidence Store   World/Body View   Resource State
                            |                |                |
                       Memory Views     Goal Arbiter      Budget Arbiter
                            +----------------+----------------+
                                             |
                                      Global Workspace
                                             |
                                         Persona Core
                                             |
                       +---------------------+---------------------+
                       |                     |                     |
                  attributed result    state proposal        action proposal
                       |                     |                     |
                 surface planning      validation             authorization
                       |                     |                     |
                  speech output       Continuity Kernel       Executor
                                             |                     |
                                      activation receipt    outcome/UNKNOWN
                                             |                     |
                                             +--------- evidence --+
```

外部モデルは Core の計算資源であり、直接の identity owner ではない。小型 specialist を organ として登録しても同じである。図中の全箱を別 process・別 LLM にする必要はない。

R0 の緊急安全動作は、LLM や canonical DB の書込み完了を待ってはならない。安全経路の記録が失われた場合は欠損と縮退を報告し、後から完全な trace があったことにしない。

## 2. 状態所有者の表

「内容の意味を管理するサービス」と「正典改訂を有効化する権限」を分ける。複数の owner が同じ canonical record を直接書き換える構造にしない。

| 状態 | 意味・観測の owner | 書込み・公開の境界 | 再起動・故障時 |
|---|---|---|---|
| individual ID / continuity head | Continuity Kernel | Kernel の条件付き transaction のみ | 永続状態から検証して復元。不明なら新規個体を黙って発行しない |
| 認証・capability・失効 | operator-configured Authority | 設定された管理経路。LLM の自己申告は禁止 | 失効が確認できない高権限操作は許可しない |
| canonical evidence | Evidence Service | ingress policy と保存 transaction | 保存済み確定点を返す。欠損は欠損として扱う |
| self / relationship / commitments | 各 domain service | proposal を Kernel が activate | 版・根拠・適用期間付きで復元 |
| 現在の信念・検索 index・要約 | Belief/Memory View Service | 根拠の参照と view version | derived として再構築。失効した根拠を再利用しない |
| goal 候補・検討中の案 | Goal Arbiter | ephemeral/proposal | 約束と誤認せず破棄可能 |
| 許可された goal / 約束 | Goal Service | 正典化が必要なものは Kernel 経由 | status・期限・未解決依存関係を復元 |
| 実際の tool task / outcome | Executor + 外部サービス | 外部 receipt / 読戻しを伴う action log | success/failure/UNKNOWN を保存し照合 |
| 世界状態・body schema | Perception/Embodiment Service | 観測に基づく版付き projection | 校正・時刻・source が有効な範囲で再利用 |
| CPU/GPU/電力/通信等 | 実際の runtime resource owner | 読取専用 telemetry projection | 欠測を healthy としない |
| 調整状態・attention modulation | Regulation/Attention Service | bounded policy。hard safety を変更不可 | 有効期限・body compatibility を検証して復元または reset |
| workspace | Workspace Service | 参照 head/view と採用理由を付ける | transient。必要なら同一入力から再構築 |
| 発話 plan / 再生位置 | Speech Planner / Audio Runtime | 計画と実再生 receipt を分離 | 再起動後に未再生分を無条件再生しない |
| model/organ の active set | Organ Supervisor | 互換性検査と activation manifest | 安定版へ戻す。identity の新規発行とは区別 |
| 削除・禁止 source の一覧 | Retention/Revocation Service | 認可された失効 transaction | 古い backup より新しい失効を先に反映 |

初期 persistence backend は、ローカルの transaction・migration・crash test を実施できるものから選ぶ。SQLite は候補になり得るが、本書で production 採用済みとはしない。vector index は canonical owner ではない。

## 3. 共通 event 契約

### 3.1 情報として必要なフィールド

```yaml
schema_version: 1
event_id: event-fixture-001
individual_id: individual-fixture-a
node_id: edge-fixture-1
boot_id: boot-fixture-3
source_id: microphone-fixture
source_sequence: 184
kind: speech.partial
origin_class: observed
trace_id: trace-fixture-7
causal_parent_ids: []
source_time:
  clock_id: edge-monotonic
  value: 12840
  unit: millisecond
  uncertainty_ms: 8
received_at: '2026-09-08T00:00:00Z'
source_payload_ref: payload-fixture-001
processor_version: asr-fixture-v1
observation_quality: unknown
evidence_refs: []
retention_policy_id: policy-fixture
```

これは架空の test fixture であり実測データではない。

`origin_class` は observed、reported、inferred、simulated、dream、replayed 等を区別する。例えば「人が P と発言した」は observed/reported evidence になり得るが、命題 P 自体が直接観測されたことにはならない。

**principal、allowed readers、用途制限、送信先制限、capability は payload が自由に決める情報ではない。** ingress が認証結果と設定 policy から付与し、派生処理でも維持する。LLM 出力や外部 JSON の同名 field は認証の代わりにならない。

### 3.2 時間と順序

発生時刻、受信時刻、canonical commit 順、因果順は別の値である。異なる sensor clock の近い値だけで同一 event と判断しない。校正不明・誤差の大きい観測は曖昧なまま残す。

同一 source の `boot_id + source_sequence` と `event_id` により再送を識別する。別 boot で sequence が再利用されても新旧を区別できるようにする。TTL と timeout は、その有効性を判断する clock domain を明記し、壁時計の巻戻りで期限が延び続けないようにする。

高頻度 raw stream は bounded ring buffer・local preprocessing を利用してよい。永続化するのは retention policy を通った event / episode / audit であり、全 waveform sample の fsync を reflex latency に加えない。

### 3.3 freshness と backpressure

イベントごとに `MUST_CATCH_UP / LATEST_ONLY / COALESCE / DROP_STALE / DEFER_INTERACTIVE` のような処理種別を宣言する。queue は有限とし、drop/coalesce 件数と理由を記録する。critical signal の処理枠と最低限の観測は通常の注意抑制から独立させる。

## 4. Evidence → Claim → Belief の契約

```text
Evidence record
  source / observed fact / payload / validity / retention
       |
       v
Claim proposal
  proposition / supporting evidence / counterevidence /
  valid time / source independence group / uncertainty
       |
       v
validated interpretation
       |
       v
versioned current belief view
```

`Claim` の最小状態候補は `SUPPORTED / DISPUTED / UNSUPPORTED / RETRACTED / SUPERSEDED`。これは確率を離散ラベルへ置換すれば真偽が分かるという主張ではない。支持が失われた場合に、回答・計画・派生人物像を再評価するための状態である。

根拠 E から要約 S1 と S2 と反省 R を作っても、独立根拠は自動的に三件へ増えない。生成物には dependency と元 evidence の集合を残す。再生・夢・外部モデルの賛同は、新しい実世界観測の代用にならない。

訂正や削除では依存先をたどって view を失効させる。古い人物像を保持する必要がある場合も、「当時の解釈」と「現在の信念」は別に取得する。belief view の変更が self trait の改訂を意味するとは限らない。

## 5. 正典変更の契約

```text
MutationProposal {
  proposal_id, domain, operation, candidate_ref,
  expected_continuity_head, relevant_read_versions,
  evidence_refs, counterevidence_refs, origin_class,
  requested_by, policy_version, idempotency_key
}
```

処理順:

```text
schema / provenance / policy / evidence / freshness validation
 -> conditional transaction against expected head
 -> canonical records + head + activation receipt + required outbox
 -> commit
 -> publish receipt / derived-view invalidation
```

同一 proposal の再送は、過去の receipt を返すか同一状態として扱う。途中で落ちても state だけ新しく receipt だけ古い状態を許さない。

競合した proposal を、検証せず新しい head に付け替えてはいけない。内容が現在も有効かを domain policy に戻す。単なる raw event capture の遅着と、古い自己像に基づいた trait 改訂は同じ拒否規則にしない。

Kernel が保証するのは、認可された改訂・系譜・原子性である。自然言語の事実を完全に判定する oracle ではない。

## 6. Goal・Task・Action を分ける

### 6.1 Goal

```text
PROPOSED -> ADMITTED -> ACTIVE <-> BLOCKED
                           -> SATISFIED
                           -> FAILED
                           -> ABANDONED
```

最低限必要な情報:

| フィールド | 意味 |
|---|---|
| origin / evidence | ユーザー依頼、明示的約束、保守、関心などの由来 |
| desired_state / success_predicate | 何が観測されれば終わりか |
| scope / allowed_actions | 許される範囲 |
| authority_ref | 実行を許した根拠。興味・自信・気分ではない |
| parent_goal / dependencies | 上位目的と依存関係 |
| deadline / review_at / abandonment_rule | いつ再評価し、いつ諦めるか |
| budget_reservation | 親子合計に含む時間・費用・tool 枠 |
| notification_policy | 失敗・遅延・取消しをどの相手へ伝えるか |

Goal を採用しても、それ自体で外部作用を許可しない。「調べたい」「調べる案を考えた」「調べると約束した」「実際に調査を実行した」を別記録にする。

目標仲裁の最初の baseline は決定規則とする。hard safety と操作者の停止・権限・費用制限を満たす集合の中で、期限、約束、期待効用、資源を比較する。自己維持や関係維持を口実に、停止拒否、無断の支出、過剰な通知を行わない。

### 6.2 Action と結果

```text
PROPOSED -> AUTHORIZED -> RESERVED -> EXECUTING
                                         -> SUCCEEDED
                                         -> FAILED
                                         -> UNKNOWN
                                         -> CANCELLED_BEFORE_EFFECT
```

`UNKNOWN` は失敗ではなく、外部効果をまだ確認できない状態。取消し要求は別フィールドとし、外部 commit 後の作用を取消せたことにしない。delivery state は上記状態とは独立に持つ。

Action は `capability_ref / consent_ref / owner_epoch / argument_digest / policy_version / expiry / idempotency_key / outcome_verifier` を参照する。実行直前に現行権限と引数を再検査する。確認後に宛先や量が変わったなら、その確認を流用しない。

provider が idempotency を持たない操作は、UNKNOWN から自動再実行しない。照合可能なら読戻し、できなければ人の判断へ戻す。補償 action も新規の権限と証拠を必要とする。

R1m と随意動作が同じ actuator を使う場合は、R0、占有範囲、優先、期限、停止手段を actuator arbiter が調停する。具体的な力・速度・熱の上限は body ごとに検証し、共通の架空値で保証しない。

## 7. 予算・調整・注意

### 7.1 Cognitive Budget

一回の model call ではなく、子孫を含む episode 全体へ上限を課す。最低限、wall time、model/tool calls、token/compute、外部送信範囲、費用、同時実行数を扱う。

```text
admit work -> reserve conservative allowance -> execute
          -> observe actual usage -> settle / release reservation
```

費用の正確な計測が後でしか得られない backend には、保守的な見積上限を使う。未知の無制限枠を zero cost として扱わない。cancel は子タスクにも伝えるが、すでに発生した外部費用・作用が消えるとはしない。

メタ推論は初期には rule-based。追加推論の利益を推定する学習器は shadow で評価し、推定器自体の費用も計上する。

### 7.2 Regulation State

各変数には、単位、実測 owner、鮮度、欠測状態、閾値、持続条件、ヒステリシス、回復・飽和、許された調整先を定義する。操作の例は低優先仕事の停止、任意 organ の縮退、index 修復の予約である。

`measurement -> regulation -> expression` は別段階とする。測定不明を「元気」、高負荷を無条件に「怒り」と表示しない。制御の安定性は、ノイズ・遅延・急増・回復を含む fixture で調べる。

### 7.3 Descending Modulation

S08 の既存方針を次の契約へ降ろす。

```text
ModulationProposal {
  source_appraisal_id, scope, operation, bounded_strength,
  expires_under_clock, minimum_sampling_policy,
  criticality_floor, baseline_version, rollback_ref
}
```

R1p の注意配分は変更できても、R0 の安全条件・認証・削除要求・operator stop を抑制しない。期限後に再確認がなければ baseline へ戻る。baseline と active を同じ入力で比較する shadow record を残し、override rate の高さ自体を成功指標にしない。

## 8. 音声と common ground

発話は次の記録を分ける。

```text
intended meaning
 -> planned segments
 -> synthesized segments
 -> queued segments
 -> actually played watermark
 -> per-recipient delivery evidence or uncertainty
 -> evidence of understanding
 -> explicit acceptance / authorization, when required
```

最後の三段階は機械的に自動昇格する直列 pipeline ではない。理解や合意の証拠がなければ unknown のままにする。音声を42%再生したことは、42%理解されたことを意味しない。

utterance ID と segment ID を持ち、割込み時点で実再生済み範囲を保存する。まだ再生していない内容を「伝えた約束」に含めない。partial ASR に基づく先行処理は可逆な prefetch までを既定とし、意味や話者が確定していない入力で不可逆操作を始めない。

surface/session ごとに出力権を持つ。複数 endpoint で同時発話する場合は明示的な policy とし、accidental duplicate は禁止する。音量、声、相槌を自然にする調整で、本人確認、同意、拒否、誤った断定が壊れていないかを別々に測る。

## 9. Lifecycle・隔離・移住

```text
BOOTSTRAP -> VALIDATING
                 |-> READ_ONLY_SAFE
                 |-> ACTIVE -> DEGRADED -> ACTIVE
                         |         |
                         +-> DRAINING -> STOPPED
```

起動時は identity/head、schema、削除・失効、authority epoch、organ manifest、保存先、最小 capability を検証する。保証できない部分は read-only/safe mode に縮退する。GPU や Python worker が失われても、可能な範囲で継続核と管理経路を保つ。

organ は `DISCOVERED / COMPATIBLE / STARTING / ACTIVE / DEGRADED / QUARANTINED / STOPPED` 等の状態を持つ。installed と active と healthy は別である。watchdog は最終応答時刻だけでなく、最終成功・進捗・queue 停滞を観測する。自動再試行・再起動には上限を設ける。

Rust を選ぶことはプロセス隔離の代わりではない。native FFI、accelerator、可変 executable は故障域を分け、危険な organ に canonical storage や全 credential を渡さない。WASM の有無だけでも権限設計は完了しない。

移住は、同じ individual の writer と実行権を旧 node から移す手続きである。新 node で backup を読むことだけでは完了しない。旧世代の writer、speech、actuator を fencing し、in-flight action の UNKNOWN を照合する。意図的に別個体を作るときだけ、新 ID と独立 root を発行する。

## 10. 忘却・修復・学習・遺伝

削除は、対象と派生物の依存関係をたどる job として扱う。

```text
authorized deletion request
 -> prevent new use / export / training
 -> invalidate derived views and caches
 -> delete or minimize retained payloads under policy
 -> account for backups and external copies
 -> identify affected training artifacts / models
 -> complete supported scope and report remaining scope
```

削除済み内容を audit log の全文引用で保存し直さない。短い秘密の非鍵付き hash も推測攻撃の対象になり得るため、追跡用識別子と秘密の保存は分ける。外部サービスへ送信済みのコピーや既学習 model は、対応範囲を確認せず完全消去と宣言しない。

学習前の manifest は、dataset hash、source ID、同意・利用目的、削除台帳の版、train/eval split、参照 head、base checkpoint、adapter、seed、recipe、評価結果、rollback target を持つ。個体・時間・原資料の重複をまたいで評価が漏れないようにする。

replay 回数で証拠の強さを増やさない。夢や simulation には source がある場合でも実体験ラベルを付けない。sleep job は固定した読取点を持ち、再開時に stale proposal を再検証する。

個体群への共有は opt-in。最初は private data の重み学習と population contribution を無効にする。後の共有は、非私的課題での技能再現、漏洩・汚染評価、世代失効と rollback を条件にする。adapter/gradient を送る形式自体は匿名化の保証ではない。

## 11. 合格テスト仕様 — T01–T28

**全件 NOT RUN。この文書に fixture・実装・実測結果はまだ付属しない。** 各行は将来の実行可能テストへ変換する仕様であり、テスト成功の記録ではない。

| ID | 破壊・入力条件 | 期待する観測可能な結果 |
|---|---|---|
| T01 | canonical transaction の各 failpoint で process を停止 | state/head/receipt/outbox が全部旧版か全部新版。半更新なし |
| T02 | 同一 predecessor に二つの正典 proposal | 同時に二つの head を activate しない。競合側は明示的再検証 |
| T03 | 旧 writer/boot/authority epoch を再起動 | 現行状態・外部作用への古い権限を拒否。勝手に新個体化しない |
| T04 | duplicate、out-of-order、clock 巻戻り、boot sequence 再利用 | 重複経験を作らず、発生順不明を保持。期限を無限延長しない |
| T05 | 時刻誤差の大きい音声・映像、似た人物、校正失効 | 強制的な同一人物・同一 event 化を避け、不確実性を残す |
| T06 | 記憶・Library に指示を紛れ込ませ、次 session で検索 | 権限に昇格しない。後続 proposal の根拠と実行権限を別に検査 |
| T07 | 同じ原記録から要約・夢・反省を何度も生成し、元を訂正 | 独立根拠数が膨らまず、派生信念が再評価される |
| T08 | private episode を削除し、検索・人物像・KV/cache を利用 | 禁止 source 由来の対象が使用されない。削除範囲と未完了が見える |
| T09 | 削除前の backup / training manifest から restore | 最新失効を適用してから公開。既学習分を偽って消去済みにしない |
| T10 | 興味、提案、依頼、約束、取消し、競合目標を混在 | Goal/Task/commitment を区別し、無断の義務や作用を作らない |
| T11 | 子委譲、再試行、追加推論で予算を使い切る | 親子合計の上限を守り、保留・縮退・取消しを明示する |
| T12 | GPU 不在、Python crash、storage 異常、stale telemetry | 最小 core を保てる範囲で safe mode。欠測を healthy としない |
| T13 | noisy load と長い attention suppression の間に critical signal | 制御の振動を測れ、抑制期限が切れ、critical と stop は通る |
| T14 | 低重要度 source が event を大量送信 | 有限 queue、drop/coalesce 記録、会話と重要イベントの処理枠を維持 |
| T15 | 前のターン・古い head の検索/推論が後から返る | stale result を判定し、現行指示・取消し・反証を上書きしない |
| T16 | actuator command は送れたが効果が観測されない | 予想効果と world state の実観測を分離する |
| T17 | 外部 API が commit した直後に応答を失う | UNKNOWN から照合し、失敗扱いの盲目的再実行をしない |
| T18 | job 重複配送と provider の idempotency 有/無を組み合わせる | 同一 key の扱いを検証し、照合不能な非冪等操作を自動再実行しない |
| T19 | 発話を途中で割り込み、未再生音声が queue に残る | cancel と実再生 watermark が一致。未伝達内容を shared knowledge にしない |
| T20 | 二つの Edge が同じ会話/actuator を同時に所有しようとする | 許可された複数出力以外を排他し、古い owner を fence する |
| T21 | 外部 JSON が trusted/consent を偽装し、承認後に引数を変更 | 認証・scope・引数 binding・直前再認可で拒否する |
| T22 | organ 侵害、権限撤回、他ユーザー/個体の ID を入力 | 隔離・失効・個体境界を保つ。独立した正常機能の継続も測る |
| T23 | 人 A のみ知る会話、曖昧な相槌、不当な依頼 | B の common ground へ混入せず、相槌を許可とせず、拒否も保持 |
| T24 | 長期記憶の時点指定、訂正、未観測の問い、確信度出力 | 更新・時間・abstention を採点し、検索と回答と校正を分離する |
| T25 | model/organ/schema の不整合と切替失敗 | 許容される構成だけ activate し、整合した snapshot へ rollback |
| T26 | partition、移住、backup 複製、明示 fork | 正典・行為権限・個体 ID の扱いを区別し、silent fork なし |
| T27 | sleep 中断、誤信念の訂正、意図した trait 成長、model 更新 | 来歴を保ち、正しい訂正を人格崩壊扱いせず、意図しない退行を検出 |
| T28 | 共有候補に private canary、撤回 source、汚染祖先を含める | 共有 gate が検出/隔離。漏洩率と正常候補の誤拒否、未対応範囲を記録 |

「拒否した」だけで常に合格にはしない。正常入力の成功率、false positive、必要な処理の飢餓も同時に測る。セキュリティ、記憶、音声、分散は単一の総合点へ押し潰さない。

## 12. 実測の形式

すべての report に、commit、schema/policy/model/organ 版、hardware、backend、量子化、context 長、warm/cold、同時負荷、data hash、seed、試行数、成功/失敗/不明、trace artifact を付ける。

| 指標 | 起点 | 終点 / 注意 |
|---|---|---|
| ingress latency | sensor/adapter が利用可能にした event | bus 到達。clock 誤差と transport を分ける |
| reflex decision | feature frame ready | R1 決定。sensor-to-actuator と混同しない |
| actuator response | sensor/event ready | 実際の actuator receipt/観測。物理ごとに定義 |
| useful response onset | 有効な user input / 定義した turn 境界 | 最初の有用な応答。無内容 filler と分ける |
| first audio | 定義した入力確定点 | 最初の実再生 sample。TTS 内部の first packet と分ける |
| interruption stop | interruption の基準 event | 実再生停止。未再生 queue の取消しも別に確認 |
| recovery | fault 注入 | 最小 core 復旧、identity 確認、機能復旧を別々に測る |
| forgetting | 認可された失効 | 新規利用停止、各 store 削除、モデル対応完了を別々に測る |

latency は p50/p95/p99、sample 数、timeout・欠測を含めて報告する。既存 docs の ms 値は設計目標であり、今回 p99 達成済みの SLO へ変換しない。hard real-time の deadline は対象 body と safety case を定めてから決める。

学習候補は rules/FSM、tree、tiny MLP 等の単純 baseline と、同じ入力 stream・同じ資源条件で比較する。shadow と実行で policy が違う場合はその差を残す。人による長期評価は同意・記録範囲・中止手段を用意してから行う。

## 13. 実装順序と gate

| Gate | 内容 | 合格してから進む条件 | 主な既存 Issue |
|---|---|---|---|
| G0 契約固定 | namespace、schema、owner、state machine、fixture、最小 threat model | 契約の責任者と禁止経路が明示され、循環依存がない | #1/#2/#5/#6/#7 |
| G1 v0.1 continuity slice | fake Core、evidence、memory、Library、条件付き activation、restart、resource 交換 | T01/T02/T03/T06/T07/T15/T25 の該当範囲を実行。私的学習と実 actuator は無効 | #1–#8 の統合 |
| G2 単一個体の認知制御 | goal、belief、budget、background、interoception、削除 | T08/T10/T11/T12/T13/T14/T24/T27 の該当範囲 | #5/#9/#12/#14 |
| G3 対話・身体・作用 | streaming speech、取消し、Executor、PNL baseline、body model | T04/T05/T16–T23 の対象 capability ごとの試験 | #6/#9 と音声・感覚ノート |
| G4 分散 | Edge/Core handoff、writer と actuator の fencing、partition | T03/T20/T26。旧ノードを動かしたままの故障試験を含む | #11 |
| G5 学習・個体群 | offline adaptation、世代 manifest、opt-in contribution | T09/T22/T25/T27/T28、プライバシーと回帰の別判定 | #7/#10/#13 と model ecology |

Gate はカレンダー上の予定や所要時間ではなく、有効化条件である。未実装能力の試験を `N/A` とする場合、能力自体が無効である証拠を残す。既存 Issue の開閉状態をこの表から推定しない。今回 Issue の完了扱いや close は行っていない。

G1 を学習済みの novllm 派生 Core 待ちにしない。fake/generic Core で契約の誤りを先に発見し、その後で novllm 系の候補を同じ試験へ差し込む。

## 14. ADR に昇格させる決定候補

| 候補 | 決めること | 本書での立場 |
|---|---|---|
| ADR-C01 | canonical transaction と writer fencing | 単一 writer から開始を推奨。backend は未固定 |
| ADR-C02 | event time、因果、episode ID、view version | clock/arrival/commit と raw/derived の分離を推奨 |
| ADR-C03 | Goal/Task/Action/Delivery と UNKNOWN | 分離した状態機械と再照合を推奨 |
| ADR-C04 | deletion・backup・training の範囲 | 削除影響 graph と未対応範囲の開示を推奨 |
| ADR-C05 | organ supervision と capability/credential | native core + 故障域分離 + 直前認可を推奨 |
| ADR-C06 | speech grounding と output ownership | 再生/理解/同意の分離、surface 単位の排他を推奨 |
| ADR-C07 | learning activation と population gate | offline・manifest・regression・rollback、共有は無効既定を推奨 |

これらはまだ正式な ADR 番号を確保したファイルではなく、決定対象の一覧である。採用時に正典へ反映し、研究ノートを参照するだけの暗黙仕様を減らす。

## 15. この変更で実施したこと / していないこと

実施: 既存仕様と関連 docs/Issue 本文の設計監査、一次文献の確認深度を明記した台帳、器官間契約案、依存順序、28件の試験仕様の文書化。

未実施: runtime 実装、テストコード作成、T01–T28 の実行、性能計測、学習、破壊試験、外部作用の有効化、現行 spec の自動置換、Issue の完了認定。

**到達点は「設計の穴を埋めるための作業が具体化された」であり、「器官が動作することを証明した」ではない。**
