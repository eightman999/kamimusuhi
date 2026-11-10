# マージ済みmasterのMac/Pi配備（2026-11-06）

## 配備元と方式

- master: `352a6e8a4c7feaea84229a2f6b54c3d54fc41f5d`（PR #81マージ）。
- Macは当該checkout、Piは当該HEADの`git archive`を独立領域へ展開してネイティブreleaseビルド。
- ソースarchiveのSHA-256は両端一致：`70cc9cbeea78068e26ab2ee55d6db8698bae1c33dead4896c95b5f3a5256a02d`。
- Piビルド領域：`/home/eightman/src/kamimusuhi-master-352a6e8`。既存の編集済みGit repoは上書きしていない。
- 両ノードのresident/runtimeバイナリを既存版のバックアップ後にatomic置換。
- Macの既存配置先にあるcomputer MCPもビルド成果物へ更新。
- 個体の初期化、認証情報変更、マシン本体rebootは行っていない。

## 再起動

`service_control restart`で受付後、同じtoolのticketが`observed_restart`になることを確認。

| ノード | 起動番号 | バックアップ |
|---|---|---|
| Mac | 44 → 45 | `node/backups/master-352a6e8-20261106T050730Z` |
| Pi | 60 → 61 | `/srv/kamimusuhi/backups/master-352a6e8-20261106T052357Z` |

Macの`node`は`~/Library/Application Support/Kamimusuhi/node`。
PiのAgentHub workerは実行中タスクがないことを確認して一時停止・再開。
Readyの待機タスクは変更していない。residentとworkerの両方がactive。
Piの`NoNewPrivileges=yes`を維持。

Piのruntime許可一覧に`curl`・`browser`・`self_improve`が欠けていた。
自動承認レビューで永続的許可変更が一度拒否されたため保留し、ユーザーの
「3ツールの利用許可追加を承認」を受けた後、runtime設定をバックアップして3項目だけ追加した。

## 再起動後の実測

すべて稼働residentの認証付き`POST /v1/tools/call`経由。

| ツール | Mac | Pi |
|---|---|---|
| curl | Agora HTTPS 200、JSON POST、404でも本文保持 | 許可済みloopbackのhealth取得 |
| git | 登録repoのstatus、既存originへのfetch | 登録repoのstatus、既存originへのfetch |
| service_control | 実再起動・ticket復帰観測・status | 実再起動・ticket復帰観測・status |
| browser | 実Chromiumの表示・snapshot、入力・クリック・JavaScript反映 | Macへ委譲して公開ページ表示・snapshot・close |
| self_improve | 既存成功タスクのstatus、未知IDのapply拒否 | Macへ委譲して同じstatus・apply拒否 |
| Agora | 登録維持、公開一覧取得、投稿済み自己紹介のブラウザ表示 | 未登録。ホスト未許可による一覧取得拒否を確認 |
| AgentHub | 認証file未設定による拒否を確認 | Mio本人認証、一覧・検索・詳細・ページング・割当filter成功 |

Macの自己改善用専用ソースは、Rustソース・Cargo設定等224ファイルがmasterと一致。
今回、新規の有料executorジョブ、コード改変・テスト・評価・applyの一連の再実行、
外部Gitへのpush、Agora再投稿は行っていない。
自己改善の一連の成功実測は`mac-web-self-improvement-2026-11-06.md`を参照。
今回Git書き込み・push fixtureを再実行せず、既存実測とマージ前CIを利用した。

外部Agoraリンクのclickは遷移待ちで一度timeoutした。直接navigate/snapshotは成功し、
入力・click・JS反映は制御したloopbackページで成功した。検証ページと一時HTTPサーバーは停止済み。

## バイナリ識別と証跡

| ノード | resident SHA-256 | runtime SHA-256 |
|---|---|---|
| Mac | `3f140ea64aebc06ecd018d9a27259192710fb574e09e3d8969e1dc3f9a34d22a` | `dc6d94af3e6eec5b76603e545a3c5612bcc8a162de03878fe0bb770115246897` |
| Pi | `52fb9d2c737df857dd6789f74990461bf8bd53e3d8be40bd331c4180badbcef7` | `d0d4ec3814a0a8bbae66554fd17693ba1acd70c53d47c59b16ea51b7376864da` |

ノード私有領域`current_state/deployments/master-352a6e8.json`に配置情報、
`master-352a6e8-smoke.json`にツール実測を保存。認証値は報告・本記録に含めていない。
設定やサービスの認証範囲は従来のノード分担を維持する（AgoraはMac、AgentHubはPi）。
