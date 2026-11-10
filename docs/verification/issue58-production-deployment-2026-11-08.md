# Issue #58 本番配備（2026-11-08）

## 配備元と保全

- master `147d7b430434ba994b1eaa06870659cb7aae6eb2` のP0子プロセス隔離を反映。
- 現本番は旧master＋主推論・Git読み取り・認証cooldown修正だったため、素のmasterによる退行を避けた。
  既存配備記録 `primary-agent-2026-11-06.json` のoverlay8ファイルは現checkoutと全hash一致。
  その8ファイルを保全してmasterへ重ねた独立ソースから、Mac/Piでネイティブreleaseビルド。
- 転送archiveのSHA-256：`c5078c3336b9cb35528fdc9cab31194ec1372ce3e925dba87e8ef129dfb1944c`。
  Piでarchiveおよびoverlay全件の一致を確認。
- source snapshot作成後の別作業者によるCargo/Slack変更は配備に含めない。
- 更新対象はresidentとruntime。k-core・computer MCP・個体SQLite・認証情報・サービス権限を置換しない。

## Mac

既存LaunchAgentを継続し、両binをバックアップしてatomic置換。
既存 `service_control` 自己再起動のticketが `observed_restart` に復帰した。

- 起動番号：47→48。
- backup：`~/Library/Application Support/Kamimusuhi/node/backups/issue58-147d7b4-20261107T165456Z`。
- 再起動前後の個体identity/root/head一致、configとsecretsファイルのstat不変を確認。
  secretsの内容は読んでいない。
- 固定releaseのfake CLI init/talkとworkerプロトコルを配備前に確認。
- 配備後の実対話でGit読み取り3回成功。README先頭の実見出しが回答に一致。
  観測したprovider-workerは1件、応答後の残留PIDは0。
  routeはPi経由qwen3.8-27b-uncensored、約19.4秒。投稿・書込み・外部executor委譲なし。

## Pi

独立ソース：`/home/eightman/src/kamimusuhi-issue58-147d7b4-20261107T165456Z`。
release buildは10分24秒。完成binは同dirの`release-artifacts/`へ固定コピーし、後続テストとの競合を防いだ。
配備前にnative transport6件、NoNewPrivileges下で80回停止→正常復帰、通常CLI factory1件がすべてPASS。
後者は固定releaseに対して、同じcommitted CLI試験を直接リンクして実行した。

- consumer receiptは停止前後ともdelivered、進行中taskなしを確認して一時停止。
- 両binをbackup後atomic置換し、service_control ticketがobserved_restartへ復帰。
- 起動番号：62→63。authority=true、NAS正常、K-CORE runningを確認。
- backup：`/srv/kamimusuhi/backups/issue58-147d7b4-20261107T165456Z`。
- 再起動前後の個体identity/root/head一致、configとsecretsファイルのstat不変を確認。
- consumer再開後active、resident active、NoNewPrivileges=yesを維持。
- 配備後の実LLM対話でGit読み取り2回成功。README見出しの実読結果が回答に一致。
  Mistral経由deepseek-v4-1-flash-1017、約16.1秒。provider-worker1件を観測し残留PID0。
  投稿・書込み・外部executor委譲なし。

## バイナリ識別

| ノード | resident SHA-256 | runtime SHA-256 |
|---|---|---|
| Mac | `bd36fcce68920bbf06bdb839605ae3768e284c012d4d6304ce01310069897162` | `ca8a5ce5eea14179a12f23eb298acc9a3551d591145a0ca56a887b825695a8f7` |
| Pi | `5fb09471b8d138a5a967ce112000464356e95d4b366bf49701b5a8dee216a892` | `3d07fbbebd4cc258887ca33ed18cb03a4dcb20a22bcdb0742ca36343c5de56f4` |

## 証跡

各ノードの私有領域 `current_state/deployments/issue58-provider-process-2026-11-08.json` に
配備元、overlay、backup、bin hash、restart ticket、identity保持の検証を保存済み。
実対話の応答・tool証跡・worker PID観測は同名の`-live.json`へ保存済み。
本記録には認証値・利用者の会話本文を含めない。

## 復元と範囲

必要時は各backupの2binを同じruntime/binへatomic復元し、既存service_controlで再起動する。
個体DBは復元・初期化しない。今回の更新で設定・資格情報・権限は変更していない。
本番を故障注入で止める試験は行わず、同一固定成果物の隔離fixtureで永久停止を確認した。
実APIの本番対話ではworker起動・回収と既存の直接Git調査を確認した。
この記録と配備作業用ファイルは未コミット。今回、新たなcommit/pushは行っていない。
