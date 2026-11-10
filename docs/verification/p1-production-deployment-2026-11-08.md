# P1 FI2/FI3・Slack共同本番配備 (2026-11-08 JST)

## 配備準備と共同作業

Slack担当はMac過去会話11往復のPi履歴・原典Library統合を完了し、正典DB不変と再実行0件を確認した。Library設定反映の再起動完了後にP1更新を実施。Library追加を含む現設定、履歴、資格情報を保全した。SlackはPiの既存窓口を継続し、Macの窓口を二重起動しない。

配備元はmaster `147d7b4` 上の未コミットP1と既存主推論overlay。固定snapshotを作り、Mac/Piそれぞれネイティブrelease buildした。commit/pushは行わない。更新対象はresident/runtimeの2bin。k-core・MCP・Slack binary・unit・資格情報は置換しない。

## 初回smokeで検出し修正した問題

初回archive `0f80e6a672da460237adccb02eaa812283748281618dbd4a50e79567fd464bce`。
Macはboot48→49、Piは履歴統合後boot64→65へ更新し、identity/headと設定metadata不変を確認。両ノードのbaselineを明示承認した。Macは実対話成功・誤pin409・worker回収0。

Pi自身の対話はFI3が未宣言の返却モデル名を拒否し502となった。要求モデルaliasは `deepseek/deepseek-v4-flash-free`、実際の返却名は `deepseek-v4-flash-ga-260731`。拒否を無効にせず、2binだけ旧版へ復元し、Mac50/Pi66で窓口・consumerを再開した。個体DB・履歴・Library設定は巻き戻していない。

修正としてtier単位の `identity_model_aliases` を追加。操作者が明示した返却名だけを宣言集合に含め、変更時は再承認する。未知名の自動学習・自動承認はない。Piでは観測・レビューした上記返却名1件だけを追加し、他の設定を保持する。

第2版archive `3e8b44b9addc581875f8d68eed455c9b5aa1de4ae71e8df417e695367950ee11`。変更はresidentのconfig/identityと構造体fixtureのみ。3crate全回帰 **691 PASS / 0 FAIL / 6 ignored**、Clippy警告なし。Slack固定releaseは8 tests PASS。Mac/Pi固定バイナリで、prompt変更の書込み前拒否・exact token承認後再開・同じ個体の維持を検証する。

## 最終配備

第2版の本番配備・実対話・Slack再接続確認が完了。

- Mac: boot50→51。backup `~/Library/Application Support/Kamimusuhi/node/backups/p1-fi2-fi3-20261107T183209Z`。
- Pi: boot66→67。backup `/srv/kamimusuhi/backups/p1-fi2-fi3-20261107T183210Z`。
- Pi設定追加前のbackup: `/srv/kamimusuhi/backups/p1-model-alias-20261108/resident.before.json`。他設定は構造比較で一致。
- 2bin配置・restart前後、baseline再承認前後で個体ID/root/head一致。restart中の設定・資格情報metadata不変。
- 両ノードの自己宣言digestと相手が認識するdigestを相互照合して一致。モデル宣言集合8件で安定。
- 各個体でstatus→exact review token accept→continue。実際のサービス環境のモデル選択変数を使い、推論中の自動承認は追加しない。
- Mac実対話: Pi経由 `openai/gpt-oss-120b`、確認文字列一致、約1.6秒。
- Pi実対話: orcarouter経由 `deepseek-v4-flash-ga-260731`、確認文字列一致、約3.3秒。
- 両ノードとも誤pinリクエストは409。Provider worker各1件を観測し、応答後残留0。
- Pi resident/Slack/agenthub-mio-workerは全てactiveへ復帰。Slack起動後ログでSocket Mode接続済みを確認。
- Pi residentのNoNewPrivileges=yesを維持。MacのSlack窓口は停止のまま。
- Slackへの新規テスト投稿は行っていない。Slack再接続と、窓口が利用する同じPi対話APIの実LLM成功まで確認。
- FI2の要約意味検証・訂正伝播はローカル回帰で確認し、本番の実会話に故障注入や自動reflectを実行していない。

| node | resident SHA256 | runtime SHA256 |
|---|---|---|
| Mac | `cc967d91ff4a2d0975dd6453376998903f14c0da577a90785ff1ae63d737019d` | `7b560da8fc9d07f997dd962ffa50fabcb74547111c1dad4a12078d99d69af8ee` |
| Pi | `c7a794b176204373880c82c85982de7b8cf8bbd299df877ae806cf96ce6ec20a` | `cc878b27581ca5a42cc8a44b78f025611bb63d91ae34669f90add541dbbe39d0` |

各ノードの `current_state/deployments/p1-fi2-fi3-2026-11-08.json` と同名 `-live.json` に配備・同一性・実ルート・worker検証を保存。第1版の復旧記録もビルド領域の `deployment-attempt1.json` に保持する。

## 復旧

必要時は進行中処理を確認し、PiのSlack/consumerを停止して、上記backupの2binだけをatomic復元し再起動する。個体SQLiteを古いbackupで置換しない。後続の正当な履歴・記憶を巻き戻すため、通常のbin復旧にDB復元は不要。

第2版から第1版へ戻す場合は追加aliasを解釈できないため適合しない。backup内の旧P0 2binへ戻すか、第2版を保持する。設定の別名項目を取り除く必要がある場合も、その項目だけを除去し、Library追加や後続変更を古いconfig全体で上書きしない。既存manifestは消去せず、再びP1を適用するときに現contextを明示reviewする。

外部批評MCPは呼び出したが終了コード1でunavailable、批評結果は得られなかった。検証証拠はローカルfixtureと上記本番smokeに限る。
