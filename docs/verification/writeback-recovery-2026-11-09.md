# 失敗書戻しの監査付き解消と本番検証（2026-11-09 JST）

ユーザーの明示承認に基づき、前日の本番E2Eで残ったMac/Pi各1件の未完了を解消した。
元markerと入力を保持し、`memory_writeback_aborted`を各1件追加した。
**両ノードとも未完了0・失敗終端1。成功書戻しの捏造や履歴削除は行っていない。**

実装sourceは`3fbe19d085fdc51c462b1383f8021669b391b8e2`。
[操作・排他・拒否条件](../implementation/memory-writeback-recovery.md)と
[集計証跡](../../evaluation/continuity/writeback-recovery-20261109.json)を参照。

## 操作と独立した読戻し

対象は前日のJSON小数往復の不具合で、UserUtteranceだけが残った既知のturnに固定した。
本番のmarker hash・元証跡・個体/head・writer epoch・設定・資格情報metadataを事前にsnapshotした。
reviewでDBに変化がないことを照合し、返却tokenと同じ理由でabortを実行した。

独立したSQLite read-only接続で、次を確認した。

- 追加イベントはmarkerに予約されたcompletion IDの`memory_writeback_aborted`1件だけ。
- `actor=operator`、理由、元marker hash、元証跡snapshot digest・IDを記録。
- 元入力への`derived_from` lineageが1件あり、元入力の行・payloadは不変。
- 同一token/理由の再実行で、監査行・lineageとも増えない。
- 対象turnに`memory_writeback_verified`は作られていない。
- 元markerを同じ場所にbyte不変で保持し、statusが監査・元証跡・lineageを再検証する。

失敗理由は運用者のレビューに基づく説明で、当初の例外全文を新しく観測したという意味ではない。
発話行がないことを「人間へ一度も表示されなかった」証明としては扱わない。

## 本番結果

| 確認 | Mac | Pi |
|---|---|---|
| 未完了 before → after | 1 → 0 | 1 → 0 |
| 別集計の失敗終端 | aborted=1 | aborted=1 |
| 元marker / 入力 | 保持・不変 | 保持・不変 |
| 失敗監査イベント / lineage追加 | 1 / 1 | 1 / 1 |
| 成功イベント捏造 | 0 | 0 |
| 同じabortの再実行 | no-op | no-op |
| 解消後の実モデル応答 | 確認しました。 | 確認しました。 |
| 解消後の会話時間 | 25.691秒 | 24.140秒 |
| 解消後の通常writeback verified | 1件 | 1件 |
| decision fallback / tools | 0 / 0 | 0 / 0 |
| resident再起動後 | unresolved=0 / aborted=1 | unresolved=0 / aborted=1 |
| 最終boot epoch / FI3 | 59 / continue | 75 / continue |
| pending task / provider worker残留 | 0 / 0 | 0 / 0 |

解消後の会話は新しい試験専用subjectで各1回だけ送信した。
APIの発話前判断・応答評定と、DBの`memory_writeback_verified`を確認した。
続いてresidentを再起動し、新しいプロセスからstatusを検証した。
個体・root・continuity head・writer epoch・設定byte hash・資格情報metadataは保持された。
Piのresident/Slack/AgentHub workerはactive、`NoNewPrivileges=yes`も保持した。
Slackなどへの試験投稿は行っていない。

これは未完了の解消と会話・保存経路の検証で、人手による文体・関係性評定や自己モデル成長の実証ではない。

## ビルド・配備・復旧用証跡

無関係な作業ツリー差分を除き、Cargo/crates/knowledge/deployの追跡済み288ファイルをsource commitとbyte照合した。
Mac/Piとも同じarchiveを使用し、Piでも展開後の全288ファイル一致を確認した。

- source archive SHA256: `2a2e857b209fe756f492abb685a009ab39005a09bc94424ff142a3da36634a16`
- native release: Mac 36.60秒、Pi 8分44秒。
- 各node rootのbackup: `backups/self-memory-abort-20261109/`。
- 配備はresident/runtimeの2binのみ。元2bin・設定・identity・SQLite online snapshotをbackupした。
- FI3の変更は`compiled_memory_writeback_implementation`1項目だけ。exact review tokenで明示acceptした。
- 各nodeの`current_state/deployments/self-memory-continuity-2026-11-08.json`を今回のsource・解消・会話・再起動証跡へ更新。
  旧manifestも上記backup内に保持した。

| node | resident SHA256 | runtime SHA256 |
|---|---|---|
| Mac | `4c1206c657fd1a21da730f49c84597882196bc8c0ee6213d0308c48757626c90` | `55221804bc5344389f64e8918ba4d4eb801470fcc676846039ec4c83b64abb67` |
| Pi | `3d81ad403bc19f23c73ad7dfc5b8aa5a2e6cf090d4e98165972b47816e099315` | `ecdcee83ffba44384037e412df428e1b3b495dded5bee0b3d333e5cdb85d2896` |

障害時にDBを過去のsnapshotへ戻して会話や監査を消さない。
bin復元時にもFI3差分をreviewする。配備前の旧binは新abortイベントを認識せず、
保持したmarkerを再び未完了と表示する。監査とmarkerを消さず、対応版で状態を確認する。

## 検証結果とCIで見つかったfixtureの前提

新しいAPI/CLIテスト11本を含み、限定sourceのstore/runtimeは **548 PASS / 0 FAIL / 6 ignored**。
Clippy全target `-D warnings`・fmt・差分検査もPASSした。
最初の限定sourceではresearch検証用の参照文書が不足していたため、追跡済みの13文書だけをHEADから補充し、
失敗・未実行の対象を再実行した。実験文書の値やhashを書き換えて通していない。

初回の[K-CORE CI](https://github.com/eightman999/kamimusuhi/actions/runs/42492788797)はPASS。
[Rust CI](https://github.com/eightman999/kamimusuhi/actions/runs/42492788618)では既存
`failed_retry_never_reuses_old_or_other_candidates`が1件失敗した。
即時HTTP応答の2候補が10msのready batchに必ず揃うというfixtureの仮定により、
CI負荷で片方が遅れると固定judge回答が実際の候補集合と一致せず、retry前に拒否される。
本番のraceやabort実装は変えず、このテスト専用の判断providerと上限付きCondvarで
primary先行・backup先行の両方を明示的に作るようにした。
初期生成・再生成のHTTPは維持し、3種の再生成失敗それぞれで旧候補の再利用・再審査・配信がないことを確認する。
非選択候補のwire上のACCEPTは元からhostのResponseAssessmentに保持されないため、
このテストは成功したbackup生成物が残っていても失敗後に再利用しないというhost側の契約を検証する。
残る18本のwire契約テストは変更していない。fixture修正はテストだけのため、上記本番binaryの再配備は不要。
修正後の`llm_jev_loop`は **19 PASS / 0 FAIL**、対象Clippy `-D warnings`・fmt・差分検査もPASS。
