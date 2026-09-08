# Kamimusuhi 器官・機構設計監査 — 2026-09-08

Status: **design audit / proposals, not implementation evidence**  
監査対象: `eightman999/kamimusuhi`  
固定した基準 commit: [`5990e93a8a0a879c055192d0c32ca670fe73253f`](https://github.com/eightman999/kamimusuhi/tree/5990e93a8a0a879c055192d0c32ca670fe73253f)  
関連: [実装契約と検証計画](../organ-contracts-and-implementation-plan.md) / [文献21本の確認台帳](../research/2026-09-08-organ-audit-literature.md)

## 0. 結論

**Kamimusuhi は「目・耳・記憶・人格という器官の名前が足りない」段階ではない。足りないのは、それらが一個体として矛盾なく動くための、権限・時間・状態・資源・停止・復旧・学習の接続契約である。**

特に設計を厚くすべきなのは、次の機能である。

1. **動機・目標・約束の仲裁**: 何に関心を持つかだけでなく、何を実行義務とし、何を諦めるか。
2. **信念維持と世界状態**: 記憶を取り出すだけでなく、何を今も信じてよいか、何が未確定か。
3. **時間と経験の組立て**: 非同期の出来事をどの順序・粒度・根拠で一つの経験にするか。
4. **忘却・修復・免疫**: 誤記憶、権限を持たない入力、削除済み情報がどこまで伝播するか。
5. **全身の実行・停止・復旧**: 複数器官が同時に動いても、二重実行・二重人格・自己隠蔽を起こさないこと。

これは既存構想を否定する結論ではない。PNL、音声、記憶分離、個体系譜、外部認知資源との境界はすでにかなり詳しい。今回必要なのは、既存の良い原則を**状態所有者、入出力、遷移、障害時動作、反証可能な試験**まで降ろすことである。

## 1. 監査方法と確認範囲

### 1.1 判定基準

規範上の優先順位は `spec.md`、`architecture.md`、関連追補、研究・設計ノートの位置付けを尊重する。研究ノートの候補を勝手に正式採用済みにしない。

各領域を、(a)役割、(b)状態所有者、(c)入力と出力、(d)権限、(e)時間・順序、(f)予算、(g)終了・取消し、(h)故障・回復、(i)学習・更新、(j)試験、の十観点で確認した。

本文の「不足」は、次の意味を分けている。

- **輪郭あり**: 概念・目的・候補は書かれているが、実装選択が複数残る。
- **部分契約あり**: MUST/SHOULD、状態名、入出力例、受入条件の一部が存在する。
- **接続未確定**: 個々の設計はあるが、他器官との所有権・順序・失効条件が閉じていない。
- **実行保証未確認**: 実装・結線・実測による成立を、この監査では確認できない。

「記述が見つからない」ことを「プロジェクト全体で誰も考えていない」と同一視しない。特に goals、forgetting、watchdog、leases、outbox、full-duplex、anti-self-blinding は既存資料に記載済みである。

### 1.2 ソース台帳

以下のパスは、原則として上記固定 commit の内容を指す。相対リンクは閲覧用であり、将来の改訂でリンク先が進んでも本監査の基準は変わらない。

| ID | 資料 | 今回の確認範囲 |
|---|---|---|
| S01 | [spec.md](../../spec.md) | 全文。INV、FR、NFR、v0.1 受入条件、未解決事項 |
| S02 | [architecture.md](../../architecture.md) | 全文。canonical owner、workspace、各認知モード、記憶、分散、進化順序 |
| S03 | [model-ecology-architecture.md](../../model-ecology-architecture.md) | 全文。器官分類、個体群、三つの学習時間尺度、私的記憶の非共有 |
| S04 | [persistent-agent-implementation-pitfalls.md](../persistent-agent-implementation-pitfalls.md) | P01–P50 と設計含意。末尾の全実験記述の逐語監査ではない |
| S05 | [sensory-nervous-system.md](../sensory-nervous-system.md) | 全文。感覚、時刻・空間校正、融合、反射 |
| S06 | [speech-and-vocal-expression.md](../speech-and-vocal-expression.md) | 全文。意図、発話、相槌、割込み、実再生範囲、評価 |
| S07 | [peripheral-neural-layer.md](../peripheral-neural-layer.md) | 全文。R0/R1、局所状態、学習候補、shadow、fallback |
| S08 | [FIO 神経関数分析](../research/fio-neural-function-growth-2026-09-08.md) | Part II の設計分析と原報告の該当箇所。外部実測の追試ではない |
| S09 | [persona-core-training-taxonomy.md](../persona-core-training-taxonomy.md) | 全文。自己他者、社会認知、認識論、反迎合、学習・評価候補 |
| S10 | [runtime-language-and-hardware-compatibility.md](../runtime-language-and-hardware-compatibility.md) | 言語、起動、互換性、可変技能、安全、検証方針の各節 |
| S11 | [self-infrastructure-agency.md](../self-infrastructure-agency.md) | 計測・身体状態・選好・改善提案・権限・実験の各節 |
| S12 | [latency-architecture.md](../latency-architecture.md) | 全文。warm path、cache、先行検索、段階応答、測定条件 |
| S13 | [research/landscape.md](../research/landscape.md) | 冒頭の比較表、認知アーキテクチャ、記憶の該当部分。全項目の文献再検証ではない |
| S14 | [docs/README.md](../README.md) | 全文。文書分類、正典、研究から設計への位置付け |
| S15 | [GitHub Issues](https://github.com/eightman999/kamimusuhi/issues) | 検索で返った #1–#14 の本文と受入条件。返却 state が null のため開閉状態は判定しない |

リポジトリの再帰 tree は `truncated: false` で確認した。この snapshot は Markdown と LICENSE を中心とし、実装コード・実行可能なテスト・build 定義は tree 上に確認できない。これは他リポジトリや手元に実装が存在しないという断定ではない。

EN 版との逐語的対照、全 daily survey、全 watchlist、外部プロジェクトの実装全体、実行環境への侵入試験、モデルの学習実験は対象外。**この監査自体はランタイム試験を実施していない。**

## 2. 器官別カバレッジ — 26領域

ここでの器官は機能上の区分であり、26個のモデルや26個の常駐 process を要求するものではない。

| 領域 | 既存設計 | まだ閉じていない点 | 指摘 |
|---|---|---|---|
| 01 継続核・個体識別 | S01/S02/S15、単一 head と guarded activation | transaction 範囲、失効した writer、復旧時の一意性 | A01 |
| 02 正典 evidence log | S01/S04、原記録と派生物を分離 | 耐久化の確定点、保存できない場合の応答 | A02/A04 |
| 03 時間・出来事の順序 | S05、clock correction と timestamp | 発生時刻と受信時刻、誤差、因果、再送、clock jump | A02 |
| 04 世界状態・予測 | S02/S05/S09、予測誤差と身体モデル | 現在の事実・仮説・予想効果を更新する owner | A08 |
| 05 エピソード記憶 | S01/S04、rolling/provisional capture | session と出来事の境界、再分節と参照継承 | A09 |
| 06 意味記憶・信念維持 | S01/S02、出典、反証、supersession | 依存グラフ、根拠撤回、循環した証拠の検出 | A03 |
| 07 忘却・削除 | S01/S04、decay/delete/supersede を区別 | 要約、索引、backup、学習物までの失効伝播 | A04 |
| 08 作業記憶・workspace | S02/S15、予算、由来、選択理由 | snapshot 整合性、遅着結果、pin と eviction の優先 | A07/A09 |
| 09 注意・顕著性 | S07/S08/S15、抑制・上位変調・反例保護 | source ごとの公平性、抑制の期限・最低観測率 | A07 |
| 10 動機・目標・約束 | S01/S02/S04/S11、values/goals/commitments | 目標間の競合、成立・放棄条件、権限と資源の仲裁 | A05 |
| 11 内受容・恒常性 | S11/S15 #14、閾値・減衰の要求あり | 実際の制御則、欠測、飽和、ヒステリシス | A06 |
| 12 メタ認知・計算予算 | S09/S12/S15 #12 | 追加計算の価値、総量上限、停止・保留、校正 | A07 |
| 13 計画・外部作用 | S01/S04、UNKNOWN、outbox、権限分離 | 状態機械の統合、相手 API に応じた照合手順 | A10 |
| 14 技能・手続き記憶 | S02/S03/S10、procedural/skill/adapter | 開始・終了条件、効果検証、失効と再学習 | A10/A16 |
| 15 感覚融合・身体図式 | S05、各感覚と融合の要件 | identity association、時空間不確実性、矛盾保持 | A08 |
| 16 PNL・運動反射 | S07/S08、R0、R1m/R1p の候補 | 実身体ごとの bounds、状態 reset、反射と随意動作の競合 | A08/A10/A13 |
| 17 発話・聴取・表現 | S06/S12、full-duplex と実再生範囲 | 再生と理解の区別、複数端末の発話権、速度と意味の両立 | A12 |
| 18 社会認知・共通知識 | S02/S09、self/other/common ground | 人ごとの公開範囲、伝達・理解・合意の証拠 | A15 |
| 19 Persona Core・個性 | S03/S09、novllm 派生、自己他者、反迎合 | 意図した成長と退行の判定、状態と重みの互換性 | A15/A16 |
| 20 睡眠・夢・統合 | S01/S02/S15 #10、仮説限定と rollback | 中断再開、固定読取点、再生バイアス、収支 | A16 |
| 21 器官 registry・健康 | S03/S10/S11、hash/capability/watchdog | installed/active/healthy の区別、障害伝播、依存関係 | A13 |
| 22 免疫・権限・隔離 | S01/S07/S10、auth/capability/sandbox | threat model、撤回、汚染の伝播先、監査 log の秘密 | A11 |
| 23 分散身体・移住 | S01/S02/S15 #11、head と handoff | 正典 writer と行為者の別管理、partition/復元/fork | A14 |
| 24 個体群・遺伝 | S03、個体別 lineage と opt-in | 私的適応の共有審査、撤回、世代 rollback | A17 |
| 25 計測・評価 | S01/S04/S06/S12、多数の候補指標 | 実行 fixture、negative test、trace coverage、合格証拠 | A18 |
| 26 実装基盤・配布 | S10、Rust/Python/optional acceleration | 最小起動契約、ABI・schema 互換、baseline 実機試験 | A13/A18 |

全領域で実装・結線・実測の成立は別途確認が必要。表の豊富さを完成率とみなさない。

## 3. 指摘と修正方針 — A01–A18

優先度の意味: **P0** は対象能力を有効化する前の阻害要因、**P1** は継続する認知・自律性の成立に必要、**P2** は高度化の比較研究。P0 が付いた将来能力まで v0.1 で全部実装する、という意味ではない。

### A01 — 継続核の transaction が「原則」から先へ進んでいない — P0

**既存:** S01/S02 と #2 は、predecessor check、atomic activation、lineage、stale node rejection をすでに要求する。

**不足:** どの記録が同一 transaction か、状態更新後・receipt 前の停止をどう扱うか、古い process が生き残ったときに何を拒否するかが未確定。

**方針:** 初期は単一 writer。canonical state、head、activation receipt、必要な outbox を同一 transaction に置く。競合 proposal は自動的に新 head へ付け替えず再検証する。writer の世代番号を持ち、古い世代の書込みを拒否する。ノード追加後の合意は既存実装を検討し、自作 consensus を最初の仕事にしない（R10）。受入: T01/T02/T03。

### A02 — 分散した感覚に「同じ現在」を与える契約が薄い — P0/P1

**既存:** S05 は clock correction、temporal alignment を列挙している。

**不足:** 発生時刻、受信時刻、log commit 順、因果順、許容誤差が同じ `timestamp` に押し込まれ得る。遅着・再送された音声を新しい経験として再記録する危険がある。

**方針:** source sequence、boot ID、event ID、causal parent、time domain、時刻誤差を分ける。local deadline は monotonic clock で扱う。canonical commit 順は出来事の発生順を捏造しない。原記録を保存できないときは「記憶した」と報告しない。raw 高頻度 stream 全件を canonical transaction に通す設計にはしない。受入: T04/T05。分節の研究参考は R04。

### A03 — 記憶はあるが、信念を訂正し続ける機構が弱い — P0/P1

**既存:** S01/S02/S04 は根拠・推論・反証・supersession を区別する。

**不足:** 根拠が訂正されたとき、どの要約・人物像・自己像・計画を失効させるかが明示されていない。同じ一件の会話から作った三つの要約を「三つの独立証拠」と誤認し得る。

**方針:** `Claim` と `Evidence` と `DerivedView` を別物にし、依存先・反証・適用時点・支持源の重複を追跡する。支持の撤回は再評価を起こすが、命題を自動的に偽と断定しない。自然言語矛盾の完全自動解決は前提にしない。確信度は校正対象であり権限ではない（R03 は系譜参考、R06/R08/R12）。受入: T06/T07/T24。

### A04 — 忘却の出口が全身へ届かない — P0 before private learning

**既存:** S01 の FR009 と S04 は、検索減衰・削除・訂正の違いを明記する。

**不足:** 削除対象から作った要約、人物モデル、embedding、prompt cache、backup、adapter、学習集合の扱いが一貫していない。

**方針:** 原 payload と最小限の履歴メタデータを分離し、削除対象の依存閉包を管理する。tombstone も秘密を含み得るので最小化する。restore は最新の削除・失効台帳を適用してから公開する。学習済み情報は削除済みと一括宣言せず、再学習待ち・対応不能など範囲を表示する（R18）。初期は私的会話の weight 学習を無効にして難所を分離する。受入: T08/T09/T28。

### A05 — 動機を実行可能な目標へ変換する仲裁器が薄い — P1

**既存:** interests、values、goals、commitments、default cognition、改善提案は S01/S02/S04/S11 にある。

**不足:** 興味、願望、ユーザー依頼、約束、メンテナンス、単なる検討候補の区別が runtime の遷移になっていない。並立した目標の優先順位・放棄・通知・期限が未確定。

**方針:** `GoalProposal → admitted/scheduled → active/blocked → satisfied/failed/abandoned` を持つ。Goal と実際の Task を分離する。目的達成の判定条件、必要権限、資源上限、見直し時点を記録する。関心が高いだけで義務や外部作用を生成しない。好奇心は新奇性最大化でなく、許可された sandbox・未知を減らす課題へ制限する。R01/R02/R20 を比較の軸とする。受入: T10/T11。

### A06 — 恒常性は信号名より、制御則と競合解決が必要 — P1

**既存:** #14 は目標範囲、閾値、減衰・回復を要求済み。S11 は telemetry、appraisal、preferences を分離済み。

**不足:** 実数値、欠測時の意味、持続時間、ヒステリシス、複数の制御器による二重調整、調整による振動の評価がない。

**方針:** resource owner の実測→単一の調整状態 owner→scheduler/organ の許可された操作、の経路を固定する。各変数に単位・出典・期限・上下限・fallback を持たせる。CPU 高負荷だから感情が悪化するという短絡を避ける。資源収支のために操作者の停止命令を拒否しない。R01/R11 は機能的参考。受入: T12/T13。

### A07 — 注意と計算予算に、全体としての上限が必要 — P1

**既存:** workspace domain budgets、deterministic router、freshness、反例サンプリング、attention modulation は S01/S08/S12 と #5/#12 にある。

**不足:** 子タスクがさらに委譲して上限を超える、background が foreground を圧迫する、抑制された感覚が永遠に戻らない、考える価値を計算するために考え続ける、といった閉路の管理。

**方針:** 親子の予約を含む episode 全体の予算、hard cap、timeout、cancel propagation を持つ。安全割込み・現在の会話・保守の枠を分け、queue 長を有限にする。低優先 event の coalesce/drop は理由を残す。抑制には期限・最低観測・重要信号 bypass を持たせる。学習化は決定的 baseline に勝ってから（R02/R06）。受入: T11/T13/T14/T15。

### A08 — 世界モデルと身体図式に「観測と推測の接着規則」がない — P1

**既存:** S05 に entity tracking、時空間校正、uncertainty fusion、cross-modal conflict がある。S11 は機械的な身体状態を扱う。

**不足:** 音声の人 A とカメラの人 A を結び付ける条件、機器移動・sensor replacement 後の校正失効、予想した動作効果と観測した効果の違い。

**方針:** 時間区間と不確実性を持つ entity/event hypothesis を保持し、曖昧なら別仮説のままにする。物理座標系と計算資源 topology は別モデル。命令送信で world state を成功状態に変えない。最初は構造化 state + 簡単な forward predictor で比較する（R04/R05）。受入: T05/T16/T17。

### A09 — 記憶の粒度と workspace の snapshot を固定する — P1

**既存:** S04 は rolling extraction、source range、hybrid retrieval、pre-compaction、derive version を詳細に扱う。

**不足:** session ID と経験単位、再分節時の ID、複数検索結果が違う版を読んだ場合の扱い、workspace から押し出せない最低状態が未確定。

**方針:** raw event の stable ID は変えず、episode segmentation は版付きの派生物にする。各 cognitive step が参照した view/version を記録する。現行の指示・権限・未完了の行為・反証・取消し状態を、単なる類似度で落とさない。検索、採用、最終回答を別々に評価する（R04/R08/R09）。受入: T04/T07/T15/T24。

### A10 — 「行動した」の意味を全器官で共通にする — P0 before side effects

**既存:** S01/S04 に confirmation、execution、UNKNOWN、reconciliation、outbox、delivery の区別がある。

**不足:** proposal→権限→予約→実行→外部効果照合→報告の一つの契約、取消しと external commit の race、反射・随意動作の同一 actuator 競合。

**方針:** typed Action と専用 Executor に統一する。効果は receipt または読戻しで確認し、確認不能なら UNKNOWN を維持する。再実行は provider の idempotency/照合能力に依存する。ローカル DB transaction だけで世界全体の exactly-once を主張しない。補償動作も新しい権限付き行為とする（R14）。受入: T17/T18/T19/T20。

### A11 — 免疫はコンテンツ判定ではなく、権限・隔離・失効の系統 — P0

**既存:** S01/S07/S10 に provenance、capability、R0、可変技能の隔離がある。

**不足:** 記憶・web・tool・画像内文字・別個体・侵害された organ が、どの境界を越え得るかを列挙した threat model、credential の扱い、信頼の撤回手順がない。

**方針:** 入力文に `trusted=true` と書かれても信用せず、認証済み ingress が権限メタデータを付与する。データに読者・用途・送信先制約を伝播し、実行直前に再確認する。悪意ある命令と誤った事実は別に扱う。汚染された派生物を隔離し正常処理の誤拒否も測る（R12/R13/R21）。R0 の安全・認証条件は上位の気分や学習から変更させない。受入: T06/T21/T22/T28。

### A12 — 声帯は詳しいが、伝わった意味の管理は不足 — P1 / P0 for commitments

**既存:** S06 は実再生範囲、割込み、AEC、自己音声、turn state、vocal reflex をすでに設計している。

**不足:** `played ≤ planned` だけでは「相手が聞いた・理解した・同意した」を表せない。また二つの端末が同時に喋ってよい条件がない。

**方針:** 発話 plan、再生 watermark、受信者ごとの到達可能性、理解の証拠、合意・許可を分離する。相槌は契約への同意ではない。発話権は会話/session/surface の資源として管理する。速さを上げる実験でも、意味保持・誤同意・許可捏造・拒否の回帰を必須にする（R15/R16/R17）。受入: T19/T20/T23。

### A13 — 心拍はあっても、全身の lifecycle がまだ図になっていない — P0/P1

**既存:** S10 は Rust runtime、heartbeat、watchdog、organ supervision、compatibility probe、fallback を予約している。

**不足:** boot 中に何を許可するか、半壊 DB・壊れた model・失効 key・不足 storage でどう起動するか、sleep と crash と shutdown の差、修理が無限再起動にならない条件。

**方針:** `BOOTSTRAP → VALIDATING → READ_ONLY_SAFE / ACTIVE → DRAINING → STOPPED` を定義し、障害では隔離・縮退へ移る。organ ごとに lifecycle、health、機能縮退、再試行上限を持つ。native binary の採用だけでは FFI crash を隔離できないため、危険な leaf は process 分離を検討する。R0 は深い推論が停止しても必要な安全停止を行える設計にする。受入: T01/T03/T12/T25。

### A14 — 単一の正典と、単一の実行者は別問題 — P0 before distributed actuation

**既存:** S01/S02/#11 は split-brain、head、移住、明示 fork を扱う。

**不足:** canonical write を拒否された古い Edge が喋る・機器を動かす場合、backup を別機器で起動する場合、完全切断時に許す動作範囲。

**方針:** canonical authority、会話発話権、actuator 所有権を別々に管理する。partition 中は限定された local safety と期限内 capability の範囲に縮退する。復帰して履歴を合わせることと、すでに二重実行した作用を取り消せることは別。restore の前に旧 writer を失効させる。新個体として起動するなら新 ID/root を作る（R10）。受入: T02/T03/T20/T26。

### A15 — 人格の安定、信念の訂正、関係の変化を分けて測る — P1

**既存:** S02/S09 は self/other/common ground、非迎合、文化差、長期関係、親密さの非強制を持つ。

**不足:** 自己一貫性スコアが、誤った信念を訂正しないことを高評価してしまう危険。本人の同一性確認や共有場面への情報持出しの基準も未確定。

**方針:** immutable な lineage、低速な dispositions、改訂可能な beliefs、可変な関係状態を別々に比較する。人別の可視情報を持ち、聞いていない人の知識へ混ぜない。warmth と agreement を別軸にする。感情推定は本人の申告ではなく仮説。相手の依存や guilt を効用最大化の指標にしない（R06/R09/R17）。受入: T07/T23/T27。

### A16 — 睡眠・発達の「成長」を採用可能な差分にする — P1/P2

**既存:** S01/S02/S03/#7/#10/#13 は replay、夢の仮説化、offline 学習、checkpoint、rollback、training provenance を持つ。

**不足:** 学習入力の読取点、どの重みと state/schema を一組として復元するか、評価データの混入、再生回数と証拠数の混同、技能の劣化検出。

**方針:** 学習前に dataset manifest、参照 head、同意、使用不可 source、評価 split を固定する。model と adapter と必要 schema の互換性を宣言し、shadow/canary 後に切り替える。反省・夢は新証拠でなく proposal。sleep の中断は未確定 proposal を残して安全に再開できるようにする（R07/R18）。受入: T07/T09/T25/T27。

### A17 — 群体・遺伝には忘却と隔離を貫通させる — P0 before sharing / P2 learning

**既存:** S03 に個体別 lineage、三時間尺度、opt-in、私的記憶を default 共有しない方針がある。

**不足:** 共有する skill/delta の private memorization 判定、同意撤回の波及、汚染した祖先 package を継承した子孫の発見と失効。

**方針:** 個体の記憶そのものではなく、共有許可された能力を非私的データ上で再現して候補化する。gradient や adapter を匿名化済みとはみなさない。世代・個体・データ・器官の lineage を結ぶ。削除要求に対して到達範囲と未対応範囲を報告する（R18/R19）。初期 population contribution は無効が既定。受入: T09/T22/T28。

### A18 — 最も大きい横断不足は、反証できる実行証拠 — P0 for claims

**既存:** 個別ノートと Issues に多数の受入条件と評価候補がある。

**不足:** fixture、failpoint、参照出力、実行環境、履歴、実測 receipt を結び付けた検証台帳がない。文書中の「ある」「できる」と、実際に結線・観測・soak された状態を混同しやすい。

**方針:** 要求→契約→試験→結果 artifact の追跡表を作り、`SPECIFIED / IMPLEMENTED / WIRED / OBSERVED / SOAKED` を別欄に持つ。no-op の器官にも health は成功ではなく未稼働と出す。評価は最終状態、出典、予算、反復信頼性、失敗時動作を含める（R08/R14/R16/R21）。本監査の T01–T28 は**試験仕様であり、合格結果ではない**。

## 4. 仕様間の緊張点と、矛盾ではないもの

| 論点 | 監査判断 | 解決方針 |
|---|---|---|
| 原記録を保存する vs 明示的削除 | 保持・失効・実削除の範囲が未確定 | payload、履歴、派生物、backup、学習物を分けた削除契約 |
| 実再生部分を common ground に反映 | 上限の考え方は正しいが、再生＝理解ではない | 受信者別の到達・理解・合意を別証拠にする |
| 一つの canonical head vs 複数端末の動作 | 同一性と行為排他は別問題 | writer、speech、actuator の権限を分離 |
| 暖かい即応 vs grounded factual answer | 先に認識や進捗を示すことは可能。未確認事実を断定してはならない | 安全な応答 prefix と根拠を要する semantic commit を区別 |
| 上位層が注意を抑制 vs 必要な異常観測 | S08 はすでに anti-self-blinding を提案済み | 期限・最低観測・critical bypass を試験可能な値へ落とす |
| local specialist が extension / organ の双方に出る | S03 の所属条件による分類であり、直ちに矛盾ではない | manifest に membership と権限を明示 |
| architecture の言語中立 vs Rust 推奨 | public contract の中立性と実装既定値の違いであり矛盾ではない | native runtime + 交換可能 worker の境界を維持 |
| v0.3 concept vs v0.1 implementation target | 文書の進化段階と実装目標の違い | release/capability の実装完了表示に使わない |
| 20–100ms、50–150ms、100–300ms 等 | 起点・終点・対象が異なる設計値。比較だけでは矛盾と判定できない | hardware、warm/cold、percentile、decision/audio/actuator を固定して計測 |

## 5. 生物学的な器官名へ翻訳すると

比喩として特に薄いのは、**目標を選ぶ基底核的機構、恒常性を調整する視床下部的機構、予測と運動誤差を処理する小脳的機構、記憶を組み直す海馬的機構、排泄・修復・免疫に相当する失効管理、循環・呼吸に相当する資源配分と lifecycle** である。

ただし生物の局在や一対一対応を証明した分類ではない。例えば「小脳モデルを実装する」のではなく、Action の予測・実測・誤差・修正をまず実装する。「感情ホルモンを増やす」のではなく、state owner と安定した制御則を定義する。人格 LLM が全器官を代行する構造にも、器官ごとに巨大 LLM を置く構造にも戻さない。

## 6. 先に作るもの / 後に回すもの

**最初の閉じた一周:** fake Persona Core、単一 writer、typed event、evidence、proposal、条件付き activation、読戻し、restart、外部 resource 交換。既存 #1/#2/#3/#4/#6/#7 を #8 で統合する。私的 weight 学習、実 actuator、population sharing は不要。

**次の一周:** goal ledger、予算仲裁、信念の失効、期限付き background、最小の interoception、実際に中断できる音声/タスク。#5/#9/#12/#14 と A03–A13 を接続する。

**その後:** #11 の分散、sensor fusion、body-specific PNL、#10/#13 の学習、個体群への継承。新規能力の有効化前に対応する P0 契約を満たす。

後回しにしてよいのは、全脳の忠実再現、巨大な latent world model、全器官の学習化、無制限 online self-modification、味覚・嗅覚の網羅、最初からの多地点 consensus、自律的な実世界での資源獲得である。器官の数ではなく、**一つの出来事が観測され、必要なら行動し、根拠付きで記憶され、誤りなら訂正され、再起動後もその履歴が分かること**を先に完成させる。

## 7. 監査の終了条件ではなく、実装の入口

今回の変更は設計・文献・試験仕様の追加であり、既存 spec を自動的に置換しない。採用する契約は別途 ADR または規範仕様へ昇格させる。

不足を埋めたと宣言するには、[実装計画](../organ-contracts-and-implementation-plan.md) の T01–T28 に実行 artifact を結び付ける必要がある。モデルが流暢に説明したこと、文書に状態名があること、外部論文が成功したことのいずれも、Kamimusuhi 自身の実装成立の代わりにはならない。
