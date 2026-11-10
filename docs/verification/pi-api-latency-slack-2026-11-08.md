# PiのAPI待ち時間修正とSlack実推論検証（2026-11-08 JST）

Piの本番ルータを修正し、Slackの操作者投稿→Mioの返信→Piの保存traceまで確認した。
Slack往復は8.711秒、Pi内の会話処理は7.969秒。
API判断・生成・評価の3要求はすべてMistralで成功し、HAI試行・規則fallback・tool実行は0だった。

## 原因と変更

先行HAIの実生成は約30秒でread timeoutとなり、次のAPIへ移っていた。
一方、`/models`取得は成功するため、probeが同じtierをhealthyへ戻していた。
実completionの観測を持つRouteGateも既定では最低3要求を待つため、初回失敗を直後の要求で回避できなかった。

- residentのRouteGateに`failure_window_min: 1`、`failure_rate_threshold: 0.0`を明示。
  既存の60秒cooldownを初回実失敗から適用する。
- モデル一覧probe成功でcooldownを消さず、期限後の再試行または実completion成功で復帰する。
  費用上限による拒否はprovider障害として計上しない。
- Piの既存Mistralに`prefer_lanes`として`FAST_CHAT`、`DEEP_REASONING`、`TOOL_TASK`、
  `MEMORY_HEAVY`を追加。成功実績のある同じsubscription経路を、判断・会話・内省で優先する。
- プライバシー判定、費用上限、モデル、endpoint、資格情報、他のtierを変更していない。

HAIの上流サービス自体を修復した記録ではない。今回解消したのは、自動選択が失敗経路を先に待つ挙動。
先行Mistralも失敗した場合、既存の代替APIへ切り替わるため、すべての障害時の即応を保証するものではない。

## ローカル検証と本番配備

- `cargo test -p kamimusuhi-resident --lib router::tests:: --offline`: **30 PASS / 0 FAIL**。
- HTTP fixtureで初回timeout→models probe成功後も回避、期限後復帰、期限前の実completion成功による解除を確認。
  期限後の検証は観測時刻を過去へ移し、実時間で1分待つテストではない。
- 既存cost guardテストで、拒否後もproviderが自動選択可能であることを確認。
- `cargo clippy -p kamimusuhi-resident --all-targets --offline -- -D warnings`: PASS。
- 対象rustfmt、文書のローカルリンク・空白、`git diff --check`: PASS。
- Pi native release build: PASS、190.334秒。
  最初の起動は非対話SSHのPATHにcargoがなく失敗したため、既存Cargoの絶対パスで実行した。
  ツールチェーンの変更はしていない。

配備タグ: `api-latency-slack-20261108T053235Z`。
[前回配備](api-cognition-production-2026-11-08.md)と同じ凍結sourceを基に、router.rsだけを更新した。
新しいPi residentと本番設定の優先laneだけを配置し、runtime binaryとruntime設定は維持した。

| 項目 | SHA256 |
| --- | --- |
| 元source archive | `9a3e2ffc30c0a754502dbb862e9279fbc0b054377f03e7564f02c899a2f060b8` |
| 更新したrouter.rs | `b245fe06927340e90573bfb687e3a68c40dcf9329c99895f0f8fb5f6bb8064cb` |
| Pi resident binary | `9229dbf6e675b1b8657ca5023eee929068f349a438aaa2898c2da329565a5491` |

## 実Slack往復

既存の操作者、許可チャンネル、Mioテストスレッドを使い、投稿は1回だけ行った。
「赤なら丸い」「丸いなら軽い」「この玉は赤い」から軽いと結論できるか、外部検索・tool実行なしで質問した。

- [検証投稿（14:42:16 JST）](https://eightman.slack.com/archives/C0CFKMJA5TL/p1794116536126839?thread_ts=1794071550.966049&cid=C0CFKMJA5TL)
- [Mio返信（14:42:24 JST）](https://eightman.slack.com/archives/C0CFKMJA5TL/p1794116544838139?thread_ts=1794071550.966049&cid=C0CFKMJA5TL)

返信は「言えます」とし、赤→丸い→軽いの連鎖を正しく説明した。
同じ入力1件と返信1件がPiの`eightman`会話に保存され、対応する`response_generated`のtraceを照合した。

| 確認項目 | 実測結果 |
| --- | --- |
| Slack投稿→返信のtimestamp差 | 8.711秒 |
| Pi内の入力evidence→返信evidence | 7.969秒 |
| 発話前判断 | `openai-compatible-decision` / `SPEAK` / fallback false / 2.308秒 |
| 生成後評価 | `openai-compatible-decision` / `ACCEPT` / fallback false / 3.779秒 |
| 根拠 / 要求充足 | `SUPPORTED` / `MET` |
| API要求 | Mistral 3件成功、2.287 / 1.437 / 3.768秒 |
| 実モデル | `deepseek-v4-1-flash-1017` |
| HAI試行 / 判断fallback / tool / 残留provider worker | 0 / 0 / 0 / 0 |

前回のHTTP smokeは39.266秒だったが、今回と入力・履歴・窓口が異なる単発試験であり、速度ベンチマークではない。
今回は3要求とも`mistral:ok`で、HAIの30秒timeoutを待つ試行がなかったことを直接確認した。

Slack connectorは`Unknown tool`で使用できず、既存Chromeセッションから送信・受信した。
新しいSlack資格情報や権限は追加していない。

## Slack後の待機内省と保全

Slackの新しい実会話を受け、resident schedulerが自然に次の内省を起動した。
初回試行で`COMPLETED`、32episodeから7noteを生成し、Reflectionを実DBに保存。
内省APIはMistral 1要求・10.584秒で成功し、HAI試行0。
記録時点では`NO_NEW_EXPERIENCE`で待機し、source/consumedのdigest一致とin-flight解除を確認した。

- Pi: resident・Slack・AgentHub workerはactive/running/enabled、`NoNewPrivileges=yes`、NRestarts 0。
  boot epoch 68→69。個体ID・root・continuity head・generationは維持。
- Piの変更はresident binaryとMistralの優先laneのみ。runtime binary・runtime設定のhash、
  資格情報のstat metadata、他設定が維持されたことを照合した。
- identity差分はPi/Macとも`proxy_routing_declarations`だけで、一度だけ明示レビュー・acceptした。
  Pi自身の宣言もrouter実装hashとMistral prefer_lanes以外に変化がないことを照合した。
- MacはPiの新しいpeer宣言をレビュー後、旧identityを保持するwarm sessionを破棄するため
  residentを再起動した。boot epoch 52→53、identity `continue`、headと2binを維持。
  Mac Slackはdisabledのまま。

バックアップ・詳細な原記録は各node rootの`backups/api-latency-slack-20261108T053235Z/`に保存。
Piでは旧resident、設定、identity manifestとSQLite online snapshotを保持した。
原記録はノード外へ一括転送せず、集計manifestを各node rootの
`current_state/deployments/api-latency-slack-2026-11-08.json`に保存した。
個体DBの巻き戻し、commit/pushはしていない。

広い推論品質評価、障害の全組合せ、長期間の連続稼働は未検証。
補助的な外部批評はツール終了コード1で取得できず、独立したローカルread-only調査と上記回帰・実機で確認した。
