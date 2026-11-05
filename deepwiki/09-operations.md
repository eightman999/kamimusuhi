# 09 — 運用: デプロイ・secrets・NAS・障害時挙動

対象: `deploy/resident/`（README.md, install-*.sh, *.service, *.example.json,
site.env, push-secrets.sh, discord_bot.py, monitoring/）

## 参考構成（実運用の写し）

```text
             NAS (NFS export → /mnt/kamimusuhi)
          ↗       ↖
        pi ←────→ llm_master            peer: tailnet IP :7860 (/health)
        │            │
   continuity     cognition
   scheduler      local LLM (llama.cpp :8080)
   K-CORE監督     GPU状態・Task Plane
   state/snapshot
        └──── HAI ───┘  https://hai-api.hcloud.ltd/v1 (任意の外部 tier)
        └ mac (cognition・Task Plane・computer use・desktop/TUI の置き場)
```

| ノード | role | 担当 | 書くもの |
|---|---|---|---|
| pi | `continuity` | 24h継続・scheduler・K-CORE監督・authoritative state・routing | `current_state/`・個体DB・NASの `*/pi/` |
| llm_master | `cognition` | local LLM・GPU・将来の reflection/embedding/学習評価 | NASの `*/llm_master/` のみ |
| mac | `cognition` | Task Plane（Devin/OpenCode等）・GUI・computer use | ローカルのみ（NAS 不使用・正本なし） |

- 正本 state（個体DB）は continuity ノードだけが持つ。cognition ノードは個体を
  初期化・更新しない。
- NAS journal は `<category>/<node>/<date>.jsonl` のノード別パス — 2ノードが同じ
  ファイルへ追記しない single writer 規則。
- resident は個体を**初期化しない**。初期化は `install-node.sh` が
  「DB も runtime.json も無い場合のみ」一度だけ行う。

## 導入の型（install スクリプト）

サイト固有値（ホスト/IP・NAS・リポジトリ名）は `deploy/resident/local/`（git 管理外）へ。
追跡されるのは `*.example.*` と `site.env.example` のみ。

```bash
# 1) ソースをノードへ（例: Mac から）
rsync -az --exclude target --exclude .git Cargo.toml Cargo.lock rust-toolchain.toml crates deploy knowledge <host>:src/kamimusuhi/
# 2) root 作業を一度だけ（ノード上）
sudo ~/src/kamimusuhi/deploy/resident/install-root.sh <node>
# 3) secrets（値は ssh stdin のみで転送。ファイルに残らない）
deploy/resident/push-secrets.sh <host>
KEY_FILES="GROQ_API_KEY=<file>" deploy/resident/push-secrets.sh <host>   # 追加キー
# 4) ビルド・配置・起動（ノード上, 要 rustup）
~/src/kamimusuhi/deploy/resident/install-node.sh <node>
# 5) NAS 初回だけ
mkdir -p /mnt/kamimusuhi/{memory,conversations,logs,artifacts,experiments,datasets,models,checkpoints}
touch /mnt/kamimusuhi/.kamimusuhi-nas
```

- `install-node.sh`/`install-root.sh` は任意の node id（`[a-z0-9_]+`）を受ける —
  pi/llm_master 以外の追加ノード（6f・tb250 等）も同じ手順。
  `k-core` は continuity ノードにだけ配置される（config の `node.role` で判定）。
- mac 向けは `install-mac.sh`（release build → `~/Library/Application Support/Kamimusuhi/node`
  → LaunchAgent。再実行で更新、`--uninstall` で解除）。
  launchd 配下の resident は macOS のローカルネットワーク許可が無いと LAN 宛てに出られないため、
  peers の `alt_urls` に tailnet を入れておく。
- systemd unit: `kamimusuhi-resident.service`（`Restart=always`）・`kamimusuhi-discord.service`。
  rsync -a で古い target の rlib/rmeta が混ざる時はノード上で
  `find crates -name '*.rs' -exec touch {} +` してから再ビルド。

## secrets の置き方

- `/srv/kamimusuhi/config/secrets.env`（mode 600・git 管理外）。
  resident.json には値ではなく**環境変数名**を書く（`auth_env`・`token_env`）。
- `KAMIMUSUHI_NODE_TOKEN`: ノード・クライアント共有の bearer。
- 各 provider key（`HAI_API_KEY` 等）は `push-secrets.sh` の `KEY_FILES` で ssh stdin 配布。
- GitHub MCP 用 token は `GITHUB_TOKEN_FILE=` 経由（fine-grained・read-only・期限付き推奨）。
- Obsidian mirror 用の git credential は `GIT_CONFIG_*` 環境変数で git に渡し、
  argv・`.git/config`・出力には出さない。

## NAS と spool の規則

- マーカー `.kamimusuhi-nas` がある時だけ NAS 扱い — 未マウントで空のマウントポイントが
  見えている時に SSD へ誤書き込みしない。mount は `soft,nofail,x-systemd.automount`。
- journal は追記配送（at-least-once・行ごとに一意 `id`）。snapshot 等は tmp+rename。
- `current_state/*.json` も `state/<node>/` へ定期的にステージ配送 —
  SSD 全損は「イベントジャーナル + 最新 state の複製」でカバー。

## 監視

- `/metrics`（Prometheus, `kamimusuhi_usage_*`）と `/v1/usage`、`/v1/usage/history`。
  Grafana ダッシュボード・収集設定は `deploy/resident/monitoring/README.md`。
- `kamimusuhi status`（exit 0/1/2）は peers・tiers・NAS（書込 probe・spool backlog・
  最終同期）・HAI 残高・K-CORE・snapshot・commitments escalated を表示。

## 障害時の挙動

| 事象 | 挙動 | 復帰 |
|---|---|---|
| continuity ノード再起動 | systemd `Restart=always`、boot_epoch +1、K-CORE 再開。in-flight 委譲は `retry_pending` へ | 自動 |
| cognition ノード停止 | routing が他 tier へ、peer は DOWN 表示。queue 中の委譲は `retry_pending` 待機（light 側 event loop は止まらない） | probe 成功で自動・冪等再送 |
| NAS 断/ハング | spool に蓄積、probe は別スレッド timeout | マーカー確認後に自動配送 |
| 外部 tier 断 | 他 tier のみ。全滅なら 503 | probe 成功で自動 |
| MCP server 死亡 | backoff で再起動 | 自動 |
| 個体 runtime プロセス timeout/クラッシュ | `received` + `failed` が journal に残る | 次ターンは新プロセス |

## 設定変更の運用

- `kamimusuhi check-config` で `resident.json` を検証してから再起動。
- `task_plane.executors_file`・runtime.json の `avatar_name`/`avatar_aliases`/
  `tools.allowed` は**再起動不要**（変更検知・ターンごと読み込み）。
- 再起動は実行中 Task Plane タスクを中断させる — executors 変更は file 経由にする。

次: [10-fio.md](./10-fio.md) — 参照系 FIO との対応。
