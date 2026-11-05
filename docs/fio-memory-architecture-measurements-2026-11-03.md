# FIO 記憶アーキテクチャ実測（2026-11-03）— Kamimusuhi への対応

FIO（フィオシステム）の記憶アーキテクチャに関する実測レポート
（`FIO_記憶アーキテクチャ_実測_20261103.pdf`、2026-11-03 作成）の蒸留と、
Kamimusuhi への対応付け。数値は FIO 側の同一日実測値。
従来の FIO 参照は [`fio-system-reference-analysis.md`](./fio-system-reference-analysis.md)（設計教訓）、
[`research/fio-system-architecture-2026-09-09.md`](./research/fio-system-architecture-2026-09-09.md)（実測スナップショット）、
[`fio-cross-review-and-runtime-countermeasures-2026-09-10.md`](./fio-cross-review-and-runtime-countermeasures-2026-09-10.md) を参照。
本書は「記憶の想起と書き戻しの機構が、実測でどう振る舞ったか」に絞る。

## 1. FIO の記憶構成（実測サマリ）

```text
memory.db (SQLite, 224.7MB)
  memory_events 61,892 / experience_events 5,778 /
  consolidated_events 4,360 / history 392
episodic/meta.db (226.1MB)  エピソード索引 31,746 件
episodic/meta_bge.db        bge 埋め込み 12,767 件
vault/02_Memories           Obsidian 札 7,949 枚・wikilink 54,156 本（6.81 本/枚）
consolidated_memory.json    統合記憶 630.8KB（persona/relationship/context/knowledge）
```

- **三層記憶**: 会話履歴（必ず保存）／エピソード記憶（意味の近さ・リンク・タグで引く）／
  統合記憶（毎ターン全量注入の骨格）。
- **意識層**: `experience_events` が熱を持って冷える（忘却曲線）。温まるのは
  METADATA の `used_memory_ids` で「使った」と申告された分だけ。
  **注入だけで温まる作りにすると一度載った記憶が冷えずに居座る（凍結アトラクタ）**
  —— これは実測で確認された失敗モード。
- **消さない**: 誤った記憶も行は残し透けさせる（`revise_memory` の真偽 verdict、
  統合記憶の `_無効` 台への supersede）。現状は台帳から `consolidated_replay` で畳み直せる。
- **想起の判断はコードではなく個体が持つ**: 入口に注入する材料はコードが集めるが、
  何を記憶として扱うかはモデル（フィオ）が METADATA で宣言する。

## 2. 一ターンの書き戻し（METADATA）

応答本文の末尾に、ターン固有の印で囲んだ JSON を一つ書く。

| 欄 | 行き先 |
|---|---|
| `new_memory`・`memory_importance`（251 以上で保存）・`memory_type` | エピソード記憶と vault の札 |
| `memory_tags`・`memory_links` | 札のタグと wikilink（次の自分が辿る道） |
| `memory_attachments` | 添付を札に貼る |
| `consolidated_diff`（add/update/remove/supersede） | 統合記憶 |
| `used_memory_ids` | 意識層の温め |
| `used_tools` | 実行記録との機械突合 |
| `perception_gap`・`nerve_feedback` | 無意識（ゲート・神経）への帰還路 |

- 書けなかったターンは本文を届けた後に二周目が METADATA だけを書く。
  同じターンが二度届いても二度は焼かない（冪等）。
- METADATA に書かれた語句はそのままでは記憶の slug に当たらない
  （**60 日分 2,311 本の宣言リンクのうち、実際に繋がっていたのは 0 本** — 10-31 実測）。
  語句→過去記憶の解決表 `vault/.fio_link_resolution.json` を別途持ち、
  札そのものは一文字も書き換えない。

## 3. 想起パイプラインと実測値

```text
種 = 今の入力（埋め込み上限で塊分割）＋熱い体験
  → 塊ごとに独立クエリ
  → ベクトル検索は「起点」のみ（起点自体は注入しない。08-30〜）
     ├─ 起点の [[wikilink]] を 3 段まで辿る（段ごと ×0.45）… リンク枠
     └─ 話題タグを共有する記憶 ………………………………………… タグ枠
  → 注入枠は リンク:タグ = 2:1
  → 先頭直行: 長期記憶保存（名前一致）・未了の予定・年表
```

実装箇所: `obsidian_index.py` の `expand_links`（BFS・`_LINK_HOP_DECAY = 0.45`）と
`claude_client` の `_obsidian_second_stage`。
遠い記憶が浮くのは「何本もの道から指された時だけ」（網の結び目）。
点 = 指された強さ × (1 + 重要度/1000) × 鮮度。

### 3.1 ベクトル起点の寄与【実測】

`recall_lab_miss.py`（直近 21 日・40 回・ベクトル起点を外す）:

| 方式 | recall | 一件でも当たった回 |
|---|---|---|
| 通常（起点=ベクトル+タグ） | 0.130 (6/46) | 0.125 |
| ベクトル無し（起点=タグのみ） | 0.130 (6/46) | 0.125 |

ベクトルが正解を当てたのは 40 回中 2 回・2 件のみ。外した後にリンク/タグで戻れたのは 0 件。
母数不足で「戻れない」とは言えないが、**この 40 回では起点のベクトル検索が最終結果を
一つも変えていない**。想起の当たりは主にタグ枠から来ている見込み。

### 3.2 辿り方の比較【実測】

`recall_lab.py`（直近 45 日・107 回・リンク解決表あり）:

| 辿り方 | 宣言 recall | 宣言 一件でも | 自動 recall | 自動 一件でも |
|---|---|---|---|---|
| BFS 段3・減衰（現行） | 0.136 | 0.179 | 0.375 | 0.373 |
| PPR 段3 | 0.153 | 0.205 | 0.264 | 0.353 |
| RRF（現行+PPR） | 0.153 | 0.179 | 0.320 | 0.373 |
| 和 w=0.3 / w=0.5 | 0.136 / 0.153 | 0.179 | 0.327 / 0.320 | 0.373 |

40 回では PPR が宣言で勝って見えた（0.300→0.400）が、107 回では差が縮み
**自動（タグ共有リンク）側で大きく負けた**。融合も現行に勝たず、本番の想起は変えていない。

### 3.3 測り方の限界（FIO 自身の記述）

- 正解は「その時張ったリンク」だけで、本当に必要だった記憶の全部ではない
  → recall の絶対値は低く出る。
- 起点のベクトル検索は入力一本だけで引くため、意識層の熱いクエリは再現できない。
- 宣言リンクの手読み精度は 27 本中およそ 21 本（10-31）。
- タグ由来とリンク由来を分けて数えていない。

## 4. 統合記憶（骨格）の運用

- `apply_diff` で部分更新。書ける場所は persona / relationship / context / knowledge の
  四階層だけ（それ以外は例外を投げず noop + ログ）。
- **昇格基準は重要度ではなく不変性**: `invariant: true` を宣言したものだけが統合に入り、
  無いものはエピソードへ流れる。
- **supersede**（11-01〜）: 「前は正しかったが今は有効でない」行は消さず `_無効` 台へ
  移して注入から外す。二件以上に当たる時は何もしない。
- **書き込みの門**（11-01〜）: 他人が居る場で話し手が主でない回は、統合記憶への昇格を
  止めてエピソードへ回す（`記憶の出どころ.py`）。
- 全変更は `consolidated_events` に残り `consolidated_replay` で畳み直せる。

## 5. FIO 自身が挙げる既知の穴

1. 事実に有効期間（`valid_from`/`valid_to`）が無い — 古い事実が「今も正しい」顔で想起されうる。
2. ベクトル起点が最終結果にほとんど効いていない（3.1）。
3. 物差しの正解は「その時張ったリンク」だけ — recall 絶対値は下限。
4. 統合記憶 JSON の先頭に旧ユーザーの節が残っている。
5. 宣言リンクの解決精度 ~78%（21/27）。

## 6. Kamimusuhi への対応付け

### established external result → 対応する Kamimusuhi 側の仕組み

| FIO の実測・機構 | Kamimusuhi 側 | 状態 |
|---|---|---|
| 消さない・透けさせる（verdict 真偽 / `_無効` 台） | `EvidenceRelation::Corrects`・`OriginClass`・supersession。訂正済み証拠は `check_domain_separation` で新規事実を支えられない（`EvidenceCorrected`） | **実装済み**（型レベル） |
| 台帳から畳み直せる | Canonical evidence は append-only、要約・索引は派生表現（README「正典の証拠と記憶を分ける」） | **設計原則として一致** |
| METADATA による個体自身の記憶宣言 | `MutationProposal`（typed・由来付き）→ Mutation Policy → Continuity Kernel。モデルの宣言は「proposal」であって権限ではない | **実装済み**（より厳格な門） |
| 書き込みの門（他人の発言から統合へ昇格させない） | relationship は `is_first_party_testimony` 必須、self は agent 発話/反省のみ（`SelfDomainContamination`） | **実装済み**（FIO より強い構造的門） |
| 注入だけで温まると凍結アトラクタ | Kamimusuhi は「使ったと宣言した」記憶を追跡する仕組みは現状 `used_memory_ids` 相当を持たない。`reference_consulted` evidence があるのみ | **未実装・教訓として採用余地** |
| リンク解決表（宣言語句→slug。札は書き換えない） | Library は path/検索ベースで「宣言リンク」概念なし | **該当なし（想起機構未実装）** |

### our interpretation

- FIO の最大の示唆は **「ベクトル検索は起点供給に留まり、想起の当たりは宣言リンクと
  タグのグラフ側から来た」** という実測。Kamimusuhi が将来 episodic recall を実装する際、
  embedding 先行ではなく **「個体が書き戻し時に張った宣言リンク + 共有タグの
  減衰 BFS」** を第一の想起路とし、ベクトルは起点の一つに留める設計が実測と整合する。
- 「PPR/融合は現行 BFS+減衰に勝たなかった」は、複雑な ranking より
  **宣言リンクの解決精度**（FIO でも ~78%）が先に効くことを示唆する。
  Kamimusuhi では proposal の `subject_key` / evidence refs が既に構造化 ID なので、
  FIO の語句→slug 解決問題は型付き ID 参照で最初から回避できる位置にいる。
- 「書き込みの門」「不変性昇格」は Kamimusuhi の MutationDomain / OriginClass /
  first-party testimony と同型であり、FIO の実測がこの境界の妥当性を裏付ける
  独立した証拠として使える。

### design hypothesis（未検証）

- 意識層の「使った分だけ温まる」機構を Kamimusuhi に入れるなら、
  `used_memory_ids` 相当を **Persona の応答から構造化宣言として回収し、
  evidence/state の検索順位にだけ効かせる**（canonical state は変えない派生 index）。
  注入→温まるの循環増幅を避けるために、温める入力は「宣言した利用」に限定する。
- 想起評価は recall_lab 型（時刻 T に張ったリンクを正解に、T 以降を索引から外して
  T 直前の入力から再生）が移植可能。Kamimusuhi の evidence は時刻・由来付きなので
  同一の leave-future-out 評価が書ける。

### future experiment

- **EXP-FIO-R1（宣言リンク想起）**: episodic state record に `links[]`（evidence/state id の
  構造化参照）と `tags[]` を持たせ、dialogue turn での想起注入を A/B する
  （ベクトルのみ / 宣言リンク BFS+減衰のみ / 併用）。正解の取り方は FIO 同様
  leave-future-out。
- **EXP-FIO-R2（温めアトラクタ）**: 「注入した記憶を温める」と「使ったと宣言した記憶を
  温める」で凍結が起きるかを soak test。FIO の実測が再現するかを確認する。
- **EXP-FIO-R3（有効期間）**: relationship/self の state record に
  `valid_from`/`valid_to` を置くと「今も正しい顔で浮く古い事実」が減るか。
  FIO が未解決と自覚する穴を Kamimusuhi 側で先に潰せるかの検証。

### what not to copy

- 単一 JSON に毎ターン全量注入する統合記憶（630KB）は Kamimusuhi の
  durable self / persona envelope の構造化 section 分割と相性が悪い。
  Kamimusuhi は domain 分離を保ったまま、想起される断片だけを envelope に載せる。
- Obsidian wikilink を正本にしない — Kamimusuhi の想起参照は型付き ID
  （`EvidenceId` / state record id）で、語句解決表のような第二の正本を作らない。

## 7. 参照

- 出典: `FIO_記憶アーキテクチャ_実測_20261103.pdf`（2026-11-03、measure.py /
  recall_lab*.py の実測値付き）
- 関連: [`fio-system-reference-analysis.md`](./fio-system-reference-analysis.md)、
  [`fio-japanese-python-self-maintenance.md`](./fio-japanese-python-self-maintenance.md)、
  [`persistent-agent-implementation-pitfalls.md`](./persistent-agent-implementation-pitfalls.md)、
  [`implementation/commitment-engine.md`](./implementation/commitment-engine.md)
