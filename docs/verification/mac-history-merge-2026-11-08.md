# Mac過去会話のPi統合（2026-11-08 JST）

## 最終状態

履歴統合とAPI受入検証が完了。Pi resident/Slackを再開済み。
resident/runtimeのソース・バイナリ・個体DBはこの作業では変更しない。
Slack crateとPi配備binaryも不変。Library設定反映のため、実行中処理0件を確認して
residentを1回再起動した。後続の別デプロイでは現configのLibrary追加を保全すること。

## 対象と安全策

Mac原本には11往復、うち操作者`eightman`は4往復。
他subjectは`task-review`3、`primary-agent-recovery`1、`primary-agent-verification`1、
`operator-issue58-deploy-check`1、`sister-pi`1で、それぞれ元subjectを保つ。
Mac/Piとも既存state_recordsは0件。会話を現在の事実・自己認識・承認へ昇格させず、
Piの会話履歴とread-only参照資料として統合した。

Mac SQLiteの整合したオンラインbackupを作成し、11往復すべてについてjournalの本文・
返答・subject・原個体をSQLite evidenceと照合した。受付と応答の2レコードは
履歴表示では1往復に折り畳み、元のappend記録も私有backupに残す。
ツール結果や外部API応答を新たな会話・事実として取り込まない。

Pi履歴の取り込みIDは原個体と原レコードから決定的に生成。
同一IDの異なる内容・異なる返答・過去importの改変・部分的な原典欠損は上書きせず停止する。
本文が同じでも別の会話ID・日時・subjectの記録は勝手に削除しない。
履歴にはMac由来・原時刻・現在の事実として未検証であるラベルを表示する。
資料には原文、相手、原個体/turn/evidence ID、fingerprint、historical_onlyを保存する。

## 配置・バックアップ

- Mac原本snapshot: `.local/mac-history-merge-20261108`（Git外・私有）。
- Pi backup: `/srv/kamimusuhi/backups/mac-history-20261108`（親700、ファイル600）。
  `pi-before.sqlite`、`spool-pi/`、`nas-pi/`、`resident.before.json`、
  `canonical-before.json`、検証済み`receipt.json`を保持。
- source bundle SHA256: `291e09ca427784cbb32f28ed4b9333c21517153433299af3a8ef7f39b8be5a71`。
- Pi履歴追加先: spoolの`conversations/dialogue/pi/2026-11-07.mac-history-import.jsonl`。
  通常のspool sync後はNASの同じ相対パスへ配送される。
- 原文資料: `/srv/kamimusuhi/private/mac-conversation-history/transcripts`（700/600）。
- read-only Library名: `mac-conversation-history`。
- SQLiteへのINSERT/UPDATE、個体コピー、正典head置換、reflect実行は一切行わない。
  既存の記憶・identity・履歴を削除しない。

## 検証

- 統合ツール7テスト: PASS。ID衝突、相手分離、同文の別会話、途中書込み、
  再実行でファイルmtime/bytes不変、原典改変の拒否を検証。
- コピー上のdry-run: 11追加、Pi既存との重複0/衝突0。
- コピー上再実行: 追加0、既存import11件をskip。
- 本番適用: 11追加。本番再実行: 追加0、11件skip。
- canonicalのindividuals/canonical_commits/continuity_heads/state_recordsの全行fingerprintが
  適用前後・resident再起動後とも一致: PASS。
- `/v1/kamimusuhi/history`でsix subjectの期待件数（4/3/1/1/1/1）と由来表示を照合: PASS。
- `/v1/library`で登録・既知テスト文字列の原文検索・json_getでhistorical_only=trueを照合: PASS。
- 再開後resident/Slack active、Socket Mode再接続: PASS。
- LLMへの事実抽出や自動記憶昇格を行っていないため、意味上の真偽や人格を変更したと主張しない。
  会話の全文は本記録・回答・Gitに含めない。

## 復元

今回の操作は正典DBを変更していないため、通常の戻しでDBを古いbackupへ置換してはならない。
その後の正当な会話やcommitを巻き戻してしまう。

参照Libraryの追加entryだけを現configから取り除き、今回の`mac-history-` IDだけを
spool/NASの専用importファイルから除外する。専用資料directoryは削除より私有退避を優先する。
resident設定を再読込する場合は実行中処理を確認し、Slackを一時停止して再開する。
`resident.before.json`の全体復元は、その後の他作業のconfig変更がない場合だけに限定する。
原本とbackupは保全し、別作業の記録を削除しない。

## 編集範囲

追加ソースは`deploy/resident/merge_conversation_history.py`と
`deploy/resident/test_merge_conversation_history.py`。本作業によるresident/runtimeコード変更はない。
コード・docsは未コミット。source bundle、本文、SQLite backupはGit外に保持する。
