# Light / Heavy ノード分離アーキテクチャ

Kamimusuhi の実行基盤を2種類のノードに分離する設計。**Light Node が Kamimusuhi の
連続性を保持し、Heavy Node は計算資源として差し替え可能**である。

## 概念

| | Light Node | Heavy Node |
|---|---|---|
| 役割 | `continuity` — 常時稼働、状態・判断・routing | `cognition` — LLM/embedding/重い処理 |
| 例 | Raspberry Pi 4B (Debian, RAM 4GB) | llm_master (GPU), 外部API |
| 保持するもの | session/goal/queue/delegation state, heartbeat | 要求された計算の結果のみ |
| LLM | 直接実行しない | ローカル tier で実行 |

重要なのは **Heavy Node = Kamimusuhi 本体ではない** こと。
Pi だけ起動していても Kamimusuhi の runtime は存在し続け、必要な思考だけ
llm_master / 外部API へ委譲する。

実装上は同一バイナリ (`kamimusuhi`, crate `kamimusuhi-resident`) が両方を兼ねる。
`node.role` が `continuity` なら light、`cognition` 等なら heavy として振る舞う。

```
                 External Input (UI / Discord / API)
                        │
                        ▼
                ┌────────────────┐
                │   Light Node   │  Raspberry Pi / role=continuity
                │ event loop     │
                │ session state  │
                │ goal state     │      delegation
                │ router ────────┼──────────────►┌─────────────┐
                │ queue          │  POST /task   │ Heavy Node  │
                │ health monitor │◄──────────────┤ llm_master  │
                │ delegation mgr │  GET /task/id │ GPU / API   │
                └───────┬────────┘               └─────────────┘
                        │
                        ▼
                 Persistent Store (個体DB / NAS journals)
```

## Task protocol (`crates/kamimusuhi-node`)

ノード間の共通ワイヤ型。HTTP/JSON。型だけの crate で、HTTP/SQLite/スレッドに
依存しない。

### `POST /task` (bearer auth)

```json
{
  "task_id": "nt-…",          // 冪等キー。再送しても再実行されない
  "type": "llm.generate",     // タスク種別
  "requires": ["llm.generate"], // 要求 capability
  "payload": {"messages": []},
  "timeout_ms": 120000,        // 全体期限（queue待ち+送信+実行）
  "target_node": "llm_master", // 省略時はclassifierが選ぶ
  "wait_ms": 0,                // >0 でlong-poll（終了まで待つ）
  "reply_to": null             // 予約済（将来のpush delivery）
}
```

応答は `TaskRecord` (`{task_id, type, state, node?, result?, error?, attempts,
created_at, updated_at, next_attempt_at?}`)。未完了は `202 Accepted`、
終端状態は `200`。同じ `task_id` の再送は `deduplicated: true` で既存
レコードを返す。

### `GET /task/<id>` (bearer auth)

現在の `TaskRecord`。委譲側はこれを `poll_ms` 間隔でポーリングする。

### `GET /capabilities` (無認証, `/health` 同等)

```json
{"node_id": "llm_master", "role": "cognition", "mode": "heavy",
 "capabilities": ["llm.generate", "node.ping", "state.read"]}
```

`/health` にも同じ `capabilities` が載るため、既存の peer プローブが
そのまま capability discovery になる（`peers[].capabilities` の静的宣言は
未到達時の fallback）。

### 状態遷移

```
queued → delegating → running_remote → completed | failed | timeout
   ↑         │                            │
   └─────────┴──── retry_pending ◄────────┘   （回復可能な失敗）
                          │
                     （attempts 上限で failed）
```

- inbound (heavy側): `queued → running → completed | failed | timeout`
- `cancelled` は任意の非終端状態から operator が投入可能

## ルーティング規則 (LOCAL / DELEGATE / QUEUE / REJECT)

`delegation::decide` が rule-based で判定する。

1. `target_node` が指定されれば pin。自分宛 → local、他ノード宛 → その peer へ
   （未設定peerや能力不足は reject — 委譲は連鎖しない）
2. `node.*` / `state.*` / `queue.*` / `heartbeat` は常に local
3. **light node は healthy な capability 保有 peer を優先**（Pi の仕事は連続性であり
   重い計算ではない）。**heavy node は local を優先**し、自分に無い capability の
   タスクだけが peer に流れる
4. capable な peer がすべて不健康 → queue（`retry_pending` で待機、attempts を消費しない）
5. 誰も実行できない → reject（422）

capability は「そのノードが `/task` でローカル実行できること」を意味する。
Pi が自分の tier 経由で LLM を呼べても `llm.generate` は advertise しない
（`node_local` tier を持つノードだけが持つ）。`node.capabilities` で追加宣言できるが、
executor の無い capability を宣言すると呼ばれたタスクが `unsupported_capability` で
失敗する — 宣言は実装があるものだけ。

## 永続化と再起動

`current_state/delegations.sqlite` (WAL)。全タスクを受理時に先に永続化する。

- 再起動時、in-flight 状態 (`delegating`/`running_remote`/`running`) は
  `retry_pending` に倒される。実行中だったものは信頼しない。
- outbound の再送は `task_id` 冪等 — peer が既存レコードを返せば継続、
  無ければ新規実行。どちらでも二重実行にならない。
- Pi再起動で復元されるもの: pending tasks / active goal (commitments.json) /
  session metadata (個体DB + dialogue journal) / delegation events。

payload/result は node-local sqlite のみ。NAS に同期される task board と
`logs/delegations` journal には id/type/state/latency だけを書く
（個人情報の bodies は NAS に出さない）。

## Failure 挙動

| 事象 | 挙動 |
|---|---|
| Heavy Node 停止 | light は動作継続。submit が transport error → `retry_pending`（attempts消費、上限で `failed`）。poll 中に切れた → `poll_error_limit` 連続失敗で `retry_pending`。回復後に自動再送 |
| Network 切断 | light の event loop は止まらない。全 peer/HTTP 呼び出しに timeout がかかる |
| Pi 再起動 | sqlite から queue/state 復元、in-flight は `retry_pending` へ |
| Heavy 応答 timeout | `timeout_ms` deadline で `timeout`。無限待機しない |
| Heavy 復帰 | parked タスクが `delegating` に戻り、冪等再送で完了または継続 |

## Pi デプロイ

既存の `deploy/resident/` を使う。Pi の light node は
`kamimusuhi-resident.service`（`Restart=always`, `RestartSec=5`,
`journalctl -u kamimusuhi-resident -f`）として常駐する — spec上の
`kamimusuhi-light.service` に相当するのはこの unit で、二重常駐を避けるため
名前を増やさない。

```bash
# Pi で
install-node.sh pi           # 既存のまま
systemctl status kamimusuhi-resident
kamimusuhi node              # spec形状の node_state
kamimusuhi task submit node.ping
kamimusuhi task submit llm.generate '{"messages":[...],"requires":["llm.generate"]}'
```

設定: `pi.resident.example.json` に `peers[].capabilities`/`priority` と
`delegation` ブロックを追加済み。`delegation.workers` は 2（Piでは小さく）。

## Status / metrics

`/status` に `node_state`（spec形状: `node_id`, `mode`, `status`,
`active_sessions`, `active_goal`, `heavy_node_available`, `pending_tasks`,
`last_heartbeat`, `last_error`）、`delegations`（state別集計・recent・
last_latency_ms・last_error）、`process`（Linuxのみ RSS/CPU）が入る。
`kamimusuhi status` の DELEGATIONS セクションにも表示。

## 今後の課題

- executor の拡充: `embedding`, `vision`, `web.research`, `plan.deep` は
  protocol に予約済み。実装は各 heavy node の adapter として足す
- `reply_to` の push delivery（relay worker 経路）
- 優先度付きキュー（現状は submission 順 FIFO + peer priority）
- capability discovery の動的更新は probe 経由で済むが、mDNS/Tailscale 等の
  node discovery 自体は未実装（設定ファイル前提）
- 複数 heavy への負荷分散（現状は priority + healthy のみ）
