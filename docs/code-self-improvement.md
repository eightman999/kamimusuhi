# コードの自己改善

既存の設定提案 `improve` と別に、`self_improve` は実コードを修正します。
`start` が既存 task plane の隔離 worktree で修正を行い、`status` で完了を確認し、
`apply` が resident 自身によるテスト・評価を経てソースへ適用します。
モデルの「テスト成功」という報告だけでは適用しません。
サービス再起動・本番デプロイ・push は行いません。

resident 設定へ以下を追加します。既存 task_plane に、同名の `allow_write: true`
workspace と利用可能な coding executor が必要です。
設定がないノードでは自己改善の適用は無効です。

```json
{
  "self_improvement": {
    "workspace": "kamimusuhi",
    "tests": [["cargo", "test", "--workspace", "--offline"]],
    "evaluation": [
      ["cargo", "fmt", "--all", "--", "--check"],
      ["cargo", "clippy", "--workspace", "--all-targets", "--offline", "--", "-D", "warnings"]
    ],
    "timeout_secs": 900
  }
}
```

コマンドは操作者の設定にある argv 配列で、シェル展開しません。テストと評価を
それぞれ1件以上指定します。timeout は両段階合計の上限（1〜3600秒）です。
子プロセスは最小限の環境とプロセスグループで起動し、resident の認証情報を
渡しません。出力の本文は履歴へ保存せず、各コマンドの終了コード・成否・時間を記録します。
テスト失敗時の詳細は、保持された worktree で同じコマンドを実行して確認してください。

個体の runtime.json の `tools.allowed` に `self_improve` を追加します。
同期的な `apply` の終了まで待つため、`tools.timeout_ms` は上記設定なら
`960000` 程度に設定します。peer 経由には300秒の上限があるため、長い検証は
自己改善ポリシーを持つノードへ直接接続してください。

```json
{"name":"self_improve","arguments":{"action":"start","objective":"不具合を修正し、再発防止テストを追加する"}}
{"name":"self_improve","arguments":{"action":"status","id":"返されたtask_id"}}
{"name":"self_improve","arguments":{"action":"apply","id":"返されたtask_id"}}
```

適用条件は、自己改善用に開始したタスクが成功していること、続きのタスクが
存在しないこと、本体 HEAD が候補の分岐元から進んでいないこと、候補の
HEAD がタスク完了時の記録と一致し、HEAD/branch が検証前後で一致すること、双方に未コミット・未追跡ファイルが
ないこと、テストと評価が全て成功することです。失敗時は候補と差分を残します。
成功時は既存 worktree 適用経路でローカル merge commit を作り、候補を片付けます。
後続の通常タスクへ移した候補は、新しい自己改善タスクとして作り直してください。

これはホストOSのsandboxではありません。executorとテストは設定されたユーザーの
権限で実行されるため、実行先の既存sandbox・権限境界を利用してください。
テストや評価の妥当性は設定されたチェックが担います。
