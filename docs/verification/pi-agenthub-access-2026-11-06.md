# Pi：AgentHub Cloud Boardアクセス（2026-11-06）

## 目的と境界

PiのMioからCloud Boardの一覧・検索・詳細を参照する `agenthub` ツールを追加。
既存 `agenthub-mio-worker.service` とWorker Bearerを再利用し、新規登録・token発行・
claim/result・Pending提案・承認・取消は行わない。Web画面やlegacy snapshotを読む
Basic操作者認証も利用しない。

所有ソースと公開OpenAPIを確認し、Piの既存Bearerで
`identity={id:mio,role:worker}`、8件取得、Mioの実行中なしを事前確認した。
GETでも期限切れRunningをUncertainへ遷移させるのは既存サーバーの動作である。

## 実装とローカル検証

- `status/list/search/get/read` を固定HTTPS endpointへ接続。
- privateな認証ファイルを内部で読む。tokenは切り詰め前に除去。
- 取得はtransport上限8MiBまで。完全JSONを型・値で検証してからviewを32KiB以下に整形。
- 通常HTTPツールの32KiB上限とホスト許可境界は維持。
- APIの最大2000件取得窓と、不完全な可能性を結果内でも明示。
- 関連9テスト、resident全回帰209件（205単体＋4統合）、Clippy、書式チェックが成功。
- 所有APIとの独立した読み取り照合でも、schema/認証の不一致を認めなかった。

## Pi配置

既存バイナリと対象設定を
`/srv/kamimusuhi/backups/agenthub-access-20261106T024258Z` に退避。
元のPiソースは上書きせず、コードのみを専用ビルド領域へ転送した。
ソースアーカイブSHA-256:
`4d05573e97d4739e109d8d9e07f5b10b08abdcb8f01d9d11dc1496ef32a19bfe`

Piの既存Rust 1.92.0を明示し、1 build job・既存release cacheでネイティブビルド。
配置時はMioのReady/Running/Uncertainがないことを確認し、既存consumerを一時停止して
residentを更新、healthと認証付きstatusの成功後にconsumerを再開する。

## 配置・実API結果

ARM64 releaseビルドはRust 1.92.0で成功（13分23秒）。新residentのSHA-256は
`015b6529f073db32f3c652f6e7f8ffba93a4742c2b3cd36eb29def944af224af`。
Piへ配置し、healthのnode=pi / boot_epoch=58を確認した。

稼働 `POST /v1/tools/call` 経由の実測：

- `status`: `identity={id:mio,role:worker}`、8件。
- `list`: 2件単位で取得し、次ページと重複しないことを確認。
- `search`: 一覧のtaskタイトルから検索し、同じtask idを確認。
- `get`: 検索対象taskの詳細を取得しid一致を確認。
- `list assignee=mio`: 4件、すべてMio割当。
- 対象外の`claim`はローカルで拒否。
- API結果と新規HTTP journalにworker/resident認証値が含まれないことを確認。
- 認証ファイルの0600維持、runtime側のallowed登録を確認。
- residentと既存 `agenthub-mio-worker.service` は両方active。

新規登録・認証再発行・ボードへの投稿・タスク操作・個体再初期化はしていない。
Pi証跡：`/srv/kamimusuhi/current_state/deployments/agenthub-access-2026-11-06.json`。
元の開発checkoutへのcommit/pushとMacサービス更新は行っていない。
