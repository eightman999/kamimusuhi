# API推論の本番反映と実機検証（2026-11-08 JST）

追補: Piの経路遅延修正とSlack実往復は
[追加検証記録](pi-api-latency-slack-2026-11-08.md)を参照。

ユーザーの明示承認を受け、Mac/Pi両方で会話中のAPI判断・評価と待機中のAPI内省を有効化した。
residentの本番HTTP経路、実API、実DBへのReflection保存まで確認した。
fixtureだけで確認した[実装時の検証](idle-cognition-2026-11-08.md)とは別の記録。

## 配備

- 配備タグ: `api-cognition-20261108T043552Z`。
- 元HEAD: `147d7b430434ba994b1eaa06870659cb7aae6eb2`＋既存の未コミット変更を含む凍結ソース。
  HEADだけを再ビルドしたものではない。
- ソースarchive SHA256: `9a3e2ffc30c0a754502dbb862e9279fbc0b054377f03e7564f02c899a2f060b8`。
- Macはnative release、Piは専用source領域でRust 1.92.0/aarch64の
  `cargo build --locked --offline --release`を実行。Pi buildは13分59秒で成功。
- 変更した本番ファイルはresident/runtimeの2binとresident設定の`cognition`。
  `subjects: ["eightman"]`、巡回30秒、process期限180秒。
- runtime設定はbyte hashを維持。Jev未設定のAuto判断はprimary APIへ接続される。
  待機policyの既定値は無会話60秒、最短10分間隔、再試行10分後・最大3回、日次24cycle。
  cycle数は個別API要求数の上限とは異なる。

| Node | resident SHA256 | runtime SHA256 |
| --- | --- | --- |
| Mac | `bdbc38bd3ad6bb38ceb4de48a5b587aa60e5f244e0db3b235ce732a4d5b0e839` | `3500444571d31b9daee91a4d9535b14e00b2b14602a41ebdb1ccccf4ce48fcc0` |
| Pi | `52184de883d0868930a62b205a1cfc173c4819c74e08d394ba593fd6a9c2cbe6` | `1b2377052a18e4fd92e834e6621eac6e2df01a2c5244443b8ccff24896d0f6d2` |

各ノードで9件のidentity構成差分を確認し、対応するexact review tokenで一度だけ明示承認した。
自動承認は追加していない。現在のidentity状態は両方`continue`。
boot epochはMac 51→52、Pi 67→68。個体ID・root・continuity head・generationは維持。
residentの他設定、runtime設定、資格情報のstat metadataも維持した。

## 実APIによる会話中の推論

本番residentの`POST /v1/kamimusuhi/talk`を専用smoke subjectで各1回実行した。
「AならB、BならC、Aは真」からCが真と結論できるか、外部ツールなしで回答させた。
両方とも正しく肯定した。これは単一の動作確認であり、推論品質のベンチマークではない。

| 確認項目 | Mac | Pi |
| --- | --- | --- |
| HTTP / 全体時間 | 200 / 5.199秒 | 200 / 39.266秒 |
| 発話前判断 | API `SPEAK` | API `SPEAK` |
| 生成後評価 | API `ACCEPT` | API `ACCEPT` |
| 評価の根拠 / 要求充足 | `SUPPORTED` / `MET` | `SUPPORTED` / `MET` |
| 判断の規則fallback | 0 | 0 |
| tools / 残留worker | 0 / 0 | 0 / 0 |
| 最終生成route | `mistral` / `deepseek-v4-1-flash-1017` | 同左 |

DBの`response_generated`に保存されたconversation traceで、発話前判断・候補評価のproviderが
`openai-compatible-decision`、両gateの`fallback: false`、`decision_fallbacks: []`を確認した。
応答が返るだけの確認ではなく、判断→生成→評価を通ったことを確認している。
Piでは発話前のAPI判断に約32秒を要した。待機内省でも下記の経路failoverによる遅延を確認した。
規則判断への縮退とは区別する。この遅延を改善する経路設定変更は今回行っていない。

## 実スケジューラによる待機中の推論

`eightman`の既存実会話を対象に、residentの周期実行から入力なしの`cogitate`が起動した。
両ノードともcheckpointは初回試行で`COMPLETED`、sourceとconsumedのdigest一致、in-flight解除。
DBのReflectionとsource link、実API利用記録を照合した。内省本文はこの文書に転記していない。

| 確認項目 | Mac | Pi |
| --- | --- | --- |
| Reflection追加 | 1 | 1 |
| 内省に提示したepisode | 8 | 32 |
| 生成note / canonical draft | 4 / 3 | 7 / 5 |
| 実APIの成功route | `pi` / `deepseek-v4-1-flash-1017` | `mistral` / `deepseek-v4-1-flash-1017` |
| API要求時間 | 35.785秒 | 42.606秒（先行経路の失敗を含む） |
| sourceのsubject | `eightman`のみ | `eightman`のみ |

draft数は提出数であり、正典への自動採用数ではない。
API経路のtier名やbilling表示だけからローカルモデル実行・請求額を推測していない。

各ノードで完了後35秒間、次の実pollを含めて観測した。
追加API利用記録0、追加Reflection0、checkpointと日次budgetの変化0。
`NO_NEW_EXPERIENCE`で待機し、内省自身を新しい経験として再送しなかった。
追加のsession/turn・ユーザー発話・エージェント発話は会話smoke各1件分だけで、内省は捏造しなかった。

## サービスと記録

- Mac residentはLaunchAgentでrunning。Mac Slackのdisabled設定を維持した。
- Piのresident・Slack・AgentHub workerはactive/running/enabled、`NoNewPrivileges=yes`。
  検証中に停止した窓口を復帰し、Slackの`Socket Mode connected`を確認した。
  Slackへの試験メッセージ投稿・実往復は行っていない。
- 最終確認時、両方のcognitionは`NO_NEW_EXPERIENCE`で待機中。

バックアップと詳細な原記録は、各ノードの次の場所に保持する。旧2bin、resident/runtime設定、
identity manifestとSQLite online snapshotを保存した。DBを旧snapshotへ巻き戻してはいない。

- Mac: `/Users/eightman/Library/Application Support/Kamimusuhi/node/backups/api-cognition-20261108T043552Z/`
- Pi: `/srv/kamimusuhi/backups/api-cognition-20261108T043552Z/`

配備manifestは各node rootの`current_state/deployments/api-cognition-2026-11-08.json`。
会話・内省の詳細を含むJSON原記録の一括転送は自動承認レビューに拒否されたため行わず、各ノード内に保持した。
この文書には集計した検証結果だけを記載した。

実装時は関連5crateで**948 PASS / 0 FAIL / 6 ignored**、fmt/clippy、Mac releaseが成功。
今回追加した実機確認とPi native buildも成功し、文書差分の`git diff --check`はPASS。
長期連続稼働、広い推論品質評価、Slack経由の会話往復は未検証。commit/pushはしていない。
