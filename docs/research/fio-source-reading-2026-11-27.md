# FIO ソース読解と Kamimusuhi 各実験・runtime との対比（2026-11-27）

Status: **research note / first code-verified FIO reference（静的読解のみ・未実行）**

Primary source: user-supplied FIO runtime source distribution（2026-11-25 版、
Python 約 18.7 万行）。記憶・会話・資格情報は含まない空の配布物で、推論・記憶・
ツールの実装を読める。

Related Kamimusuhi docs:

- [`../fio-memory-architecture-measurements-2026-11-03.md`](../fio-memory-architecture-measurements-2026-11-03.md)
- [`../fio-system-reference-analysis.md`](../fio-system-reference-analysis.md)
- [`fio-system-architecture-2026-09-09.md`](./fio-system-architecture-2026-09-09.md)
- [`fio-neural-function-growth-2026-09-08.md`](./fio-neural-function-growth-2026-09-08.md)
- [`../fio-cross-review-and-runtime-countermeasures-2026-09-10.md`](../fio-cross-review-and-runtime-countermeasures-2026-09-10.md)
- [`../implementation/recall-v0.md`](../implementation/recall-v0.md)
- [`experiment-dispositions-2026-11-08.md`](./experiment-dispositions-2026-11-08.md)

これまでの FIO 参照文書は、FIO 側の PDF・聞き取り・クロスレビューを根拠にした
二次資料だった。本書は FIO の実コードに当てた最初の記録で、過去文書の記述を
照合し、Kamimusuhi の 23 研究ディレクトリと runtime 機構へ対応付ける。

**範囲**: 記憶・想起・自律・器官の設計対比に絞る。FIO の安全・権限・承認まわりの
評価は本書では扱わない。

Evidence vocabulary:

```text
CODE-VERIFIED      取りまとめ側が FIO 原文の該当行を読んで確認した
CODE-REPORTED      精読担当がコード本体を読んだ報告。取りまとめ側は未再確認
OUR INTERPRETATION Kamimusuhi 側の解釈
DESIGN HYPOTHESIS  Kamimusuhi の設計候補（非規範）
FUTURE EXPERIMENT  昇格前に検証が要るもの
```

限界を先に書く。FIO・Kamimusuhi ともコードは実行していない。FIO の実 DB と本番設定は
見ていない。Kamimusuhi の実験数値は既存 report の転記で、再計測ではない。
FIO のパスは配布物ルートからの相対表記（`fio/core/…`）。

---

## 1. 要約

- 両者は同じ機能を逆の方法で作っている。FIO は認知機能を「規則＋本体 LLM への
  問い合わせ」で直接実装して本番で回す。Kamimusuhi は同じ機能を合成世界で
  小型モデルに学習させて検証し、通ったものだけ接続する。
- Kamimusuhi の学習器官は本番接続ゼロ（5 件とも Shadow）。FIO の機構を移植済みなのは
  想起（Recall v0）だけで、その骨格は実コードと一致した。
- 過去文書の主要な記述は実コードで裏が取れた。一方で「使った分だけ温める」は
  意識層だけの性質であること、ベクトル蔵の大半が構造想起に届かないことなど、
  二次資料からは分からなかった点が複数ある（§3）。

## 2. FIO 実コードの要点

### 2.1 記憶の層

CODE-VERIFIED / CODE-REPORTED（層の列挙は両方、書き手の対応は CODE-VERIFIED）:

| 層 | 保存先 | 書き手 | 想起での使われ方 |
|---|---|---|---|
| 会話履歴 | `memory.db` `history` | 毎ターン | 履歴として送る |
| 台帳 | `memory.db` `memory_events` | 全書き込み | 使わない（照合・再構築用） |
| ベクトル蔵 | `episodic/meta.db` `memories`（numpy 総当たり） | 蒸留記憶、全発話の複写、沈降体験、セッション塊、併合結果 | 起点探しだけ |
| Vault ノート | `vault/02_Memories/*.md` | 書き戻しの蒸留ミラーのみ | タグ・リンク想起の本体 |
| 統合記憶 | `consolidated_memory.json` | `consolidated_diff` | 毎ターン全文注入 |
| 意識層 | `memory.db` `experience_events` | 毎ターン 1 件 | 直近 20 件注入、想起クエリの材料 |
| セッション | `data/sessions/*.jsonl`、`索引.db`（FTS5） | 毎ターンの往復全文 | 全文検索で該当場面を注入 |

「三層記憶」という過去文書の整理より層は多く、互いに別経路で書かれる。

### 2.2 書き込み

- CODE-VERIFIED: 入口は `fio/core/claude_client/memory_writeback.py` の `書き戻す()`。
  応答返却後に走る。冪等の鍵（sha256、600 秒窓）は処理の**前**に立つ（`:398-404`）。
- CODE-VERIFIED: 蒸留保存は `new_memory` があり重要度が 250 を**超える**ときだけ（`:680`）。
- CODE-VERIFIED: Vault ミラーはベクトル蔵への保存の成否を見ずに走る（`:771`）。
- CODE-VERIFIED: `save_distilled_memory` は全例外を「落ちた」に変換するため
  （`fio/core/memory.py:998-1000`）、再試行キューの分岐にほぼ入らない。
- CODE-VERIFIED: 全発話を重要度 300 でベクトル蔵へ複写する（`memory.py:660-690`）。
  蒸留記憶に掛かる保存可否の判定はここを通らない。
- CODE-VERIFIED: 周期処理はメッセージ数基準で、50 ごとに併合、100 ごとに履歴切詰め、
  5 ごとに沈降（`memory_writeback.py:1176-1189`）。docstring の「100 ターン」は約 50 ターン。
- CODE-REPORTED: METADATA は 14 項目。二周目の突き返しは形・空・食い違い・型の 4 種で最大 5 回。
  併合は類似 0.85 超の 2 件を小さい器が 200 字に要約し、元をアーカイブする（長さ・忠実性の検査なし）。

### 2.3 想起

CODE-VERIFIED（式と枠）:

```text
ベクトル   score = similarity * (1 + importance/1000) * 0.995^(days * speed)
           speed = exp(-0.45 * valence)        偽判定は更に *0.05
起点の並べ直し  1/(rank+1) * (1 + importance/1000) * recency * (1 + 0.25*タグ重なり)
タグ       Σ(log(総数/出現数)+1) * (1 + importance/1000) * recency
リンク     3 段、hop ごと 0.45^hop を合算
```

- 式: `fio/core/memory.py:1197-1211`、`fio/core/obsidian_index.py:759-762`、`:1251-1277`。
- 注入枠は `max(絞り, リンク10 + タグ5 + 起点下限10)` で、器によらず実質 25 件
  （`fio/core/claude_client/__init__.py:519-533`）。配分はリンク 16・タグ 9、起点 10 は足場で通常は注入されない。
- 注入書式は `## 関連する過去の記憶` の下に `- 本文` だけで、id は付かない（`__init__.py:3290-3293`）。
- `reinforce_on_access` は既定 False で、True を渡す呼び出しが無い（`memory.py:1131,1243`）。
  エピソード記憶は想起されても強化されない。
- 「構造が空」の判定は予定・短期メモなどを先頭に連結した**後**なので、タグ・リンクが 0 件でも
  常駐メモが 1 件あればベクトル起点への縮退は起きない（`obsidian_index.py:1263-1275`）。
- **ベクトル蔵と Vault は母集団が違う**: `write_episode_md` の呼び出しは書き戻しの蒸留ミラー
  （`memory_writeback.py:826`）、既定オフのセッション蒸留、手動の移行スクリプトの 3 箇所だけ。
  一方 `save_distilled_memory` は 9 箇所から呼ばれる。全発話の複写・沈降体験・セッション塊・
  併合結果はノートを持たず、`resolvable_docs` で起点から外れる。
  コード内コメントは解決率 17〜43% を「未同期」と説明する（`obsidian_index.py:718-730`）。

CODE-REPORTED: クエリは最新入力を 512 トークン単位で割ったもの（最大 16 本）＋意識層の
熱い体験の先頭 80 字（Anthropic 2 本、他 6 本）。履歴は使わない。埋め込みは 1 ターン 1 回。

### 2.4 意識層とセッション

- CODE-VERIFIED: 減衰は `live_heat = heat * exp(-dt/S)`、
  `S = 21600 * (1 + ln(1+ref)) * type倍率 * exp(0.45 * valence)`（`fio/core/experience_store.py:687-702`）。
- CODE-VERIFIED: `correction` は倍率 8.0 だが事前 valence −0.6 を持ち、負 valence は
  `S ≤ 21600*2`（12 時間）に頭打ちになる（`experience_store.py:91-115`、`fio/core/memory_bias.py:111-120`）。
  書き戻しが積む type は `turn` だけ（`memory_writeback.py:1138`）。
- CODE-REPORTED: 温めは METADATA の `used_memory_ids` と注入集合の積だけ。注入したのに
  使われなかった分は冷やす。`used_memory_ids` を省略すると注入分が全部冷える。
- CODE-REPORTED: セッションは往復全文を jsonl に追記し、FTS5（文字 2-gram、BM25、鮮度 30 日半減）で引く。
  新セッションの最初の 1 回だけ前回の往復を最大 20,000 字据える。履歴切詰めはセッションを閉じてから
  10 行残して削除し、閉じられなければ消さない。LLM によるセッション蒸留と忘却クリンナップは既定オフ。

### 2.5 ターン・自律・ツール

- CODE-VERIFIED: ツール往復は 1 試行あたり最大 50 周（`fio/core/claude_client/native_tool_loop.py:842`）。
- CODE-REPORTED: 推論ループは旧 prefetch 型・ネイティブ tool_use 型・SDK/CLI 型の 3 系統が並存。
  二パス（本文を先に返し METADATA は背景）は、1 パスで欠落時に約 32 万トークン・915 秒の再生成が出た実測が動機。
- CODE-REPORTED: Heartbeat は 20 秒周期。DMN は差分検知か平均約 30 分の確率起動で、通常ターンとして走る。
  知覚の処理は「規則で落としてから LLM に訊く」順に並ぶ。
- CODE-REPORTED: ツールは 284 件で、常駐約 40 件＋名前索引、`find_tool`→`tool_schema` で遅延取得。
  プロンプトキャッシュ境界は最大 4 本で、保温の再送を持つ。

## 3. 過去文書との照合

### 実コードで裏が取れた記述（CODE-VERIFIED）

重要度 250 超で保存 / リンク 3 段×0.45 / リンク:タグ = 2:1 / ベクトルは起点のみ /
不変性宣言による統合昇格 / 無効化台への supersede / 他人の発言からの昇格抑止 /
書き戻しの冪等 / FTS5 文字 2-gram＋BM25 / 神経の 4 象限 / 無意識層との接続 JSON 1 本 /
ツール索引の名前のみ化。

### 実コードで初めて分かったこと

| 過去文書の記述 | 実コード |
|---|---|
| 「使った分だけ温まる」（記憶全般の性質として記載） | 意識層（`experience_events`）だけ。エピソード記憶は強化されず id も付かない |
| ベクトル起点を外しても recall 0.130 で不変（実測） | ベクトル蔵の大半がノートを持たず起点から外れる、という構造上の原因がある |
| 凍結アトラクタを実測で確認 | 対策（クリンナップ）は実装済みだが既定オフ |
| 三層記憶 | 別経路で書かれる層が 7 つ以上 |
| 閉会時に 1 段落へ蒸留（9 月時点） | 既定オフ。現行は原文を固めて据え直す |
| 訂正を長く残す層 | 負 valence の上限で 12 時間に頭打ち。かつ `turn` 以外は書かれない |
| ツール 226 → 272 | 284（配布物の登録実測） |

### Kamimusuhi 文書側の古さ

`fio-memory-architecture-measurements-2026-11-03.md` §6 の表は「想起機構未実装」
「`used_memory_ids` 相当なし」と書くが、Recall v0 で実装済み。本書では原文を書き換えていない。

## 4. 実験ごとの対比

判定は [`experiment-dispositions-2026-11-08.md`](./experiment-dispositions-2026-11-08.md)、数値は各 report の転記。

| 実験（判定） | 問いと結果 | FIO の対応物 | OUR INTERPRETATION |
|---|---|---|---|
| R0 記憶ゲート（候補） | 容量有限で何を覚えるか。gru128 0.988、FIFO 0.533 | 本体 LLM が重要度を申告、250 超で保存。容量制限なし | FIO のエピソード記憶には「後で使われたか」の信号が無い。`state_record_usage.use_count` は R0 の報酬に近い実信号 |
| U0 必要性誘導記憶（凍結） | 内部ニーズで記憶価値が変わるか。主閾値未達 | valence で減衰速度を変える程度 | FIO に先例なし |
| H0 恒常性（候補） | 内部変数の維持だけで行動が出るか。PASS だが ID では手製規則が優位 | 資源・位置の差分検知、健全性通知、眠り旗。全て規則 | FIO は規則だけで本番運用。H0 の「規則が学習以上」と同じ向き |
| K0-F / F2 機械内受容（凍結） | テレメトリから計算資源を選べるか。BLIND と有意差なし | 器の選択は環境変数・クールダウン・実測往復時間 | 同上 |
| K0-E / E2 能動情報取得（凍結） | いつ観測・想起・言語呼び出しをするか。言語呼び出しの適合率約 0.48 | 安い判定を先に置き LLM は最後 | 「規則で落としてから LLM」の順が過剰呼び出しの実用的な抑え方になっている |
| S0 自他区別（継続） | 自己起因の変化を分離できるか。AUC 0.90〜0.97、遅延 2 で崩れる | 申告ツールと実行記録の突合、裏付けのない実行主張の検出、出どころタグ | FIO の自他区別は台帳照合で、予測モデルではない |
| T0 時間感覚（継続） | 外部時計なしで経過時間を表現。外挿 0.260 | 現在時刻・稼働時間を毎ターン注入 | FIO は時計を学習させず外から与える |
| P0 予測的驚き（棄却） | 予測できないチャネルだけ注意するか。規則に劣る | 知覚の遷移頻度を数えて沈める（決定論） | 同じ機能を計数で実装 |
| O0 対象永続性（凍結） | 遮蔽中の位置と同一性。追跡規則が優位 | 画面フレームの短期バッファ、5 分の保留棚 | 対応は弱い |
| X0 クロスモーダル結合（凍結） | 結果なし | 神経の気配を確率 OR でまとめる算術 | 対応は弱い |
| G0 系 接地（棄却、v6 は継続） | 未学習と同程度 | なし（知覚は LLM の視覚へ） | 対応なし |
| CX0 器官統合皮質（棄却） | 汎用 GRU が最良 | 統合は 1 本のプロンプト文脈＋接続 JSON | FIO は汎用側の構成 |
| C0-Python 継続性（凍結） | 結果なし | 新セッション冒頭に前回の往復を据える | FIO の継続は原文の据え直し |
| FI0 直列化安定性（継続） | 結果なし | 索引の名前順固定、キャッシュ境界 4 本 | FIO の動機は費用で、決定の安定性は未測定 |
| FI1 ターン・トランザクション（継続） | 結果なし | 冪等、再試行キュー、中断ジョブの非再実行 | FIO の実装で見られた形が障害注入の題材になる（§7） |
| FI2 文脈完全性（継続） | 結果なし | 要約型の切詰めをやめ原文＋全文検索＋据え直しへ | 本番で再帰要約から BM25 併用へ移った実例 |
| FI3 同一性ゲート（継続） | 結果なし | 複製個体・移行中の書き戻し停止 | モデル交換時の同一性ゲートに当たる機構は見当たらない |
| MIOBA・BODY0（凍結） | 閉ループ成立、移植部は定位を悪化 | アバター UI の身体、LLM を呼ばない運動反射 | 対応は名前だけ |

## 5. runtime 機構ごとの対比

Kamimusuhi 側は調査担当の報告。配線・定数は取りまとめ側がコードで確認した
（[`organs.rs`](../../crates/kamimusuhi-runtime/src/organs.rs)、
[`c0.rs`](../../crates/kamimusuhi-core/src/c0.rs)、
[`dialogue.rs`](../../crates/kamimusuhi-runtime/src/dialogue.rs)、
[`idle_cognition.rs`](../../crates/kamimusuhi-runtime/src/idle_cognition.rs)、
[`config.rs`](../../crates/kamimusuhi-runtime/src/config.rs)、
[`kcore.rs`](../../crates/kamimusuhi-resident/src/kcore.rs)）。

| 機構 | FIO | Kamimusuhi |
|---|---|---|
| 1 ターン | 単一 LLM がツール往復を最大 50 周。ストリーミングと割り込みあり | 判断 Provider の関門、言語 Provider の競争、応答ゲート。ツール往復は既定 4 周、段ごと 120 秒。ストリーミングと割り込みなし |
| 保存の決定 | 本体 LLM が毎ターン書き、ほぼそのまま保存 | 反省サイクルが提案、Mutation Policy と Continuity Kernel が受理 |
| 記憶の正本 | SQLite・ベクトル蔵・Markdown・JSON が別経路 | 追記専用の証拠と世代番号つきコミット |
| 忘却・統合 | 類似併合、熱の減衰、履歴切詰め（一部は既定オフ） | 未実装。追記と無効化のみ |
| 自律 | 20 秒の鼓動、通常ターンとして走る自発思考 | ツールなしの反省 1 回（無入力 60 秒、最短 600 秒間隔）。別に Commitment Engine と配達検証つき能動発話 |
| 器官 | 規則と LLM による「神経」が本番稼働 | 学習器官は 5 件とも Shadow。`run_organs_for_persona` は対話経路から呼ばれない。常駐 K-CORE は stdin なしで起動 |
| ツール | 284 件、名前索引＋遅延スキーマ取得 | `tool_catalog`→`tool_enable` の 2 段 |
| キャッシュ | 境界 4 本と保温 | 静的→動的の順に並べ、キャッシュ利用量を計測 |
| 同一性 | 複製検知 | 世代系譜、writer epoch、同一性ゲートの明示承認 |

## 6. 想起の対比（Recall v0）

| 項目 | FIO | Recall v0 |
|---|---|---|
| 起点 | 埋め込み検索＋意識層の熱い体験 | 文字 bigram 一致＋72 時間以内に使った記録 |
| リンク展開 | 3 段、hop ごと ×0.45 | 同じ（`recall_link_depth` 3、`recall_hop_decay` 0.45） |
| タグ | IDF 重み＋重要度＋鮮度 | 共有タグ数 |
| 結び目 | 複数経路の合算 | 被リンク数で加点（0.1）、逆方向 0.20 |
| 枠 | 25 件（リンク 16、タグ 9） | 6 件（2:1） |
| 注入前の審査 | なし | 判断 Provider が関連性を判定 |
| 注入時の id | なし | 型付き id と経路 |
| 使用による強化 | エピソード記憶にはなし | 応答に id が現れた分だけ `mark_used` |
| 有効期間 | なし | `valid_to` 切れを除外 |

OUR INTERPRETATION: 骨格は一致し、FIO が自覚していた穴（有効期間、注入による温め）は
Kamimusuhi 側で先に塞がれている。使用追跡を episodic 記録へ適用している点は FIO より先にある。

## 7. Kamimusuhi への示唆

DESIGN HYPOTHESIS:

- **R0 の実メモリ対応**: 本番化関門 (2) の入力定義に `state_record_usage`（`use_count`・`last_used_at`）を
  使う。R0 の合成報酬「後で照会されたキーだけ覚える」に最も近い実信号。FIO の台帳にはこれに当たる
  エピソード記憶側の信号が無い。
- **構造化 `used_refs`**: Recall v0 が検討事項に挙げる wire には FIO の `used_memory_ids` が実装例になる。
  採る点は「注入集合との積だけ受理する」。避ける点は「欄の省略で注入分が全部冷える」。
- **埋め込み起点を足す場合**: FIO の問題は検索母集団と辿れる母集団が別だったこと。
  `state_records` を単一の正本として同じ表から引く限り、同じ問題は起きない。
- **睡眠・統合**: FIO の忘却系は重要度減衰が廃止、クリンナップと LLM 蒸留が既定オフ。
  足す前に、止められた理由（複利減衰の事故、要約による劣化）を要件に入れる。

FUTURE EXPERIMENT:

- **FI1 の障害注入に次の 3 形を入れる**: (a) 冪等の鍵を処理前に立て、失敗後の再送が捨てられる。
  (b) 投影を先に書き、台帳追記の失敗分を読み戻す経路が無い。(c) 派生ストアへのミラーが主保存の成否と無関係に走る。
- **FI2 / `faithfulness.rs` の反例**: 類似 2 件を小モデルが 200 字へ併合して元をアーカイブする操作を、
  忠実性検査が Mutated / Unverifiable と判定できるか。
- **EXP-FIO-R2 の前提修正**: 「使った分だけ温める」の FIO 側の実績は意識層に限られる。
  episodic 記録での凍結・非凍結は FIO の再現ではなく Kamimusuhi 側の新規測定になる。

## 8. What not to copy

- ベクトル蔵と構造想起の正本を別々の経路で書く構成。
- 保存の成否を呼び出し側が区別できない戻り値（例外の握り潰しと再試行の不発）。
- 仕様の正本と経緯メモを同じ docstring に混ぜること（記述と実装の食い違いが複数あった）。

## 9. 限界・未確認

- 範囲: 安全・権限・承認まわりは扱っていない。
- FIO: SDK ループ本体、ストリーム系、`ambient`・`神経系`・`body` の本文、音声・UI・Discord、
  個別ツールの大半は未読。CODE-REPORTED の項目（併合の周期窓外れ、重要度 1000 超の非クランプ、
  沈降失敗時の印付けなど）は取りまとめ側が原文に当てていない。
- FIO の実測値（解決率、処理時間、DB 規模）はコメントと過去文書由来で、再現していない。
- Kamimusuhi: テスト・ビルドは未実行。反省サイクルや idle cognition の本番頻度は未確認。
- FI0〜FI3（Python）、C0-Python、X0、K0-F2 は結果ファイルが無く、対比は設計同士の比較。
- 本書は FIO の設計を記述した参照ノートで、FIO の動作保証や評価ではない。

## 10. 参照

- 出典: user-supplied FIO runtime source distribution（2026-11-25 版）。
- Kamimusuhi 側: [`../implementation/recall-v0.md`](../implementation/recall-v0.md)、
  [`../implementation/experimental-organ-integration.md`](../implementation/experimental-organ-integration.md)、
  [`../idle-cognition.md`](../idle-cognition.md)、
  [`../provider-fault-containment.md`](../provider-fault-containment.md)、
  [`../../experiments/r0/reports/R0_RESULTS.md`](../../experiments/r0/reports/R0_RESULTS.md)
