# 運用補助のRust移植・検証記録

2026-11-05。対象は `kamimusuhi-computer-mcp`、`kamimusuhi-discord`、
`kamimusuhi-monitoring`。研究用PythonとCloudflare Relayは対象外。
旧Pythonは互換性照合用に保持する。

## ローカル検証

移植時の `bash scripts/ci-local.sh` は819テスト成功、失敗・ignoreなし。
整形・workspace Clippy・core依存境界検査も成功した。
Computer MCPは旧版とのstdio比較、DiscordはモックHTTP・キュー制御、
監視生成は既存20パネルJSONとの意味的一致を確認している。
実機検証後の入力修正・Gateway probe追加については、対象3crateの17テストと
全target（example含む）のClippy `-D warnings` を再実行して成功した。

## 実機検証

- Mac: Rust binaryでAX許可、14件のウィンドウ列挙、小領域PNG保存を確認。
  TextEditの専用一時文書でAXツリー、ASCIIと日本語の入力、保存後の文字列一致、
  プレーンテキストのクリップボード復元、マウス移動・クリック・スクロールを確認した。
- 実機でASCII直接入力がIMEの影響を受けることを確認したため、macOSの
  `type_text` はASCIIも貼付経由に変更。貼付後に待ってからクリップボードを戻す。
  修正後はIME設定を変更せずに上記の保存結果が一致した。
- Pi (Linux aarch64): 既存Rust 1.92.0を明示して専用 `/tmp` でnative build。
  本番と分離したXvfb `:109` でhealth、実X11ウィンドウ列挙、クリック・キーの
  イベント受信、Unicode入力コマンド、スクロール、1280×800のPNGを確認。
  Unicodeの文字列表示自体はxevでは照合していない。テスト用Xvfbとxevは終了した。
- Discord: `check_gateway` exampleを既存設定で実行し、実GatewayのREADYと
  設定サーバーへの所属を確認。投稿・resident呼出し・個体DB更新は行っていない。
  probeは対話ハンドラを登録せず、30秒上限で切断する。
- Monitoring: native CLIで一時JSONを生成し、配布JSONとの意味的一致を確認。

本番binary・サービス・認証設定は変更していない。Discordの実チャンネルへの
受付・返信・添付配送と、稼働resident経由のjudge/承認フローは未検証。
llm_masterは今回のSSH接続がタイムアウトしたため、そのホストでは未検証。
