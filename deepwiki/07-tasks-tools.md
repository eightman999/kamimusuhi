# 07 — タスク・Commitment・Tools・承認

対象: `tasks.rs`, `task_orchestrator.rs`, `task_ledger.rs`, `task_worktree.rs`,
`commitments.rs`, `judge.rs`, `approvals.rs`, `tools.rs`, `mcp.rs`, `library.rs`,
`docs/implementation/task-plane.md`, `docs/implementation/commitment-engine.md`

個体（と操作者）から見える「仕事」の層を4つに分ける:

```text
対話ターン  → タスクボード（やる意図の可視化・自動記録）
            → Task Plane（外部 agent harness への委譲・長時間作業）
            → Commitment Engine（検証されるまで追い続ける耐久目標）
副作用      → approval_required（人の承認）/ judge_required（モデル審査）
```

## タスクボード（tasks.rs / task_ledger.rs）

`current_state/tasks.json` に載る進行中の作業。`GET/POST /v1/tasks`（action:
list/create/update）。自動記録される kind:

| kind | 記録元 | depends_on |
|---|---|---|
| `dialogue` | 対話ターン | — |
| `tool` | その間の tool 呼び出し（peer 転送分は node=転送先） | 対話ターン |
| `approval` | 承認待ち（状態=あなたの判断待ち） | 要求した tool 呼び出し |
| `commit` | 承認後の反映 commit/push | 承認 |
| `job` | 定期ジョブ（ジョブごとに1枚を更新） | — |
| `delegated` | Task Plane への委譲 | 委譲した対話ターン |
| `commitment` | Commitment Engine（再起動一律 failed 化から除外） | — |

再起動時に進行中だった自動記録タスクは「失敗（中断）」になる。完了は3日表示・400件保持。
個体は `task_list`/`task_create`/`task_update` で自分の計画を載せられる
（自動記録タスクは変更不可）。

## Task Plane（task_orchestrator.rs）

Chat Plane（人と話す・FAST_CHAT 4 秒 SLO）と Task Plane（調べる・書く・長時間）を分け、
後者は Devin / OpenCode / Command Code / generic CLI の agent harness へ委譲する。
**harness は個体ではなく「個体が使う外部の作業器官」**。

不変条件（task-plane.md より）:

1. `task_delegate` は task id が決まった時点で返る — Chat は Task を待たない。
2. 結果は `external_task_result` の Evidence（外部コンテンツ）。belief ではない。
3. モデル一覧は各 harness から動的取得（hard-code しない）。`executor:model` の組で扱う。
4. harness には最小環境 + `env_passthrough` だけ — resident の token・provider key は渡らない。
5. 書き込み権限は Task 単位で明示（既定 read-only）。`delegate-write` は worktree+ブランチで
   行い、本体へは `kamimusuhi agents apply` でだけ入る（task_worktree.rs）。
6. 課金 `unknown` を 0 円扱いしない — `metered`/`unknown` は自動選択しない。

executor は設定データ。`task_plane.executors`（起動時）と `executors_file`
（**変更検知で再起動なしに再読込**。壊れていたら直前構成を維持し status に理由）。
`adapter`: `devin_cli`/`opencode`/`command_code`/`generic`（preset 無し harness は
generic で discover/run/permission_args/billing を書くだけ）。

`quota`: `max_tasks_per_day` / `max_usd_per_day` 等で admission。
実績 routing（`routing_mode`: shadow → 実績ベース）と評価バッテリー
（`kamimusuhi agents eval` / `eval-report`、task-battery.example.json）。

## Commitment Engine（commitments.rs — 最大モジュール）

「その瞬間の判断力」は Chat/Task Plane が担うが、**一つの目的に食らいつき続ける**
機構は別物。`Goal → Commitment → checkpoint → retry/replan → completion` を
個体側の性質として実装する（commitment-engine.md）。

- 正本は `current_state/commitments.json`（atomic write）+ `logs/commitments` へ
  `append_sync`（fsync）— プロセス再起動と電源断の両方に耐える。
- tick（既定30s）ごとに全 commitment を走査: 委譲タスクを観察 → done なら成功条件を検証 →
  close / failed ならシグネチャ比較（`executor:model`+正規化エラー）で
  `same_failure_limit`(2) 超過でラダーを1段上げる。
- **再計画ラダー L0–L6**: L0 同じ方法 / L1 別モデル / L2 別手法 / L3 別 executor /
  L4 分解（planning 委譲の JSON steps を採用）/ L5 前提を疑う（レビュー委譲）/
  L6 operator へ質問して `escalated`（以後 retry しない）。
- `active` を離れるのは検証済み close・明示 `abandon`・予算/ラダー尽きた `escalated` の3つのみ。
  escalated は `stall_note_secs` ごとに reminder を出し続け、回答なしで消えない。
- `command` 成功チェックは operator 作成 commitment のみ（個体のは `files_exist` 等の観測のみ）。
- **ディスパッチ成功 ≠ 完了**。完了は観測したタスク状態 + 成功条件の検証だけから来る。
- 再起動: `active`/`sleeping` かつ `resume_after_restart` は `interrupted` を立てて
  checkpoint から復帰（チャット履歴ではなく commitment+checkpoint+next_action から再開）。
- **ピア委譲**: plane を持たない continuity ノードでは実行だけを healthy peer の
  `/v1/agents` に借りる。正本・checkpoint・metrics は発生元ノード側。
  `abandon` はリモートの委譲タスクにも `cancel` を転送（harness 孤児化防止）。
- `notify_subject` を設定すると done/abandoned/escalated 遷移が outbox 経由で
  その subject の対話ターンとして個体に届く（結果は「個体が体験した」システムイベントとして
  通常 intake の gate を通って記憶に入る）。
- API/tool/CLI: `GET/POST /v1/commitments`（create/update/resume/close/abandon/metrics/list/show）、
  `commit_*` tools、`kamimusuhi commit …`。予算引き上げは operator 限定
  （engine が自分を律する境界なので、追跡される側が緩められない）。

## 承認と審査（approvals.rs / judge.rs）

副作用のある tool 呼び出しに二つのゲート:

- `approval_required`: モデル審査を経ず **直接 operator 承認キュー**へ。
  `current_state/approvals.json` に保留（7日で期限切れ）、`logs/approvals/` に記録。
  `POST /v1/approvals/decide` は loopback からでもトークン必須 —
  承認は tool ではなく、モデル出力からは不可能。
- `judge_required`: `tool_judge` 設定の審査モデル（既定 `hai` tier）が判定。
  判断不能は `on_unavailable`（既定 `approval`）で operator 承認へ —
  **不在の審査は暗黙の yes にしない**。

## Tools / MCP / Library

`/v1/tools` は OpenAI function-calling 形式で公開。組み込み:

- 情報系: `library_list/tree/read/search`, `json_get`, `json_find`
- タスク: `task_list/create/update/cancel`, `task_delegate`, `task_status`, `task_continue`,
  `agent_models`, `agent_health`
- 姉妹: `peer_say`（end=true で会話を閉じる）, `peer_list`
- commitment: `commit_create/list/show/update/resume/close/abandon`
- 動的: `tool_catalog`（一覧）→ `tool_enable`（有効化）

`mcp_servers[]` は stdio MCP サーバを子プロセス常駐・監視（落ちたら backoff 再起動）し、
tool を `mcp__<server>__<tool>` として公開。**自ノードに無い tool は healthy peer へ
転送される**（一覧 fan-out は 5 秒で打ち切り、呼び出しは `logs/mcp/<node>/` に記録）。

`libraries[]` は読み取り専用の参照データ公開（`POST /v1/library` の
list/tree/file/search）。`.git` と library 外のパスは返さない、file は 2MiB ページ、
search は 64MiB 以下のテキストから最大100件。無い library は peer 転送。

tool 定義は送信前に圧縮される（description 200字・引数 120字・名前順固定）—
プロンプトは静的部分（system・tool・seed・自己状態・記憶）→動的部分（観測・
TURN_CONTEXT・入力）の順で、llama.cpp の prompt cache が効く。

次: [08-surfaces.md](./08-surfaces.md) — 人が触る面。
