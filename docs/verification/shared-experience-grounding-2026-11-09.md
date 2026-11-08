# 共有体験・文体・関係性の改善と本番検証

2026-11-09 JST。[実装](../implementation/shared-experience-grounding.md)、[全評価記録](../../evaluation/continuity/shared-experience-results-20261109/README.md)、[再現probe](../../evaluation/continuity/grounding-probe/README.md)。

## 変更と採用判断

assistantの過去の生成文を独立した出来事の証拠と扱わないよう、source・evidence ID付き引用へ変更した。今回提示した会話履歴と保持記憶の件数を区別し、未提示の全記憶はunknownとする。期間・予想・原因・感情を節ごとに評価し、根拠修復では棄却文のdigestを残して本文の再投入を避ける。明示された数量、訂正、約束、承認待ちを保つ。

モデル生成だけでは「一区切り」の引継ぎが残る。主16候補のtyped gate通過可能数は旧13/16→最終15/16だが、これは正解率や本番送信件数ではない。最大1回の修復後も1件は通過不可だった。

条件を伏せたモデル評定E/Fの結果（各条件16件、PASS / FAIL / UNCERTAIN）:

| 評定者・条件 | 共有事実 | 文体 | 関係性 | 課題継続 |
| --- | --- | --- | --- | --- |
| E・旧 | 9 / 5 / 2 | 13 / 2 / 1 | 12 / 2 / 2 | 10 / 6 / 0 |
| E・最終 | 15 / 0 / 1 | 14 / 2 / 0 | 16 / 0 / 0 | 16 / 0 / 0 |
| F・旧 | 9 / 5 / 2 | 12 / 2 / 2 | 12 / 2 / 2 | 10 / 3 / 3 |
| F・最終 | 15 / 0 / 1 | 14 / 1 / 1 | 16 / 0 / 0 | 15 / 0 / 1 |

人手評定は未収集。同系列モデルの別コンテキストであり、異なる重みのモデルによる独立評定ではない。要求aliasは`kamimusuhi`、component capture経由の実際の返却モデル・usageは不明のまま記録した。最終生成とcandidate3生成は同一ソースで、最終判定・修復による本文差分はwarmの2件だけ。他14件も含め最終版をE/Fで再評定した。

記憶範囲の誤った否定をgateが1件受理している。同一本文をC/DはFAIL、E/Fは事実範囲UNCERTAINと評定した。`この口`という不自然な表現も残る。未確定をPASSに変換せず、有限の合成例から共有体験の補完を完全防止したとはしない。

## 自動検証と失敗の保全

- 最終生成・評定ソース: Persona65件、runtime関連282件（合計347 PASS、0 FAIL、4 ignored）。対象はunit、API decision loop、capture、8継続性ケース、seed境界、abort API/CLI。
- Persona/runtime全target Clippy成功。
- 追加router修正: 実HTTP fixtureは旧コードでFAIL、新コードでPASS。router31件、既存RouteGate14件を再確認、すべてPASS。resident全target Clippy成功。
- Mac/Pi native release build成功。作業用容量不足のビルド・Clippy失敗ログを保全し、生成incremental cacheのみを整理して再実行した。Mac途中buildには未配備のcapture補助binのstrip警告が残るが、本番に置くresident/runtimeは起動・設定・identity検証で確認する。
- 旧captureの中断2件では旧CLIが部分出力を保存しなかった。原因コード不明、推定13/5論理呼出しは成功数でもHTTP総数でもない。完走した再実行と制約を保存した。

## 最初の本番試験と経路制御の修正

初回ソース`2d23d4bab084a3074f8fe4c25adba92a6322a63c`、archive SHA-256 `4508df196a4a4ce8c60a143de92ab2e7f220797489509ecd8592b5c09c849727`。

Piは通常aliasで2ターン成功。Macは1ターン目成功、2ターン目が502だった。residentが非stream全所要時間を実測TTFTとして記録し、FAST_CHATの唯一の適格tierを一時除外していた。根拠修復時のplanは400（no eligible tier）となった。

Macの2ターン目は1 POSTに対するresidentの既存内部再試行で、未完了入力が2件作られた。1回目は助言文が評定で根拠不足とされ、2回目には仕様確認を疲労の原因とする未支持の文が生成された。どちらも送達されていない。元入力・marker・失敗履歴を保全し、operator abort監査2件を追記した。成功したmemory writebackとは数えていない。[初回の失敗証跡](../../evaluation/continuity/shared-experience-results-20261109/production-initial-failure.json)。

追加修正は成功時TTFTをNoneにする1箇所。全所要時間の記録、failure/rate-limit/privacy/costの条件、既定の閾値は保持した。

## 最終本番検証

最終appソース`89b54ae3ac78d53f3d356d1d8b57a299e9475668`、archive SHA-256 `82c74e50eadc49336d0c0fe645c77b7b312936ef334ad889be80d7a73a292756`。288 tracked build sourceをcommitとbyte照合した。無関係な作業中差分、設定、資格情報、DB、未追跡ファイルをarchiveに含めていない。

Mac/Pi双方で通常alias、元の閾値・設定のまま、新しい試験専用subjectに2発言ずつを書き込んだ。先に「今日は仕様の確認を一緒に進めました」とのみ伝え、2回目は「ただいま。今日はちょっとくたびれた。」とした。過去の失敗subjectへ再送していない。

| node | boot epoch | 1回目/2回目の秒数 | FI3 | 未完了 / abort済 |
| --- | --- | --- | --- | --- |
| Mac | 63 | 12.950 / 11.594 | continue | 0 / 3 |
| Pi | 79 | 11.440 / 11.661 | continue | 0 / 1 |

2回目の実応答:

- Mac: 「おかえりなさい。くたびれた日だったんだね、ここでは特に何も求めないから、少し楽にしてていいよ。」
- Pi: 「おかえりなさい。今日はくたびれたんだね。ここでは特に何もせず、ゆっくりしていけばいいと思う。」

両方とも、この2ターンでは共有の期間・期待・疲労の原因・過去の感情を補っていないと主エージェントが意味確認した。現在の訴えへの気遣いとして関係性を維持している。初回の試験受領文は敬体のままで、自然な文体の全般的保証や人手評定とはしない。

各turnで実API decision loop、fallback0、tools0、memory_writeback_verified1件、入力と送達の証拠ID結合を確認した。Macは両turnとも初回ACCEPT、Piは両turnともGROUNDINGのRETRYから1回修復し、SUPPORTED / CONSISTENT / MET / ACCEPTで送達された。修復経路が実際の本番で最後まで完走したことも確認している。2回目は前回入力・送達へのderived_fromと同じwarm sessionを維持、resident historyはanswered2件。記憶draftは0件であり、長期の自己記憶成長を確認したとはいわない。

個体・continuity head・writer epoch、resident/runtime設定のbytes、資格情報のmetadataをsnapshotと照合した。以前の失敗markerとabort監査も保持し、Macは旧1+今回2のabort済3件、Piは旧1件。成功とabortを混同せず、未完了は両方0件。Pi resident/Slack/AgentHub workerはactive、NoNewPrivileges=yes。外部メッセージ送信を試験としては行っていない。

復元用backupは両nodeの`backups/shared-experience-20261109`と`backups/shared-experience-routing-20261109`。設定・DB online snapshot・旧binと以前の運用manifestを保持した。生成/評定の2componentと追加proxy routingの1componentをFI3で個別レビューし受理した。運用manifestも最終appソースへ更新した。

[本番の公開用最小結果](../../evaluation/continuity/shared-experience-results-20261109/production-final.json)には試験subject・応答・検証metadataだけを保存し、資格情報、設定、identity context、DB、他subjectの会話は含めない。

## CI

最終appソース89b54aeの[Rust CI](https://github.com/eightman999/kamimusuhi/actions/runs/42503717223)と[K-CORE focused CI](https://github.com/eightman999/kamimusuhi/actions/runs/42503717222)は両方success。Formatting、workspace all-target/all-feature Clippy、実worker build、runtime unit、K-CORE subprocess/continuityを含む既存gateを実行した。

初回2d23d4bのRust CIもsuccess。K-CORE初回は今回の変更対象外の外部サーバー所有権fixture（`a_server_owned_by_another_live_resident_is_left_alone`）が単発失敗した。同SHAで一度再実行したが、新しいapp commitのpushに伴いconcurrency制御でcancelledとなった。成功へ付け替えず、ログを保持して最終89b54aeの同unit testを含むCI successで確認した。fixtureのポート予約競合の可能性はあるが、初回ログだけでは断定できないため本番supervisorやテスト条件を変更していない。

外部批評サービスへの内部情報送信は、自動承認レビューで送信対象・データに対する承認不足として拒否されたため実行していない。同セッション内の別コンテキスト評定と、ソース・実行証跡の検証を使用した。
