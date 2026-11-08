# 自己記憶・コンテキストの継続性

## 境界

保存、注入、振る舞いへの効果は別々に検証する。採用済み自己報告を、外部世界や相手についての独立した事実・命令・権限として扱わない。
正典evidenceとC0履歴は保持し、訂正や予算による注入除外は削除と区別する。

## 毎ターンの構成

- `DURABLE_SELF`: C0の採用済み自己モデル。直接の根拠とrootが存在し、同じ個体に属し、訂正されていない項目だけを渡す。live対話・reflection・replayで同じprojectionを使う。
- `CONTINUITY_CONTEXT`: 話題一致を待たず渡すactive goals/open questions、subject別記憶へのIDと出所の索引、自己投影の監査、以前のturnの書戻状態。索引自体に変更権限はない。
- 関係・体験・原会話は既存のsubject/source境界と検索・関連性判定を維持する。訂正済みの直接/派生根拠を持つ記憶、履歴、raw recall候補は現在のコンテキストから外す。原文は正本に残る。索引は各domain最大16件、出所ID最大8件で、追加出所数を明示する。索引は全文検索や全件一覧ではない。
- `SELF_NOTES`は既存の作業仮説。自己モデルへ自動昇格せず、未検証というラベルを維持する。根拠hintがevidence IDとして解決でき、訂正されていれば注入から外す。

自己projectionはstored/surfacedの件数・digest、除外field/key/proposal IDと理由を返す。除外した文面を監査欄から再注入しない。
除外詳細は64件とfield/key各128文字のprefixまでに制限し、総除外数を別に記録する。
旧snapshotが大きすぎる場合も24KiBまでとし、超過項目を黙って落とさず除外に記録する。UTF-8 byte上限でありtoken数の保証ではない。
索引のgoals/questionsは核と一部重複するため、全リクエストのtoken上限をこの24KiBで保証するものではない。

## 自己更新と投影の整合性

新しいSelfUpdateは値2,048B、snapshot24KiB/64項目、根拠8件、key128Bまで。
同field別keyの同値重複、変更なし、object key/array memberを落とす一括置換を拒否する。
scalarの訂正や既存keyを保った拡張は可能。意味が近い別表現の重複、自然言語の矛盾・真偽を判定したとは言わない。
aggregateの項目を明示的に取り下げるAPIは追加していない。根拠の訂正は注入除外へ伝播し、scalarは更新できる。

intakeに加えてactivation transaction内で最新view・根拠を再検査する。
headとviewを同一SQLite read snapshotから取得し、選択branchのproposalをpredecessor順に再適用した結果と保存snapshotを照合する。
欠落・不正JSON・書込後の不一致は空モデルへ置き換えず失敗する。rollback branchは混ぜず、過去に新規intake規則を遡及適用しない。
整合性照合の費用は選択branchの長さに比例する。proposalとsnapshot双方の協調改竄に対する署名保証ではない。

## 書き戻しの着地

turn開始前に`memory-writeback/<turn-id>.json`へ本文なしのcrash markerをfsync付きで発行する。
完了時は入力・生成・配信済み発話と、採用されたcanonical draftのreceiptを読み返し、
`memory_writeback_verified` SystemEventを保存・再読込してから使い捨てmarkerを片付ける。
completion直後に停止してmarkerが残った場合も、exact completion ID・個体・session・turnを照合して読取時に完了と判定できる。

`turn.c0.memory_writeback`は提出数・採用数・receipt IDを区別する。`no_drafts`を「覚えた」と扱わない。
配信成功後のDB失敗・中断はmarkerとして残り、再起動後も`memory`/`/memory`の`memory_writeback`へ出る。
進行中との区別はできないため`incomplete_or_in_flight`と呼ぶ。現在のturnは自分のprior healthから除外し、
後続turnの成功で以前の未完了を消さない。未完了turnを自動再実行しない。
新しい配信済み発話には入力・提示した履歴/自己モデル/記憶/想起/認識可能な作業メモ根拠へのDerivedFromリンクを残し、訂正時に同じ内容のechoが無関係な履歴として残ることを防ぐ。
発話と全リンクは`append_with_links`の同一SQLite transactionで保存・readbackする。途中link失敗で未連結発話が履歴に残ることは許さない。
これは保守的な依存追跡であり、実際に使わなかった提示記憶の訂正でも発話が除外される場合がある。過去の出所未連結echoは遡って分類しない。
marker破損は健康扱いしない。新機構以前のturnの障害や、モデルが書くべきdraftを一度も出さなかったことまでは遡って検出しない。

## 5本の評価

| 契約 | 実行評価 |
|---|---|
| 訂正後に旧情報へ戻らない | 根拠のrootを訂正し、次の実DialogueSession入力とfixture応答から旧自己認識が消える。履歴は保持、再採用を拒否し再open後も維持 |
| 再起動後も約束を引き継ぐ | 自己goalを保存しプロセスを閉じる。独立CLIが読め、新sessionの話題非一致入力でも索引・fixture応答へ届く |
| 自己更新で教訓を落とさない | 一覧を削る更新を拒否。別項目を保持した拡張を採用し、次の入力に全教訓が残る。store側でも妥当JSONの投影欠落を検出 |
| 書き戻し停止を検知する | SQLite triggerで配信後のAgentUtterance保存だけを失敗させ、応答配信と保存成功が別であること、再起動後の未完了検出を確認 |
| モデル変更前後の振る舞いを比較 | 同一case/input/contextと明示した条件差のみでpaired captureを比較。機械検査と独立2名の文体・関係性評定を分ける |

前4本は`cargo test -p kamimusuhi-runtime --test continuity_scenarios --offline`。
関連境界はcoreの`self_update_guard`、storeの`c0_integrity`でも検証する。
5本目のschema・CLI・実行例は[継続性比較評価](continuity-evaluation.md)を参照。

fixtureは機構・故障時挙動の検証であり、実LLMの記憶能力や文体維持を証明しない。
実モデル出力・独立評定を得ていない比較は`unverified`を維持する。
新しいcontext/書戻実装はFI3 fingerprint対象となる。既存個体への配備は差分reviewとexact tokenでの明示acceptが必要で、自動acceptを追加しない。
