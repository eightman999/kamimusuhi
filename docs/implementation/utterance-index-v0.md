# Utterance index v0（発話の全文索引・派生）

直近プール（`retrieval.evidence_pool`、既定 96 件）から外れた古い発話を、想起の候補として
審査へ渡せるようにする派生索引。FIO の転写索引（文字 2-gram・BM25・ありふれ語の除外・鮮度）を
Kamimusuhi の型付き pipeline に移したもの。実装済みの内容のみを記述する。

## なぜ要るか

生発話の想起は「証拠の新しい順 `evidence_pool` 件」を文字 bigram で照合するだけだった。
このプールは種類を問わない証拠（tool 結果・system event を含む）で数えるので、発話はすぐ
押し出される。プールの外へ出た発話は、reflector が episodic 記録として捕まえていない限り、
二度と候補にならなかった。

## 何を入れたか

```text
evidence_records（正本・追記のみ）
   │  ターン冒頭に同期（1 ターン最大 2,000 件・派生表だけ書く）
   ▼
utterance_index（FTS5・contentless・本文は持たず bigram だけ）
utterance_index_map（FTS rowid ↔ evidence_id）
   │  検索（読み取り専用）
   │    入力の bigram → 索引に無いもの・ありふれたもの（>30%、索引 50 件以上のとき）を落とす
   │    → 珍しい順に最大 24 個で OR 照合 → bm25 上位
   ▼
runtime: 点 = −bm25 × (1 + 0.35 × 0.5^(日数/30))
         点 > 0 かつ 先頭点 × 0.25 以上だけ残す
         訂正済み・根拠なし・他 source・今ターン提示済みを除外
   ▼
追加候補として Jev の関連性判定へ（`via: "utterance_index"`）
   ※ 判定を迂回しない — 注入の可否は従来どおり judge が握る
```

- **派生であって正本ではない**: 索引は `evidence_records` からいつでも作り直せる。発話本文は
  索引に複製しない（contentless）。正本の行は一切書き換えない。
- **追加候補であって注入ではない**: 索引の当たりは Recall v0 の宣言経路と同じ「extra」。
  baseline・プール候補を押しのけず、判定が `Uncertain` なら workspace に入らない。
- **範囲は従来の生発話想起と同じ**: 同一個体・同一 `source_id`・未訂正・根拠あり・今ターン未提示。
- **Ephemeral は索引しない**: `retention_class = standard` の発話だけ。公開チャンネルの一時的な
  発話を、索引が長く引ける状態にしないため。
- **rowid に依存しない**: `evidence_records` は暗黙 rowid しか持たず、VACUUM（スナップショットの
  `VACUUM INTO` を含む）で番号が変わりうる。索引は `evidence_id` で結び、FTS 側の rowid は
  `utterance_index_map` の明示的な `INTEGER PRIMARY KEY` にしている。
- **読みと書きを分離**: 検索は読み取り専用。同期はライブの対話ターン冒頭だけで行う。
  write-free な経路（replay・`recall-eval`）は索引を読まず、書きもしない。
- **fail-soft**: 同期や検索が失敗しても想起の他の経路でターンは進む。失敗は trace に残す。

## 実装の所在

| 層 | 場所 | 内容 |
|---|---|---|
| store | `migrations/0007_utterance_index.sql` | `utterance_index`（FTS5 contentless, `unicode61 remove_diacritics 0`）、`utterance_index_vocab`（fts5vocab）、`utterance_index_map`（STRICT）、未索引走査用の部分索引 `evidence_indexable_utterances` |
| store | `utterance_index.rs` | `utterance_grams`、`utterance_index_sync`（冪等・件数上限つき）、`utterance_index_search`（珍しい bigram の選択と bm25 順）、`utterance_index_len` |
| core | `c0.rs` | `RetrievalParams::utterance_index_top_k`（既定 2）。`PARAM_BOUNDS`/`get`/`set` 済み |
| runtime | `c0.rs` | `search_utterance_index`（鮮度・相対足切り・採否規則）、`utterance_index_score` |
| runtime | `dialogue_recall.rs` | 索引の当たりを evidence 群の extra として提示、`RecallStats::{index_hits,index_unavailable}` |
| runtime | `dialogue.rs` | ターン冒頭の同期、`workspace_context["recall"]["utterance_index"]`、`TurnC0::recall_index_hits` |

## パラメータと定数

- `retrieval.utterance_index_top_k` = 2（0–16）。0 で索引経路を止める。`evidence_top_k` が 0 のときも止まる。
- 定数（FIO の実装値）: 照合に使う bigram 最大 24、ありふれ判定 30%（索引 50 件以上で適用）、
  鮮度の重み 0.35・半減期 30 日、相対足切り 0.25、取得件数 `max(8 × top_k, 40)`。
- 同期は 1 ターン最大 2,000 件。既存個体の積み残しは数ターンかけて解消する。

## trace

`workspace_context["recall"]["utterance_index"]` に件数だけを残す（発話や id は残さない）:
`offered`（審査へ渡した件数）・`synced`（このターンで索引した件数）・`sync_pending`・
`sync_failed`・`unavailable`。

## 検証

- store `tests/utterance_index.rs` 5 件: v7 への移行、同期の冪等性と件数上限、Ephemeral・非発話を
  索引しない、正本の行数が変わらない、source ごとの分離と順位、ありふれ bigram の除外、
  VACUUM 後も正しい証拠を指す
- store unit 1 件（bigram の作り方）、runtime unit 3 件（鮮度の式、extra 候補として審査を通る、
  evidence 経路が無効なら提示しない）
- E2E `tests/utterance_index_recall.rs` 2 件: 110 件の発話でプールから押し出した発話が
  `via: "utterance_index"` で審査に渡り、関連と判定されると workspace に入る。判定が未決なら入らない。

## 配備時の注意

- **schema v7 は前進のみ**。一度この版で開いた DB は、v6 までしか知らない旧バイナリでは開けない
  （migration runner が fail-closed で拒否する）。巻き戻す場合は移行前のスナップショットが要る。
- 初回ターンで最大 2,000 件を索引する。長く動いている個体では数ターン `sync_pending: true` が続く。
- 索引の大きさは bigram 化した本文に比例する（本文の数倍の文字数を転置索引として持つ）。

## 限界・未実装

- 実運用での効果は未測定。検証は fixture までで、実データでの再現率・審査の採否・レイテンシは測っていない。
- 複数ターンにまたがる文脈は返さない。FIO は当たりの前後数往復を「該当シーン」として注入するが、
  ここでは発話 1 件を候補にするだけ。
- 1 セッションあたりの件数制限（多様性）は無い。
- 索引の作り直しコマンドは無い（派生表を空にすれば次のターンから同期で再構築される）。
- 証拠が将来削除される場合の索引側の掃除は未実装（`contentless_delete=1` で削除自体は可能な形にしてある。
  検索は `evidence_records` と結合するので、消えた証拠が返ることはない）。
- NFKC などの文字正規化はしていない（既存の `lexical_score` と同じ）。全角英数と半角英数は別の bigram になる。
- FI2（文脈完全性）の BM25 系対照として使えるが、その評価は未着手。

## 関連

- [`recall-v0.md`](./recall-v0.md) — 宣言リンク・タグ想起と使用追跡
- [`text-dialogue-v0.md`](./text-dialogue-v0.md) — 対話ターンと生発話の想起
- [`../research/experiment-dispositions-2026-11-08.md`](../research/experiment-dispositions-2026-11-08.md) — FI2 の位置づけ
