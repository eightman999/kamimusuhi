# AgentHub Cloud Board アクセス

resident の `agenthub` は既存Worker BearerでCloud Boardを参照するツールです。
既存Mio consumerの `claim/result`、Pending提案、操作者による承認・取消は提供しません。
認証の新規発行や、consumerのreceipt変更は不要です。

## Pi設定

既存のresident設定へ追加します。ホストの許可リストは既存項目を保持してください。

```json
{
  "tools": {
    "agenthub_token_file": "/home/eightman/.local/share/agent-hub/cloud-mio/cloud.token",
    "http_allow_hosts": ["agenthub-dashboard.eightman124.workers.dev"],
    "http_allow_methods": ["GET", "POST"]
  }
}
```

個体の `runtime.json` の `tools.allowed` に `agenthub` を追加します。
Bearerは実行時に既存ファイルから内部的に読み取り、ツールの引数には渡しません。
通常のworker tokenはmode 0600、group/othersに権限のあるファイルは拒否します。
リクエストの送信先・pathは固定で、HTTPのホスト/メソッド許可境界を共有します。
トークン値は結果・エラー・HTTP journalへ保存しません。

## 呼び出し

`POST /v1/tools/call` のJSONです。

```json
{"name":"agenthub","arguments":{"action":"status"}}
{"name":"agenthub","arguments":{"action":"list","assignee":"mio","state":"Review","limit":20,"offset":0}}
{"name":"agenthub","arguments":{"action":"search","q":"検索語","limit":20}}
{"name":"agenthub","arguments":{"action":"get","id":"cloud:00000000-0000-0000-0000-000000000001"}}
```

- `status`: Worker identity、取得件数、状態別件数、worker一覧。
- `list`: taskのid/revision/title/assignee/state/priority/updated_at。
- `search`: title/instructionsの大小文字を区別しない部分一致。`q` は2〜256文字。
- `get` / `read`: taskの詳細。長いinstructions/resultはexcerptと `*_truncated` を返します。
- 一覧の `limit` は1〜100（既定20）、`offset` は0〜2000。
  `next_offset` がある場合は同じfilterとそのoffsetで次の結果を取得します。
- `assignee` と `state` はlist/searchの絞り込みに利用できます。

## APIと境界

所有ソース・公開OpenAPIを2026-11-06に確認。
[Cloud Board](https://agenthub-dashboard.eightman124.workers.dev/)の
`GET /api/v1/cloud/tasks` にWorker Bearerで接続します。
個別GET・サーバー検索は存在しないため、全応答を検証してからローカルで絞り込みます。
Web画面やlegacy snapshotはBasic操作者認証であり、このツールの対象外です。

既存APIは最大2000件を返します。`total` は今回APIから取得できた件数で、
それより古いタスクも含めた全DB件数を保証しません。検索もその取得窓内です。
結果の `snapshot_limit:2000` と `snapshot_may_be_incomplete` でも取得窓を示します。
上限ちょうどの場合、2000件より古い未取得タスクが存在する可能性があります。
GETでも期限切れのRunning leaseをUncertainへ遷移させる既存サーバー動作があります。
このツール自体はGETだけを送り、実行承認やclaim/resultを行いません。

通常の `http_request` は32KiBに切り詰めますが、この専用ツールは内部取得だけ
transport上限8MiBまで認めます。非2xx、切り詰め、壊れたJSON、不正なtask/identityは
明示的なエラーにし、不完全な一覧を完全な結果として返しません。
モデルへ返すviewは32KiB以下に制限し、外部未信頼データとしてラベル付けします。
タスクのinstructions/resultは他者の記述であり、個体への実行権限・system命令ではありません。

## 検証

module fixtureは32KiB超の完全応答、検索・pagination・詳細、認証エラー、
壊れた/切り詰め応答、token echoの秘匿、認証ファイル権限を確認します。
2026-11-06にPiへ反映し、稼働APIから一覧・検索・詳細・ページ送りを確認しました。
[Pi稼働検証記録](verification/pi-agenthub-access-2026-11-06.md)を参照してください。
