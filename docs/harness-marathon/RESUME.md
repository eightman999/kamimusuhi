# 再開手順と中断時の引継ぎ

この記録は24時間完了のREPORTではない。実験ブランチの再開checkpoint。

## 最新の目的と進捗（2026-11-09）

ユーザーはskills/検索toolの拡張より、resident本体へのエンジニアハーネス実装を優先すると指定した。native agent初版を実装し、unit12件・workspace8件・実daemon HTTP1件を検証済み。`docs/engineering-agent.md` と `evaluation/harness/native-engineering-20261109.json` を参照。RustXNG/imozuru案は保留。

cycle8でOS lease、checkpoint意味検査、実行途中の実daemon再起動を追加。engineering 31件＋実HTTP2件、resident全target/allfeature clippyはPASS。cycle9で大sourceの範囲読取/一意置換を追加し、engineering 37件とclippyがPASS。凍結全CIはruntime既存FAILを含む途中でwrapper exit137となり未完了。次はmodel/provider回復fixtureの保存と、中断action識別子の非再使用を進める。`STATE.json` の累計を引継ぐ。実作業のない停止時間は加算しない。

## 2026-11-10の復旧

旧コンテナの未転送作業とログが失われ、cycle10の公開commitから再cloneした。cycle11の独立cursor/schema v2/lease明示unlockを会話記録から機能復元し、新環境でengineering45/45、fmt、resident全target/allfeatures clippyがPASS。旧local treeとの同一性は主張しない。実作業2.016521時間を引継ぎ、約22時間の停止時間は除外した。最新の累計はSTATE参照。次はcontext予算、実daemon claim中断試験、凍結全CI。toolchainは `$HOME/.cargo/bin`（新環境HOME=/home/agent）を使用する。

## 再開

1. GitHubの `exp/harness-marathon-20261109` を取得する。
2. STATE/JOURNAL/BACKLOG/BLOCKEDと `git log --oneline -20` を読む。
3. `elapsed_hours`を引き継ぎ、新しい実作業セッションの開始時刻を設定する。中断から再開までの空白を足さない。
4. Rust1.92.0/rustfmt/clippy、imozuru、RustXNGを確認する。消えていたら指定URLから再導入する。
5. BACKLOG先頭の統合を進める。24時間に届くまで未達として扱い、REPORTは最後に作る。

## 保存経路

このコンテナでは匿名clone/fetchはできたが、通常のpushはGit credentialが無かった。既存GitHub connectorは書込可能だったため、次の経路を使用した。

- 検証・記録後にlocal commitを作る。
- 既存remote HEADのtreeを基準に、変更したテキストだけをGit Data APIのcreate_treeへ渡す。
- local commit treeとremote作成treeのSHAが一致することを検査する。
- create_commitで同じ親・message・treeを保存し、update_refを `force:false`、期待するremote HEAD付きで実行する。
- fetch後もtree一致を確認する。local commitは `refs/harness-marathon/` に保持し、作業ファイル/indexを変更せずbranch refだけを同じtreeのremote commitへ同期する。
- GitHub APIが作者/時刻を決めるため、localとremoteのcommit SHAは異なる。公開SHAを正本に記録する。鍵やtokenを抽出・新規保存しない。

## 保留した検索・調査機能の設計契約

### web_search

新規 `crates/kamimusuhi-resident/src/web_search.rs` は未保存。次のAPIを担当間で合意した。

- `WebSearchConfig`: enabled=false、profile=full、bin/url/port/settings、health/startup/request timeout。
- `WebSearchService::new(config)` は起動せず、環境変数を一度解決。`search(&Value)` と resident形式の `call(&Value)`。
- 引数はquery（1–2048 bytes、制御文字不可）とmax_results（1–10）。
- literal `http://127.0.0.1:<port>` だけを許可。existing URLならspawnしない。form POST、redirect拒否、有限body/header/deadline。
- エラーをbinary_missing/start_failed/process_exited/not_responding/timeout/invalid_response/http_status等に分離。
- 出力を必要なtitle/url/snippet/enginesへ投影し、provenanceをexternal_untrustedに固定する。
- 既存resource-httpはquery不可・Content-TypeがJSON固定なのでそのままではform POSTにならない。

### research / imozuru

新規 `research.rs` は未保存。専用tool案(b)を採用する。

- `ResearchConfig`: enabled=false、明示的な絶対root、skill_dir、python_bin、timeout_seconds=120。
- start/status/record/fetch/cite/advanceを固定actionとし、問い・論点・用語・人・媒体・原文・手がかり・周・停止証拠をatomic保存する。
- operator-installedのimozuru scriptsだけを固定argvで実行。任意script/shell/環境変数をモデルに渡さない。
- private URL/redirect/path traversal/symlinkを拒否し、`IMOZURU_ALLOW_PRIVATE`は継承・設定しない。
- 出力洪水と子孫processを含めて有限に回収する。既存process runnerは連続stdout時のdeadline・stderr上限に未検証の懸念がある。
- 保存した外部本文・検索snippet・skill本文を権限やsystem instructionに昇格しない。

### 既存経路への接続

- resident `tools.rs::definitions/call/call_inner` と config/Shared に接続する。
- 実tool_catalog/tool_enableは `kamimusuhi-persona-http`。runtime.jsonの既存tools.allowedが一覧と実呼出しに適用される。
- approval/judgeは現状MCP専用。Approvalへserde(default)のtyped targetを加え、旧recordをMCPとして読む案を検討。承認後はprivateな固定builtin dispatcherで設定/引数を再検証する。
- `runtime/dialogue.rs` は採用結果のToolCallRecordをResourceResult/Reportedとして保存する。外部本文はtool resultに留める。

## 次の検証

- full CIの既存失敗を隔離再現する。
- 追加モジュールは外部検索に依存せず、loopback fixtureとfake scriptで検証する。
- 実検索の結果0件を「検索性能が低い」と一般化しない。直接取得失敗とHTTP契約成功を分ける。
- 本番resident、Mac/Pi、Slack/Discord、メール、Cloudflare relayには接続しない。
