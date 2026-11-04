# 10 — 参照系: FIO 記憶アーキテクチャ実測との対応

対象: `docs/fio-memory-architecture-measurements-2026-11-03.md`（本ページの詳細版）、
`docs/fio-system-reference-analysis.md`, `docs/fio-cross-review-and-runtime-countermeasures-2026-09-10.md`,
`docs/persistent-agent-implementation-pitfalls.md`

FIO（フィオシステム）は別系統の永続 LLM 個体で、長期実運用と実測記録を持つ
参照実装。Kamimusuhi のドキュメント規律では「外部結果・解釈・仮説・実験」を
分けて記録する。

## FIO の実測から得た設計入力（要約）

| FIO の実測・機構 | 値・内容 | Kamimusuhi への示唆 |
|---|---|---|
| 三層記憶 | 会話履歴（必保存）/エピソード（索引31,746件）/統合記憶（毎ターン全注入, 630.8KB） | Kamimusuhi の session/episodic/self の分離と同型だが、全量注入ではなく domain 分離のまま想起分だけを envelope へ |
| 意識層の温め | 「注入した」ではなく `used_memory_ids` で「使った」と宣言した分だけ温まる。注入で温めると**凍結アトラクタ**（一度載った記憶が冷えず居座る）を実測 | 想起フィードバックを入れる時は「宣言した利用」のみを温め入力にする |
| METADATA 書き戻し | 応答末尾に1個の JSON（new_memory/tags/links/consolidated_diff/used_memory_ids/used_tools…）。書けないターンは二周目が METADATA のみ。冪等 | Kamimusuhi の ProposalDraft + tool_calls 記録が対応。型付き proposal の方が強い門 |
| 宣言リンクの解決 | モデルが書いた語句→記憶 slug の正誤表が別途必要（2,311本中 実際に繋がったのは 0 本→解決表で ~78%） | Kamimusuhi は想起参照を型付き ID（EvidenceId/state id）で持てるので語句解決の第二正本を作らない |
| 想起の当たりはリンク/タグ | ベクトル起点を外しても recall 不変（40回中ベクトル貢献2件）。BFS+段減衰 が PPR/RRF/融合に総合勝ち | **embedding 先行でなく宣言リンク+共有タグの減衰 BFS を第一の想起路に** |
| 書き込みの門 | 他者の発言が居る回は統合記憶へ昇格させない | `is_first_party_testimony`（relationship）・`SelfDomainContamination`（self）で既に構造化済み |
| supersede/無効化 | 消さず `_無効` 台へ・verdict 真偽で透かす・台帳から畳み直し可 | `EvidenceRelation::Corrects`・`EvidenceCorrected` で実装済み |
| 既知の穴 | valid_from/valid_to 無し・ベクトル寄与ほぼゼロ・正解=その時のリンクだけ | relationship/self record の有効期間は Kamimusuhi 側で先に潰せる検証余地（EXP-FIO-R3） |

## 採用しないもの

- 単一 JSON の全量注入式「統合記憶」（domain 分離と prompt cache 効率の両面で不採用）
- Obsidian wikilink を正本にする記憶表現（Kamimusuhi は型付き ID 参照）
- Graphiti 型の常駐グラフDB + 構造化出力 LLM（FIO も「効く根拠が時間系質問に偏る」と見送り）

## Kamimusuhi 側の現在地（記憶まわり）

実装済み: canonical evidence の append-only・domain separation gate・
supersession・`received`/`answered`/`failed` の対話ジャーナル・
commitment notify による「体験としての結果回収」・tool/reference の外部資料扱い。

**Recall v0 実装済み**（`docs/implementation/recall-v0.md`）: 宣言 `links` の
outbound BFS（depth 3・×0.45）＋共有タグ枠を 2:1 で Jev 候補へ載せる想起路、
`valid_from`/`valid_to` 期限フィルタ、「応答で id を名指した分だけ温める」
`state_record_usage` — いずれも FIO 実測の示唆を Kamimusuhi の型付き pipeline に落としたもの。

未実装（設計段階）: declarative 想起の生産側精度測定・被リンク逆評価・
embedding 起点・sleep/dream consolidation。

FIO との最大の構造的差異: FIO は「モデルが METADATA を書いて記憶を作る」、
Kamimusuhi は「モデルは ProposalDraft を出し、MutationPolicy + Continuity Kernel が
 canonical commit する」。記憶の**生成を個体に、権限をカーネルに**分けている点は同じだが、
Kamimusuhi は門を型で強制する。

詳細な実測値・ラダー・実験案は `docs/fio-memory-architecture-measurements-2026-11-03.md`。

[← index](./README.md)
