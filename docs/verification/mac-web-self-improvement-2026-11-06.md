# Macノード：Webツール・自己改善の稼働検証（2026-11-06）

## 反映範囲

既存MacノードのLaunchAgentを継続使用し、resident/runtimeバイナリ、個体の
`tools.allowed`、ツール通信timeout、Agoraの許可ホスト、headless Playwrightを反映。
旧バイナリ・設定はノードの `backups/web-self-improvement-20261106T015848Z` に保存。
APIキー・node tokenの値は本記録に含めない。

開発checkoutには未コミット変更があるため、自己改善の適用先はノード内の
`workspaces/kamimusuhi-self` に作成した専用ソース。現在の実装を含む独立した
Git管理領域で、既存開発checkoutとその編集は保全した。

## curl・ブラウザ・Agora

- 稼働residentの `curl` から Agora `/api/hello` にHTTP 200。
- Playwright MCP 0.0.82 / Chrome Headless Shell 154.0.8037.0を専用領域に導入。
- 稼働 `browser` からAgoraを表示し、アクセシビリティsnapshotを取得。
- 公開リンクのクリック・戻る、localhostの使い捨てHTMLで入力・クリックを実行。
- 最終snapshot内の `Verified: Kamimusuhi live browser verified` でDOM/JavaScript反映を確認。
- 実MCPは `browser_click` / `browser_type` に `target` を要求した。wrapperの
  公開引数 `ref` を維持し、呼出し先schemaに合わせて変換する互換修正を追加。
  旧schemaとpeer転送を含む関連7テスト、Clippyを通過。
- Agoraの自己紹介を稼働 `agora` で投稿しHTTP 201（thread id 4）。別のGETで
  HTTP 200と本文を確認：[自己紹介](https://agora.grimoire-fio.workers.dev/t/4)。

## 自己改善

Command Codeはcatalog上のhealthが正常でも、実タスクでは認証エラーとなった。
認証情報は変更せず、ユーザーが承認した既存契約内のDevin executorへ切り替えた。
実タスクの修正対象はcurlの `--fail-with-body` 対応・回帰テスト・対応ドキュメント。
Devinのタスク `t1a1ae9c20e2` が修正前の回帰テスト失敗と修正後のcurl関連6件成功を確認し、
指定2ファイルだけを変更して成功終了した。続いて稼働 `self_improve apply` で
resident自身が以下を実行し、すべて終了コード0で成功した。

- resident全テスト（直列）：177,106 ms
- `cargo fmt --all -- --check`：4,548 ms
- resident全targetのClippy（`-D warnings`）：135,974 ms

適用結果は `applied:true`、専用ソースのmerge commitは
`aaf8c991bede15304d6fe60c42c6a6fae927b30a`。
適用後の2ファイルを開発checkoutへ保全コピーし、199のRustソース等の一致を確認して
releaseをビルド・稼働配置した。元の開発checkoutへcommit/pushは行っていない。

このMacで同じ自己改善経路を使う際は `self_improve start` の `executor` に
`devin` を指定する。Command Codeはログインを修復するまで実行先に選ばない。

失敗系も稼働APIで確認した。失敗終了した自己改善taskの `apply` は拒否され、
許可外のHTTPホストと未対応のブラウザ操作も `ok:false` になった。
Mac launcherのPATHにCargoを追加し、次回install-mac.sh実行でも保持するよう修正。
ブラウザ検証用localhostサーバーとページは検証後に停止・閉鎖した。

## 最終稼働版での再確認

自己改善後のバイナリで以下を確認した。

- `curl --fail-with-body` でAgora `/api/hello` をHTTPS取得しHTTP 200。
- localhostの制御した404応答が `ok:false` になり、応答bodyを保持。
- `curl --json` のJSON POSTをlocalhostで受信し、本文とContent-Typeを確認。
- 実Chromiumで公開済み自己紹介ページを再表示し、snapshot内の本文を確認。
- 再起動後もAgora登録状態を保持。

元の開発checkoutは未コミットのまま。運用上の残件はCommand Codeの既存認証エラーで、
今回の自己改善の成功経路は承認済みDevin executor。Pi/llm_masterのバイナリは変更していない。
