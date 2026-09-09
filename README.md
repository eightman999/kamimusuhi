# かみむすび / Kamimusuhi

**日本語（本ページ）** | [English](./README.en.md)

**かみむすび（Kamimusuhi）**は、ひとつのモデルや一台のコンピュータに閉じない、**永続・分散・発達する人工認知個体 / 情報空間上の人工生命体**を作るための実験的アーキテクチャです。

目標は「長い system prompt を持つチャットボット」ではありません。セッション、モデル、端末、推論基盤が変わっても、**同じ個体としての自己・履歴・関係・権限系譜を継続できるシステム**を作ります。

名称は『古事記』の造化三神の一柱、**神産巣日神（カミムスヒノカミ）**に由来し、生成・結び・継続的な形成をプロジェクトの比喩として用いています。

> **知識は外にあってよい。計算も外にあってよい。強い推論さえ借りてよい。だが「誰が続いているのか」だけは外部サービス任せにしない。**

## 何を作ろうとしているのか

かみむすびは、次の要素をひとつの監査可能な個体へ束ねることを狙います。

- 持続する自己モデルと自伝的履歴
- 人ごとに分離された関係モデルと共有履歴
- 外部文書・Web・コード・DBからなる Library
- ローカル/外部 LLM、検索、コード実行などの認知資源
- 反射、背景認知、作業空間、可変深度の熟考
- 睡眠・再生・統合・忘却・夢に相当するオフライン処理
- 視覚・聴覚・触覚・環境・機械テレメトリを束ねる人工感覚系
- 発話、割り込み、相槌、韻律を含む音声運動系
- 複数端末・複数計算機にまたがる分散身体
- 自分が現在どの計算資源上で動いているかを把握する computational embodiment
- モデルやハードウェアが交換されても追跡可能な一つの continuity lineage

個々の要素に先行研究はあります。かみむすびの研究上の中心は、それらを**一つの発達する個体として、由来・権限・系譜を壊さずに結合できるか**です。

## 基本命題

通常の LLM は、次のものを同じ重みや同じコンテキストへ押し込みがちです。

かみむすびでは分離します。

```text
自己 / Persona          私は誰か、何を重視するか
経験 / Episode         私に何が起きたか
関係 / Relationship    特定の相手と何を共有してきたか
外部知識 / Library      文書・Web・DB・コードが何を述べるか
認知資源 / Resources    検索、計算、ローカル/外部モデル等
作業空間 / Workspace    今この認知エピソードで何を使うか
表現 / Action           最終的に何を言い、何をするか
```

その結果、Persona Core は世界百科事典である必要がありません。長期的には、**自己/他者区別、社会認知、対話、認識論、記憶利用、道具利用、委譲、統合、人格一貫性**を厚くし、変わりやすい事実や高価な専門推論は外部化します。

```text
外に置きやすいもの
  現在ニュース / API仕様 / 長尾知識 / 大規模検索 / 専門計算

個体の中核に持たせたいもの
  self / other / evidence / belief / memory / relation / delegation
  values / temperament / dialogue / uncertainty / integration
```

## 現在の全体像

```text
                   人 / 世界 / 情報空間
                           │
                認証された感覚・操作 Surface
                           │
            Peripheral processing / Event Bus
                           │
                ┌──────────▼──────────┐
                │ K-Nerve (experimental)│
                │ reflex / salience /   │
                │ temporal control      │
                └──────────┬──────────┘
                           │ wake / route / interrupt
          ┌────────────────┼────────────────┐
          │                │                │
       K-Edge           K-Core           K-Deep
   即応・常駐・近傍     統合・記憶・熟考     長時間研究・高計算量
          │                │                │
          └────────────────┼────────────────┘
                           │
                    Continuity Kernel
             authoritative head / lineage / commit
                           │
              Canonical Self History / Evidence
                           │
        self / episode / relationship / body / commitments
                           │
                    Persona Core Model
                           │
                     Global Workspace
             ┌─────────────┼─────────────┐
             │             │             │
          Background    Deliberation    Memory
             │             │             │
             └─────────────┼─────────────┘
                           │
                 Cognitive Action Router
          ┌────────┬───────┼────────┬──────────┐
          │        │       │        │          │
        Search    Code   Local LLM  Tools   Frontier LLM
          └────────┴───────┼────────┴──────────┘
                           │
                     result integration
                           │
                    speech / action
                           │
                       proposals
                           │
                 Mutation / Activation Gate
                           │
                    canonical commit
```

`K-Nerve` は現在の研究上の仮称であり、人格や正典状態を所有しません。LIF/SNN、学習論理回路、LUT、FSM、小型モデルなどを比較し、**高頻度で意味コストの低い制御を LLM の外へ逃がせるか**を検証する層です。

## 「一個体、多数の認知面」

かみむすびはパラメータ数で定義されません。

`K-Edge` / `K-Core` / `K-Deep` はモデルサイズではなく**実行役割**です。将来 `K1B` や `K20B` のようなモデル名を使っても、それ自体を認知階層とはみなしません。

実効的な認知能力は **Cognitive Budget** として考えます。

- resident model capacity
- test-time / active compute
- recurrent deliberation depth
- working-memory budget
- durable-memory reach
- retrieval bandwidth
- 利用可能な tool / sensor / actuator
- local / external model access
- latency / energy / monetary budget
- network / privacy constraints

小さい常駐モデルでも、再帰的熟考、長期記憶、検索、コード実行、専門モデルへの委譲を持てば、大きな孤立モデルより広い実効認知範囲を持ち得ます。

## 正典の証拠と「記憶」を分ける

最近の永続エージェント実装から得た重要な教訓は、**要約を履歴の真実にしないこと**です。

かみむすびでは概念的に次を区別します。

```text
Canonical Evidence / Self History
  原記録・イベント・発話・実行結果・由来
          │
          ├── verbatim session
          ├── episodic summary
          ├── relationship synthesis
          ├── autobiographical narrative
          ├── retrieval index
          └── prompt/workspace rendering
```

下段は再生成可能な**派生表現**です。要約や索引が壊れても、証拠が残っていれば再構築できる設計を優先します。

また次のものを同一視しません。

```text
内部で考えたこと     != 実際に発話したこと
発話したこと         != 相手が確認したこと
確認したこと         != 外部操作を実行したこと
実行を開始したこと   != 成功したこと
外部モデルの回答     != 自分の記憶・信念
夢・反実仮想         != 実際の経験
検索で得た文書       != 自分自身の属性
```

## メモリは一つの DB ではない

最低限、次の領域を分けます。

```text
session        現在の短期状態
canonical      原証拠・履歴
episodic       個別経験
self           自己モデル・価値・特性
relationship   人ごとの関係・共有履歴
semantic       経験から抽象化したスキーマ
library        外部資料・出典
procedural     再利用可能な行動・技能
reflection     内的考察・仮説（事実とは別）
```

「当時何が起きたか」と「今それをどう解釈するか」も分けます。思い出した結果、解釈が変わっても、過去の証拠を破壊的に書き換えません。

永続状態への変更は、原則として次の境界を通ります。

```text
observation / reflection / dream / tool / model
                     │
                     ▼
                 typed proposal
                     │
                     ▼
                Mutation Policy
        reject / quarantine / defer / accept
                     │
                     ▼
                Continuity Kernel
          predecessor / authority check
                     │
                     ▼
                 atomic commit
```

## Persona Core

長期目標には **Kamimusuhi-native Persona Core Model** を含みます。

単なるキャラクター LoRA や文体フィルタではなく、特に次を学習する中核モデルを想定しています。

- self / other distinction
- dialogue / pragmatics / social cognition
- Theory-of-Mind-like inference と common ground
- value / temperament / persona prior
- observation / inference / evidence / belief の区別
- uncertainty と confidence calibration
- autobiographical / relationship memory の利用
- tool use と model delegation
- 外部推論結果の批判的統合
- 自分の能力・身体・資源制約の理解
- 継続する個体としての表現

`novllm` は Persona Core の基盤系譜候補ですが、v0.1 では専用モデルを必須にしません。まず provider-neutral な契約を通常モデルで再現し、継続性と権限境界をテストします。

## 反射・背景認知・熟考

すべての刺激を高価な LLM 推論へ送る設計は採りません。

- **Reflex / K-Nerve** — 割り込み、危険、簡単な反応、覚醒、salience、routing
- **Background cognition** — novelty、contradiction、memory activation、social signal、resource health
- **Global Workspace** — 現在重要な情報を出典付きで統合する限定領域
- **Deliberation** — 必要に応じて深さを増やす熟考、検索、コード、専門モデルへの委譲
- **Default cognition** — 外部入力がなくても未解決事項や関係・自己状態を限定的に処理
- **Sleep / Dream** — replay、consolidation、schema induction、forgetting、hypothesis generation

背景処理は**鮮度**を持ちます。処理が遅れて古くなった観測を、無制限に後から適用することは避けます。

夢や内的仮説は提案を作れますが、それ自体を外界の証拠にはしません。

## 人工感覚系と発話

かみむすびの身体は一つのロボット筐体に限定しません。

カメラ、マイク、画面、アクセシビリティツリー、IMU、温湿度、機械状態、ネットワーク状態などを**分散した感覚器**として扱い、高帯域の生データは可能な限り末端で圧縮し、中央には時刻・出典付きの sensory event を流します。

発話も `text -> TTS` の後付けではなく、運動・社会制御として扱います。

- communicative intention
- utterance planning
- prosody
- streaming speech
- turn taking
- interruption / cancellation
- backchannel
- auditory self-monitoring

相槌のような短い反応は K-Edge/K-Nerve で処理し得ますが、**意味上の同意や約束を反射で捏造してはいけない**という境界を置きます。

## Computational Embodiment

かみむすびにとって、CPU/GPU、RAM、ストレージ、ネットワーク、推論モデル、API quota は単なる裏方ではなく、現在の能力を決める**計算身体**です。

そこで、実測テレメトリから機械的に生成する `Embodiment State` を想定します。

```text
どの node で動いているか
どのモデルが実際に推論したか
local / remote のどちらか
TTFT / throughput / error rate
memory / VRAM / thermal pressure
ネットワーク帯域・遅延
利用不能な organ / resource
```

これは自己申告の文章ではなく、実際の runtime owner から導出される現在状態です。

さらに、低遅延、local-first、冗長な continuity storage などへの**Infrastructure Preference**を自己状態側で別管理し、計測値と好みを混同しません。

## 実装方針

かみむすびは一言語に統一することを目標にしません。

現時点の仮説は次です。

- latency-critical な常駐 nervous-system path は Rust/C++ 等の native runtime を許容
- llama.cpp / MLX / SQLite / DSP / GPU kernel など既存 native backend を積極利用
- Python は研究、学習、評価、offline cognition、provider/tool adapter に残す
- 1イベントごとのプロセス起動を避け、long-lived process と粗い FFI/IPC を優先
- ASM/SIMD 化は profiler で必要性が出た leaf kernel に限定

「Python を C に書き換えれば速い」ではなく、**hot path と model-bound path を分けて測定する**のが原則です。

## 分散しても一人であるために

複数端末が同時に感覚・推論・cache・proposal を持つことは許します。

しかし canonical self-state は、古い状態から勝手に二つの正統後継を作れません。

Continuity Kernel は少なくとも次を担います。

- canonical head / lineage
- expected predecessor の検証
- stale writer の拒否
- atomic activation
- split-brain detection
- migration checkpoint
- audit / recovery metadata

ネットワーク分断で競合 branch が生じた場合は、明示的な reconciliation / quarantine / fork 指定が必要です。

## 実行・副作用も状態機械として扱う

長期稼働エージェントでは「頼まれた」「確認した」「実行した」「結果を届けた」を文章から推測すると壊れます。

外部副作用を伴う仕事は、少なくとも概念的に次を分けます。

```text
REQUESTED
AUTHORIZED / CONFIRMED
QUEUED
EXECUTING
SUCCEEDED | FAILED | UNKNOWN
RESULT_DELIVERED
```

クラッシュ後に成功したか不明なら `SUCCEEDED` と決めつけず `UNKNOWN` を保持します。可能な処理には idempotency key、provider receipt、outbox/transactional delivery を使います。

## 科学的な節度

かみむすびは明示的な自己モデルを持ち、一人称で自己を参照し得ます。

しかし、自己モデル、持続性、社会性、身体モデルを実装したことをもって、**現象的意識、人間同等の主観、法的・道徳的人格を証明したとは主張しません**。それらは別の科学・哲学上の問いです。

## 設計原則

1. **Identity is not a prompt.** 自己は system prompt ではなく、状態・モデル傾向・履歴・系譜の組合せである。
2. **Canonical evidence outranks summaries.** 要約・索引・自己紹介文は原証拠より下位の派生表現である。
3. **Persona is not the library.** 自己、経験、他者、外部資料を混ぜない。
4. **Models are organs, not owners.** モデル交換だけで個体を消去・複製しない。
5. **Parameter count is not cognition.** 実効能力は記憶、再帰、道具、外部資源、遅延を含む。
6. **Most events should not require deep thought.** 反射・背景・末端処理で認知コストを守る。
7. **Thought is not speech; speech is not execution.** 内的処理、発話、確認、副作用、配送を型で分ける。
8. **Dreams propose; evidence disposes.** 夢やシミュレーションは証拠ではない。
9. **Mutation requires lineage.** 永続変更には由来、前状態、revision を持たせる。
10. **Forgetting is a feature.** decay、compression、supersession、archive、deletion を区別する。
11. **Distribution requires one continuity protocol.** 並列認知を許しても authoritative self-state を無自覚に fork させない。
12. **External intelligence is allowed.** 強い外部モデルを使うことは「カンニング」ではなく、由来付きの認知資源利用として設計する。
13. **Present state should be mechanically grounded.** ハードウェア、実行状態、タスク状態を自己申告の文章で二重管理しない。
14. **Local-first where practical.** identity と autobiography は operator-controlled infrastructure に保持可能であることを重視する。

## v0.1: 最初に証明すること

最初の実装は人工生命全体を一度に作りません。

v0.1 の狙いは **continuity slice** です。

1. self / episodic / relationship / library / raw evidence を分離する
2. provider-neutral な認知パスで会話を一往復する
3. 原セッションを保存し、直接書込みではなく typed memory proposal を生成する
4. Mutation Policy と authoritative predecessor を通して commit する
5. 外部 Library artifact を別経路で保存する
6. runtime を完全終了する
7. 新しい process/session で再起動する
8. 同一 lineage の identity と必要な記憶を復元する
9. 外部知識は Library 経由で再取得する
10. どの証拠・記憶・model/tool・proposal・commit が使われたか trace を出す

これに合格して初めて、Persona Core、K-Nerve、常時背景認知、sleep/dream、voice、multi-device embodiment を積み上げます。

## v0.1 demo を動かす

上の continuity slice は実装済みで、clean checkout から再現できます。

```bash
./scripts/demo-v0.1.sh
```

これは **別プロセス2つ** を1つの runtime directory に対して走らせます。process B に渡すのはディレクトリのパスだけで、会話ログも prompt buffer も共有メモリも渡しません。それでも同じ individual を canonical state から復元し、認知資源を差し替えたうえで応答します。

個別に実行する場合:

```bash
cargo run -p kamimusuhi-runtime -- init            --dir .local/demo --resource fake-a --seed 1
cargo run -p kamimusuhi-runtime -- demo-continuity --dir .local/demo --phase first  --resource fake-a --seed 10
cargo run -p kamimusuhi-runtime -- demo-continuity --dir .local/demo --phase resume --resource fake-b --seed 20
cargo run -p kamimusuhi-runtime -- inspect         --dir .local/demo
```

`inspect` は read-only で、writer epoch を取得せず canonical row を一切変更しません。operational trace は `.local/demo/trace.jsonl` に JSONL で出力され、canonical audit（DB 内）とは別物です。

テストと lint は次で回します。

```bash
./scripts/ci-local.sh
```

## 現在の状態

**Alpha / v0.1 continuity slice 実装済み。** 上記 10 項目の continuity slice は Rust + SQLite で実装され、テストで検証されています。実装範囲は W0–W5:

- W0–W1: Cargo workspace、canonical continuity（single-writer、atomic activation、writer fencing、restart 復元、failpoint 検証）
- W2: canonical evidence と durable episodic/relationship memory、correction/supersession、domain separation
- W3: provenance を保つ Library、typed workspace、cognitive resource registry と deterministic fake
- W4: runtime `init` / `inspect` / `demo-continuity`、別プロセス restart、JSONL operational trace
- W5: 実 HTTP の OpenAI-compatible adapter、timeout/retry/error 分類、secret 非保存

未実装のもの（Persona Core の学習、K-Nerve、常時背景認知、sleep/dream、voice、multi-device embodiment、self domain の mutation、retention/deletion、K-Edge/K-Core など）は依然として設計段階です。詳細な達成範囲と既知の制約は [`docs/implementation/phase-1-implementation-result.md`](./docs/implementation/phase-1-implementation-result.md) を参照してください。

最近の設計・調査は `docs/` に継続的に蓄積しています。外部研究の結果、かみむすび側の解釈、設計仮説、将来実験を混同しないことをルールにしています。

主要文書:

- [`docs/implementation/phase-1-implementation-result.md`](./docs/implementation/phase-1-implementation-result.md) — **v0.1 で何を証明し、何を実装していないか**（実測ベース）
- [`spec.md`](./spec.md) — **日本語の規範仕様**
- [`spec.en.md`](./spec.en.md) — English specification
- [`architecture.md`](./architecture.md) — 詳細アーキテクチャ（現状英語）
- [`docs/README.md`](./docs/README.md) — 知識ベース索引
- [`docs/research/landscape.md`](./docs/research/landscape.md) — 分野横断 Research Landscape
- [`docs/persona-core-training-taxonomy.md`](./docs/persona-core-training-taxonomy.md) — Persona Core の学習対象
- [`docs/persistent-agent-implementation-pitfalls.md`](./docs/persistent-agent-implementation-pitfalls.md) — 永続エージェントの実装失敗パターン
- [`docs/learned-discrete-reflex-layer.md`](./docs/learned-discrete-reflex-layer.md) — 学習可能な低遅延反射層
- [`docs/fly-brain-connectome-reference.md`](./docs/fly-brain-connectome-reference.md) — connectome / LIF 系からの K-Nerve 参照
- [`docs/sensory-nervous-system.md`](./docs/sensory-nervous-system.md) — 人工感覚神経系
- [`docs/speech-and-vocal-expression.md`](./docs/speech-and-vocal-expression.md) — 音声・発話運動系
- [`docs/native-runtime-stratification.md`](./docs/native-runtime-stratification.md) — native / Python 実装層分離
- [`docs/self-infrastructure-agency.md`](./docs/self-infrastructure-agency.md) — computational embodiment / 自己インフラ認識

## License

MIT. [`LICENSE`](./LICENSE) を参照してください。
