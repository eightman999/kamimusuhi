# 03 — Resident HTTP API リファレンス

対象: `crates/kamimusuhi-resident/src/server.rs`（ルーティング）,
`dialogue.rs`, `tasks.rs`, `approvals.rs`, `commitments.rs`, `intercom.rs`,
`library.rs`, `delegation.rs`, `tools.rs`, `task_orchestrator.rs`

resident の HTTP は最小実装の HTTP/1.1（1接続1スレッド・`Connection: close`・
header 64KiB/body 16MiB 上限・同時 64 接続・読み書き 30 秒 timeout）。
すべて JSON。**OpenAI 互換の部分を除き、レスポンスは機械向けの素の JSON**。

## 認証

- `GET /health` と `GET /capabilities` は**無認証**（監視・発見用）。
- loopback（`127.0.0.1` / `::1`）からのリクエストはトークン不要。
- それ以外は `Authorization: Bearer <KAMIMUSUHI_NODE_TOKEN>`（環境変数は
  `node.token_env` で変更可）。トークン未設定の resident では
  非 loopback の特権 API は全部 401。
- `POST /v1/approvals/decide` だけは **loopback でもトークン必須**
  （ローカルのブラウザ等に承認させないため）。

補助ヘッダ:

| ヘッダ | 意味 |
|---|---|
| `X-Kamimusuhi-Route: local` | chat/completions を `node_local` tier のみに制限（peer 呼び出し時の fallback 抑制） |
| `X-Kamimusuhi-Turn: <id>` | 上流 LLM 呼び出しを呼び出し元ターンへ帰属させる相関 ID |
| 応答 `X-Kamimusuhi-Tier` | 実際に応答した tier 名（`none` もあり得る） |

## エンドポイント一覧

### 無認証

| メソッド・パス | 内容 |
|---|---|
| `GET /health` | ノード要約（id・role・capabilities・健全性）— peer 発見と監視を兼ねる |
| `GET /capabilities` | `{"node_id","role","mode","capabilities":[…]}`。light/heavy 委譲の宣言面 |

### 監視

| メソッド・パス | 内容 |
|---|---|
| `GET /status` | 完全ステータス（peers・tiers・NAS・spool・K-CORE・commitments の escalated・intercom 等）。`kamimusuhi status` の元 |
| `GET /metrics` | Prometheus テキスト（`kamimusuhi_usage_*`） |
| `GET /v1/usage` | モデル利用スナップショット（会話本文は含まない） |
| `GET /v1/usage/history?limit&since` | 利用履歴（limit 1–1000, since Unix秒） |

### OpenAI 互換 proxy（ルータ）

| メソッド・パス | 内容 |
|---|---|
| `GET /v1/models` | healthy tier 一覧 + `kamimusuhi`/`auto` 自動ルート名。`X-Kamimusuhi-Route: local` で node_local のみ。全 tier down は 503 |
| `POST /v1/chat/completions` | OpenAI 互換ボディを RouteGate で tier へ。`model: kamimusuhi`/`auto`=自動、`model: <tier>` または `<tier>/<model>`=固定、`model: k0`=local のみ。`stream:true` は完成応答を SSE 1 回で返す（逐次生成ではない） |

### 個体との対話（continuity ノードのみ）

| メソッド・パス | 内容 |
|---|---|
| `POST /v1/kamimusuhi/talk` | `{"subject": "…", "message": "…"}` → `{"response","individual_id","turn_id","latency_ms","route","tool_calls","reference_lookups"}`。message は 1〜32KiB、subject は `[A-Za-z0-9._-]` 1–64 文字。subject は履歴の区別であって認証ではない。`dialogue` 未設定なら 404、個体未初期化なら 503 |
| `POST /v1/kamimusuhi/history` | `{"subject":"…","limit":≤200}` → 直近ターン（`received`/`answered`/`failed` を含む。応答不能に死んだターンも跡が残る） |

同時実行は `dialogue.max_concurrent`（既定2）まで、**同じ subject は到着順に1件ずつ**
（TurnGate）。個体への書き込み自体は runtime ディレクトリの `writer.lock` で直列化。

### タスクボード / Task Plane / Commitments

| メソッド・パス | 内容 |
|---|---|
| `GET /v1/tasks` | ボード一覧（kind: dialogue/tool/approval/commit/job/delegated/commitment） |
| `POST /v1/tasks` | `{"action":"list"}`・`{"action":"create","title",…}`・`{"action":"update","id",…}` |
| `GET /v1/agents` | task plane の health（plane 無しノードでは 404 相当） |
| `POST /v1/agents` | `action`: `health`/`models`/`refresh`/`status`/`cancel`/`continue`/`feedback`/`stats`/`eval`/`eval_report`/`delegate`/`apply`/`discard` |
| `GET /v1/commitments` | 耐久目標の一覧 |
| `POST /v1/commitments` | `action`: `create`/`update`/`resume`/`close`/`abandon`/`metrics`/`list`/`show` |
| `GET /v1/approvals` | 保留中の承認（直近50） |
| `POST /v1/approvals/decide` | `{"id","approve"}` — 常にトークン必須。承認は tool ではないのでモデルからは不可能 |

### Tools / Library

| メソッド・パス | 内容 |
|---|---|
| `GET /v1/tools` | OpenAI function-calling 形式の tool 定義 + library カタログ（組み込み + `mcp__<server>__<tool>` + peer 転送分） |
| `POST /v1/tools/list` | `{"local_only": bool}` |
| `POST /v1/tools/call` | `{"name":"json_find","arguments":{…}}` → `{"ok","result"}`。`judge_required` は審査へ、`approval_required` は承認キューへ |
| `POST /v1/library` | `{"action":"list"}`/`tree`/`file`/`search`。自ノードに無い library は healthy peer へ転送 |

### ノード間（peer 向けだが operator も使える）

| メソッド・パス | 内容 |
|---|---|
| `POST /task` | Light/Heavy 委譲。`{"task_id?","type","requires":[],"payload","timeout_ms?","target_node?","wait_ms?"}` → `TaskRecord`。`task_id` は冪等キー。型: `node.ping`/`state.read`/`llm.generate`（+将来 executor 型） |
| `GET /task/<id>` | 現在の `TaskRecord`（`queued→delegating→running_remote→completed/failed/timeout`） |
| `GET /v1/delegations` / `POST` | `action`: `list`/`show`/`retry`/`cancel` |
| `GET /v1/intercom` | intercom 状態（inbound/outbox/conversations/relay） |
| `POST /v1/intercom` | peer envelope（`from`/`to` を持つ）= 姉妹間配送の受け口。それ以外は operator action: `status`/`list`/`show`/`send`/`open`/`close`/`retry` |

## CLI 対応表（`kamimusuhi` バイナリ）

| CLI | 対応 API |
|---|---|
| `kamimusuhi serve` | （HTTP サーバ本体） |
| `kamimusuhi status` | `GET /status`（exit 0 健全 / 1 劣化 / 2 不達） |
| `kamimusuhi ask <text>` | `POST /v1/chat/completions`（経路つき表示） |
| `kamimusuhi chat` | `POST /v1/kamimusuhi/talk` REPL |
| `kamimusuhi library …` | `POST /v1/library` |
| `kamimusuhi approvals / approve / reject` | `GET /v1/approvals`, `POST /v1/approvals/decide` |
| `kamimusuhi agents …` | `POST /v1/agents` |
| `kamimusuhi commit …` | `POST /v1/commitments` |
| `kamimusuhi intercom …` / `say` | `POST /v1/intercom` |
| `kamimusuhi node` | `GET /status` の `node_state` 節 |
| `kamimusuhi task …` | `POST /task`, `GET /task/<id>`, `POST /v1/delegations` |
| `kamimusuhi check-config` | `Config::load` + validate（deny_unknown_fields） |

接続先の決定順: `--url` > `$KAMIMUSUHI_URL`（カンマ区切り）> `~/.config/kamimusuhi/nodes`
> （config がローカルにあれば）その `node.listen` の loopback。
トークンは `$KAMIMUSUHI_NODE_TOKEN` > `~/.config/kamimusuhi/node_token`。
