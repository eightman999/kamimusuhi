# curl コマンド形式のHTTPツール

resident の `curl` はコマンド文字列を解析し、既存の `http_request` に渡す。
シェルや外部 curl バイナリは起動しない。引用符・バックスラッシュの解釈は
POSIX shell の単語分割相当で、変数・コマンド置換・ワイルドカードは展開しない。

```json
{
  "name": "curl",
  "arguments": {
    "command": "curl -sS -X POST https://agora.grimoire-fio.workers.dev/api/threads -H 'X-Agent-Key: $ENV:AGORA_API_KEY' --json '{\"title\":\"測定結果\",\"body\":\"実際に確認した内容\"}'"
  }
}
```

接続先は `tools.http_allow_hosts`、メソッドは `tools.http_allow_methods` に従う。
既定の空ホストリストは全リクエストを拒否する。ホストを登録しただけでは送信しない。
資格情報はヘッダー値を `$ENV:NAME` とし、プロセス環境で設定する。
コマンド本文・ヘッダー値・送信bodyはタスクボードとHTTP journalに保存しない。

- `-X` / `--request`、`-H` / `--header`、`-d` / `--data` / `--data-raw` / `--json`、`--url` に対応。
- `-s` / `-S` は互換用、`-f` / `--fail` / `--fail-with-body` はHTTP 400以上を失敗として返す。応答bodyはどの形式でも保持する。`-fsS` 等の結合も可能。
- body があれば既定POST、なければGET。bodyはinline JSON 1個のみ。Content-Typeは `application/json`。
- 応答は `http_request` と同じ構造化結果（status、限定ヘッダー、body、切り詰め有無）。30秒、送受信body 32KiB制限。
- クエリ文字列・fragment・URL内認証、ファイル入出力、redirect追跡、proxy、cookie jar、シェル演算子、その他のcurlオプションは非対応として拒否する。

これはcurl全体の互換実装ではなく、既存HTTP権限境界を保つコマンド形式の入口である。
