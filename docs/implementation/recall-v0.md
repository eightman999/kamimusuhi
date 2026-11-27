# Recall v0（宣言リンク＋タグ想起・使用追跡）

FIO 記憶アーキテクチャ実測（`docs/fio-memory-architecture-measurements-2026-11-03.md`）を
起点にした想起機構の第一スライス。実装済みの内容のみを記述する。

## 何を入れたか

```text
episodic pool (subject 単位・上限)
   │
   ├─ lexical seed（従来の bigram 一致）＋ hot seed（last_used_at が窓内）
   │     │
   │     ├─ link frame: 宣言 links の outbound BFS（depth≤3・hop ごと ×0.45）
   │     │     state:<id> → その記録（pool 外も get_state_records で辿る）
   │     │     evidence:<id> → evidence_refs に含む記録（逆引き）
   │     └─ tag frame: seed とタグを共有する記録（score=共有数）
   │
   └─ リンク:タグ = 2:1（既定計6枠）で候補として Jev 判定へ載せる
        ※ 判定を迂回しない — 注入の可否は従来どおり judge が握る
```

- 宣言メタデータは `candidate`→`payload_json` にそのまま載る（書き込み経路・migration の変更なし）:
  `{"tags":[…], "links":["evidence:<hex>","state:<hex>"], "valid_from":<unix秒>, "valid_to":<unix秒>}`
- `valid_to` 切れの記録は想起対象から除外（FIO の既知の穴「有効期間が無い」を先行で潰す形）。
- `last_used_at`/`use_count` は `state_record_usage`（migration 0006）の別テーブル —
  state_records 行を書き換えない。
- **温めは「注入された」ではなく「使った」分だけ**: 応答本文・tool 引数/結果に
  記録 id（32-hex）が明示的に現れた分だけ `mark_used`。FIO の実測で確認された
  凍結アトラクタ（注入自体が温めて一度載った記憶が冷えない）をこの規則で避ける。
  （「使った」の情報源は後に 3 本へ拡張 — 下の「追補（使用の構造化）」。）
- link frame は relationship 記録にも到達しうる（`state:` link 経由・source_type は
  `durable_memory`）。SelfModel は fail-closed のまま対象外。

## 実装の所在

| 層 | 場所 | 内容 |
|---|---|---|
| core | `memory.rs` | `MemoryLink`（`evidence:`/`state:` 文字列の typed ref）、`StateRecord::{tags,links,valid_from,valid_to,is_valid_at}`、`RecordUsage`、trait `MemoryUsage`（mark_used/usage/usages。読み取り専用 `MemoryRepository` の書き側相方）、`MemoryRepository::get_state_records`（既定ループ実装） |
| core | `time.rs` | `UtcTimestamp::from_rfc3339` |
| core | `c0.rs` | `RetrievalParams` に `recall_link_depth`(3)・`recall_hop_decay`(0.45)・`recall_declared_budget`(6)・`recall_hot_window_secs`(259200)。`param_bounds`/`get`/`set` 済み |
| store | `migrations/0006_recall_v0.sql` | `state_record_usage`（state_record_id PK・last_used_at・use_count、STRICT） |
| store | `memory.rs` | `MemoryUsage` impl（複数行 upsert・IN 問い合わせ）、`get_state_records` override |
| runtime | `dialogue_recall_links.rs` | `select_declared`（BFS+tag selector）、`DeclaredHit`/`DeclaredSelection`、`used_ids` |
| runtime | `dialogue_recall.rs` | `now` の通し・`admits_memory` の `is_valid_at` 門・`RecallStats`（expired_skipped/link_hits/tag_hits）・`RecallGroup::extend_offered` |
| runtime | `dialogue.rs` | 応答確定後の used-marking（FIO ルールのコメント付き）・`TurnC0` 新規4フィールド・`workspace_context["recall"]`・`MemoryRetrieved` trace |
| runtime | `c0/chat.rs` | `/memory` で tags/links/use_count/valid_to を表示 |
| runtime | `dialogue_recall_links.rs` | `usage_sources`（verbatim 参照・自己申告・審査帰属の 3 源を surfaced 集合へ clamp）、`UsageSources::{used,agreed}` |
| runtime | `llm_jev/assessment.rs` | `ResponseAssessmentRequest::usage_candidates`・`ResponseAssessment::usage`・`RecallUsage`・設問 `usage_<i>_<candidate>`・`MAX_USAGE_CANDIDATES`(8) |
| resident | `tools.rs` | 組み込み tool `memory_used`（宣言の受領のみ・状態変更なし） |

## パラメータ（`retrieval.*`）

`recall_link_depth`=3（0–8）・`recall_hop_decay`=0.45（0.0–1.0, f64）・
`recall_declared_budget`=6（0–16, 2:1 split）・`recall_hot_window_secs`=259200（0–31,536,000）

## 検証

- core/store: `tests/recall_v0.rs` 4件（migration・FK・payload 読み・upsert 耐久・batch 順序）
- runtime: `dialogue_recall_links.rs` 内 unit test 9件＋`dialogue_recall.rs` 3件
- E2E: `tests/recall_declared.rs` — 宣言 `state:` link で episodic pool 外の relationship 記録に
  到達→`via: "link_hop_1"` で候補化→判定→注入。ターン1で id 言及→`use_count=1`、
  ターン2で言及なし→据え置き（温め規則の確認）
- `cargo test --workspace` 全緑（`HAI_API_KEY` 未設定環境）

## 限界・未実装

- 宣言リンクの**生産側**は reflector/LLM が `candidate` に書くだけ。reflector
  プロンプトは `tags`/`links`/`valid_from`/`valid_to` を宣言してよいことを
  指示済みで、intake が `sanitize_declared_metadata` で fail-soft 正規化する
  （不正値はフィールド単位で落とす）。宣言精度の測定は未実施
  （FIO でも解決精度 ~78% が壁だった点）。
- BFS は outbound のみ（被リンク側の逆評価・「結び目の浮上」加点は未実装）。
- ベクトル/embedding の起点は未追加（FIO 実測で寄与ほぼゼロだったため後回し）。
- `used_ids`（verbatim 参照）は 32-hex の部分文字列一致のまま — これ単独では応答が
  id を名指さない限り温まらない。構造化した 2 源（自己申告 tool と審査帰属）を
  併走させて補う（下の「追補（使用の構造化）」）。
- replay（`c0/replay.rs`）は同じ lexical seed から `select_declared` で拡張し、
  judge がいない代わりに declared-path を未判定 extras として注入する
  （`via` は staged payload に同梱）。`mark_used` は呼ばない —— replay は
  実ターンではなく、モデルが参照していない。scenario は lexical 経路のまま。
- 宣言リンク recall の計測は `kamimusuhi-runtime recall-eval --dir <d> [--limit N]`
  （FIO `recall_lab.py` 相当・read-only）。正解は「T 時点の宣言リンク」のみの下限値、
  hot/used seed は履歴非再現のため除外 —— 既知の制約はレポート JSON の `limits` に同梱。

## 追補（v0 続編 — 生産側・逆リンク・replay 適用）

- **生産側**: C0 reflector の instruction に宣言キーを明示（`tags`≤8・`links` は提示済み id の
  `evidence:`/`state:` ref・`valid_to` は時限事実のみ）。`c0/reflection.rs::sanitize_declared_metadata`
  が提示 manifest と照合して fail-soft 正規化（不正値はフィールドだけ落とし draft は生きる）。
  `run_with_reflector` seam・testkit `FakeReflector::with_episodic_declared` 追加。
- **逆リンク（結び目）**: `select_declared` に in-degree 逆引きを追加 — ≥2 本の宣言リンクで
  指される記録は `score *= 1 + recall_knot_bonus × min(in_degree,5)`（FIO「網の結び目」）。
  逆方向 traversing は `recall_reverse_decay`（既定0.20、0で無効）で弱い枝として辿れる。
- **replay**: `c0/replay.rs::assemble_replay` も同じ `select_declared` で拡張（判定なしの
  extra として注入・`mark_used` はしない — replay は本ターンではない）。
- 追加パラメータ: `recall_knot_bonus`=0.1（0.0–1.0, f64）・`recall_reverse_decay`=0.20（0.0–1.0, f64）。
- 残る未実装: embedding 起点（FIO 実測で寄与ほぼゼロのため後回しのまま）・
  宣言精度の実測評価（FIO でも ~78% が壁だった領域）・sleep/dream consolidation。

## 追補（使用の構造化 — 自己申告＋審査帰属の冗長化）

Persona の応答は自由文だけで、使った記録を構造化して返す経路が無かった。verbatim 参照
（32-hex の部分文字列一致）だけでは実質ほぼ温まらないため、「使った」の情報源を 3 本にした。
どれも「注入された」ではなく「使った」という主張で、surfaced 集合（そのターンに言語器官へ
提示した記録）へ clamp してから和集合を `mark_used` する。

| 源 | 経路 | 追加コスト |
|---|---|---|
| referenced | 従来どおり。応答本文か、宣言 tool 以外の tool 引数・結果に id が現れる | なし |
| declared（自己申告） | 言語器官が組み込み tool `memory_used({"ids":[…]})` を呼ぶ。`state:<id>` 形・大文字も受ける | tool を呼んだターンはモデル往復 +1 |
| judged（審査帰属） | 応答審査（Jev / OpenAI 互換 decision）の同じ batch に、候補ごと・記録ごとの選択式設問 `USED / NOT_USED / UNCERTAIN` を足す。読むのは配達する候補の分だけ | LLM 呼び出しの追加なし。設問が「候補数 × 記録数（≤8）」増える |

規則:

- **clamp**: declared は surfaced に無い id を棄却し、件数だけ trace に数える（`declared_rejected`）。
  judged も surfaced に無い id の判定は無視する。宣言は host が提示したものを追認できるだけで、
  発明された id・古い id は何も温めない。
- **省略は「使っていない」ではない**: 宣言が無い・判定が `UNCERTAIN` / `NOT_USED`・審査が
  fallback（ルールベース）のいずれでも、何も冷やさず何も取り消さない。FIO は申告欄の省略で
  注入分が全部冷える作りだったが、ここでは採らない。
- **審査不在時**: ルールベース gate は帰属を返さない（空）。その場合は referenced と declared だけで動く。
- **trace**: `workspace_context["recall"]["usage_sources"]` に
  `judge_candidates / referenced / declared / declared_rejected / judged / agreed` を件数で残す
  （id は残さない）。`agreed` は declared と judged の重なりで、2 つの構造化源の一致を後から測れる。
- **権限**: 使用の印は側表 `state_record_usage` の bookkeeping で、正典の記録内容ではない。
  Continuity Kernel を通らず、真偽・durable belief・mutation 権限のいずれも与えない。

有効化と配備:

- `memory_used` は組み込み tool だが、言語器官に見えるのは個体の `runtime.json` の
  `tools.allowed`（毎ターン提示するなら `tools.core`）に入れたときだけ
  （例: `deploy/resident/runtime-reference.example.json`）。入れなければ declared は常に 0 件。
- 審査帰属は設定なしで有効（surfaced 記録があるターンだけ設問が増える。0 件のターンは batch 不変）。
- `llm_jev/assessment.rs`・`llm_jev.rs`・`llm_jev/api.rs` は同一性 manifest の対象なので、
  この版を常駐へ入れると identity gate が `AcceptRequired` になる（明示承認が必要）。

検証:

- `llm_jev/assessment.rs` unit 4 件（設問の増え方・選択候補だけ読む・候補 0 件で batch と wire が不変・
  上限と一意性・ローカル gate は帰属なし）
- `dialogue_recall_links.rs` unit 3 件（3 源の clamp と和集合・省略と UNCERTAIN は無印・宣言 tool を
  verbatim に二重計上しない）
- resident `tools.rs` unit 1 件（`memory_used` は受領のみで何も書かない・不正引数の拒否）
- E2E `tests/recall_declared.rs`: 応答が id を名指さないまま、ターン 1 は審査帰属だけ、ターン 2 は
  自己申告だけで `use_count` が進み、surfaced に無い id は棄却、ターン 3（宣言なし・UNCERTAIN）は据え置き

未実装・未検証:

- 実モデルでの挙動は未測定 — 言語器官が実際に `memory_used` を呼ぶ頻度、審査の `USED` 判定の精度、
  2 源の一致率（`agreed`）、設問増による審査レイテンシ。いずれも fixture による検証まで。
- 源ごとの内訳は trace のみ。`state_record_usage` に源の列は足していない。
- replay（`c0/replay.rs`）は従来どおり `mark_used` を呼ばない。
- R0 本番化関門 (2) の入力定義（`use_count` を「後で照会された」信号として使う案）は未着手。

## 関連

- [`../fio-memory-architecture-measurements-2026-11-03.md`](../fio-memory-architecture-measurements-2026-11-03.md) — 実測の出典と EXP-FIO-R1〜R3
- [`../fio-system-reference-analysis.md`](../fio-system-reference-analysis.md)
- [`../../deepwiki/05-individual.md`](../../deepwiki/05-individual.md)
