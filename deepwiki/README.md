# Kamimusuhi DeepWiki

このディレクトリは、Kamimusuhi リポジトリの**現在のコードベース**から生成した
構造化ドキュメント（DeepWiki）です。正本はコードと `spec.md` / `architecture.md` であり、
ここは「今どうなっているか」を最短で把握するための読み物です。

> English note: this wiki is written in Japanese, the project's primary language.
> For an English entry point see `../README.en.md` and `../spec.en.md`.
> The API reference (page 03) and quickstart (page 02) are mostly self-explanatory
> JSON/HTTP and shell snippets.

## ページ索引

| ページ | 内容 |
|---|---|
| [01-overview.md](./01-overview.md) | プロジェクトとは何か・現在の姿（常設ノード群と個体）・リポジトリ地図 |
| [02-quickstart.md](./02-quickstart.md) | **tailnet 内の1台に resident を立てて API で使う最短ルート**（導入はここから） |
| [03-api-reference.md](./03-api-reference.md) | resident HTTP API の全エンドポイントと CLI 対応表 |
| [04-resident.md](./04-resident.md) | resident デーモンの内部: 設定、ルーティング、プローブ、spool、メトリクス |
| [05-individual.md](./05-individual.md) | 個体 runtime: continuity kernel・証拠/記憶ドメイン・対話ターン・persona envelope |
| [06-peers.md](./06-peers.md) | 複数ノード構成: peers/alt_urls・light/heavy 委譲・intercom と relay |
| [07-tasks-tools.md](./07-tasks-tools.md) | タスクボード・Task Plane（外部エージェント）・Commitment Engine・承認/審査・MCP/Library |
| [08-surfaces.md](./08-surfaces.md) | 接続面: デスクトップ GUI・TUI・CLI・Discord bot |
| [09-operations.md](./09-operations.md) | デプロイ構成・systemd/launchd・secrets・NAS・障害時挙動 |
| [10-fio.md](./10-fio.md) | 参照系: FIO 記憶アーキテクチャの実測と Kamimusuhi への写像 |

## 読み方の目安

- **まず動かしたい**: [02](./02-quickstart.md) → [03](./03-api-reference.md)
- **構造を理解したい**: [01](./01-overview.md) → [04](./04-resident.md) → [05](./05-individual.md)
- **複数台で組みたい**: [06](./06-peers.md) → [09](./09-operations.md)
- **個体に仕事を任せたい**: [07](./07-tasks-tools.md)

## 規範文書との関係

| 層 | 文書 | 役割 |
|---|---|---|
| 規範（正本） | `spec.md` / `architecture.md` / `model-ecology-architecture.md` | 変わらないはずの要求・境界 |
| 実装記録 | `docs/implementation/`・`deploy/resident/README.md` | 実装済みの事実と運用 |
| この deepwiki | `deepwiki/` | コードから逆引きした現状地図 |
| 調査知見 | `docs/` その他 | 採用・棄却・仮説の記録 |

矛盾を見つけたらコードが正です。deepwiki の記述は各ページ冒頭の対象ファイルで確認できます。
