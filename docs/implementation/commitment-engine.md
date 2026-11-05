# Commitment Engine（持続する目標）

澪の「その瞬間の判断力」は Chat Plane / Task Plane が担うが、一つの目的に食らいつき続ける機構は別の性質を持つ。Commitment Engine は **Goal → Commitment → checkpoint → retry/replan → completion** を Mio Core の構造として実装する器官で、LLM の強さではなく澪側の性質として持続力・継続力を与える。

- **持続力**: 1タスクを30分・3時間・1日と追い続ける
- **継続力**: 再起動・LLM交換・失敗・別タスク割り込み後も、昨日の続きを始める

```text
Commitment（current_state/commitments.json）
      │  tick（既定 30 秒）ごとに全 commitment を走査
      ▼
 委譲タスクを観察 ─ done → 計画を進める → 成功条件を検証 → close
      │          ├ failed ─ 同一シグネチャ → 再計画 L0→L6
      │          │          新シグネチャ   → 再試行
      │          └ 中断（再起動）→ セッション継続 / 再試行
      ▼
 委譲タスクなし ── checkpoint.next を実行
      ├ delegate → Task Plane へ
      ├ verify   → 成功条件チェック
      ├ sleep    → 外部条件待ち
      └ ask      → オペレータへエスカレーション
```

## 不変条件

1. 会話が終わっても commitment は死なない。永続化の正本は `current_state/commitments.json`（atomic write）で、イベントは `logs/commitments` に `append_sync`（fsync 付き）でジャーナルする — プロセス再起動だけでなく電源断まで耐える。NAS 同期では `current_state/*.json` 自体も `state/<node>/` へ定期的に運ばれる（SSD 全損はイベントジャーナル＋最新 state の複製でカバー）。
2. 「諦める」は暗黙に起きない。`active` を離れるのは検証済み close・明示的 `abandon`・予算/ラダー尽きた `escalated` の3つだけ。
3. 同じ失敗を繰り返さない。失敗はシグネチャ（executor:model + 正規化したエラー）で比較し、`same_failure_limit`（既定 2）を超えたらラダーを1段上げる。一時パスや終了コードなど揮発的な部分は正規化で潰す。
4. 実行器官は Task Plane。Commitment Engine 自身は executor を持たず、`task_delegate` に委譲する。executor/model 選択・課金 quota・worktree 分離はすべて既存の Task Plane 側の規則に従う。
5. 委譲結果は `external_task_result` の Evidence であって belief ではない。canonical な identity / memory を engine は直接書かない。
6. `command` 成功条件チェックは operator 作成の commitment でしか実行しない。個人が作った commitment のチェックは `files_exist` 等の観測的なものに限定する。
7. ディスパッチ成功を完了と見なさない。完了は観測したタスク状態と成功条件の検証だけから来る。
8. tick の判定は revision 付き。外部実行はロック外で行い、結果適用時に revision が変わっていれば捨てる（API 経由の編集を上書きしない）。

## Commitment オブジェクト

```yaml
goal: "○○を完成させる"          # 目的（必須）
success_condition:              # 何をもって完了か + 機械チェック
  description: "tests pass"
  checks: [{type: command, run: "cargo test"}]   # command は operator のみ
  operator_confirm: false       # true なら operator 確認まで close しない
state: active                   # active|sleeping|escalated|done|abandoned
owner: mio                      # mio | operator
board_task: t…                  # Task Board 上の対応タスク
plan: [{objective, kind, status, task_id…}]   # L4 分解で差し替わる
checkpoint:                     # 再起動後はここから再開
  doing: "…"                    # WHAT_I_WAS_DOING
  why: "…"                      # WHY
  worked: […]                   # WHAT_WORKED
  failed: […]                   # WHAT_FAILED
  current_state: "…"            # CURRENT_STATE
  next: {type: delegate|verify|sleep|ask_human|replan}   # NEXT_ACTION
attempts: [{task_id, level, executor, model, outcome, failure_signature…}]
strategies_tried: ["fake:fake-1", …]
replan_level: 0                 # 0..=6
budget: {time_secs, attempts, usd}   # 尽きたら escalate（既定 attempts=30, 6h）
spent: {attempts, usd}
resume_after_restart: true
current_task: t…                # 委譲中の board task
interrupted: false              # 再起動で中断された印
sleep_until / sleep_reason      # sleeping の起床条件
question                        # escalated 中のオペレータへの問い
escalated_at                    # escalated 入りした時刻（滞留の計測）
notes                           # 時系列メモ（bounded）
revision                        # 変更ごとに +1（tick の楽観ロック）
```

履歴類（attempts / failed / notes / commitments 本体）はすべて上限付きで増え続けない。

## 再計画ラダー L0–L6

「粘る」と「同じことを繰り返す」を分けるため、同一シグネチャの失敗が `same_failure_limit` に達するたびに1段上がる。

| レベル | 動作 |
|---|---|
| L0 | 同じ方法でもう一度 |
| L1 | 同じ executor で別モデル（カタログの未試行モデル） |
| L2 | 別手法（失敗履歴を埋め込んだ別の目的語で再委譲） |
| L3 | 別 executor へ切替（ルーティング変更）。候補なし → `blocked_sleep_secs` 待って L4 へ |
| L4 | 問題を分解。planning タスクを委譲し、返された JSON steps を計画として採用してラダーを L0 に戻す |
| L5 | 前提を疑う。レビュータスクに `{"continue", "reason", …}` を答えさせ、`false` なら L6 へ |
| L6 | オペレータへ質問して `escalated`（以後 retry しない） |

engine が自分で結果を消費する委譲（L4 分解・L5 レビュー）は `pending` で追跡し、失敗時に必ずクリアする（分解失敗の結果をレビュー結果と誤認しない）。

## 再起動と中断

- 起動時のロードで、`active`/`sleeping` かつ `resume_after_restart` の commitment は `interrupted` を立ててそのまま復帰する（チャット履歴ではなく `commitment + checkpoint + next_action` から再開）。
- 中断で board 上の委譲タスクが `failed`（結果 Evidence なし）になっていれば、それは戦略失敗ではなく中断なので `Resume` する: executor が `resume_args` に対応していれば外部セッションを継続し、無理なら同じ行動をやり直す。いずれもラダーの失敗回数には数えない。
- Task Board 側でも `kind: commitment` の board task は再起動時の一律 failed 化から除外する（commitment 本体が生存を決める）。
- 外部条件待ち（委譲タスク消失・使える executor なし等）は `sleeping` で時限起床する。

## ピア委譲（plane を持たないノード）

continuity ノード（Pi 等）は `task_plane` を持たないが、commitment はそこで生存する。この場合 attempt の実行だけを健全なピアの task plane に借りる — commitment の正本・checkpoint・metrics は常に発生元ノード側。

- `dispatch` はローカル plane があれば従来どおり `task_orchestrator::handle` を直呼びし、無ければ `peer_healthy` の立つピアへ `/v1/agents` に同じ `delegate` リクエストを転送する（往路は tools.rs::post_peer と同じ経路・認証・60 秒の有界タイムアウト）。`no task plane` を返すピアはスキップし、クォータ等の admission エラーは最初の一つを呼び出し元へ返す。全ピアが使えなければ 503「no task plane …」→ 外部条件待ち sleep。
- 観察: `current_task_node` が他ノードなら tick はピアへ `status` アクションを POST し、同じ `Decision` 群に写像する（done→TaskDone / failed+結果なし→中断として Resume / cancelled / 404→TaskLost / 到達不能→様子見）。
- 継続: ピア側が再起動等でタスクを cut した場合（failed かつ結果行なし）、`continue` アクションをピアへ転送する。セッションの有無の判定はピア側の `continue_task` に任せる（status 応答の `external_session_id` は観察用に追加済み）。
- 放棄: `abandon` はリモートの委譲タスクにも `cancel` を転送する（ピア上の harness プロセスを孤児にしない）。
- 帳簿: Attempt と board task detail にノード名を記録する（`attempt.node`、`attempt_node`）。
- 制約: `files_exist`/`command` の成功チェックはローカル FS を見るため、plane のないノードで peer 実行した成果物は検証できない。チェック自体が実行不能な場合は梯子を登らず「検証不能」として escalate する。

## 個体との接続

- 澪は `commit_*` tools で commitment を作成・更新・resume・close・abandon できる（`runtime.json` の `tools.allowed` に列挙する）。board task（`task_*`）は「今やる意図」の可視化で、commitment は「検証されるまで追い続ける耐久目標」— 使い分けは tool description に明記。
- 個体の操作権限: `command` チェック・`autonomous_workspace` 権限・`operator_confirmed`・**予算の引き上げ**は operator 限定のまま。予算は engine が自分を律する境界なので、追跡される側が緩められると境界として機能しない。
- **成果の記憶回収（`commitments.notify_subject`）**: 設定すると done / abandoned / escalated への遷移が outbox に積まれ、notify スレッドがその subject で `dialogue::talk` に1ターンを投げる — 結果はシステムイベントとして「澪が体験した」ことになり、正本メモリには通常の intake と同じゲートを通って入る。escalated は同じ question を一度しか通知しない（resume が状況を変えなかった再 escalate で通知がループしない）。

## エスカレーションの可視性

- `escalated` は tick 対象に含め、`stall_note_secs` ごとに `escalation_reminder` をジャーナル＋ノートする — 回答が来ない escalation を放置で消えさせない。
- `/status` の `commitments.escalated` に id・goal・question・`waiting_secs` を列挙。board task 側は `awaiting_operator` + `escalated: true` detail が付く。

## 評価指標（PersistenceScore）

`commit` / `/v1/commitments` の `metrics` アクションと `/status` から取れる。正答率ではなく持久力を測る。

- task survival time（created→closed の生存秒）
- interruption recovery rate（interruptions に対する resume 成功）
- successful resume rate（resume 後に進捗した割合）
- no-progress → replan 率 / replans 総数
- repeated-failure avoidance（同一シグネチャの連続失敗数）
- unfinished commitment abandonment rate
- long-horizon completion rate（長時間 commitment の完了率）

## 実装の所在

| 層 | 場所 | 内容 |
|---|---|---|
| resident | `commitments.rs` | Commitment 型・store・tick 状態機械（`decide`/`apply`）・再計画ラダー・成功条件チェック・persistence metrics・通知 outbox + notify スレッド・`/v1/commitments` handler・`commit_*` tool 定義 |
| | `config.rs` | `commitments` セクション（`enabled`/`tick_secs`/`max_active`/`same_failure_limit`/`stall_note_secs`/`blocked_sleep_secs`/既定予算/`notify_subject`） |
| | `state.rs` | `Shared.commitments`、`/status` の `commitments` セクション（escalated 一覧を含む） |
| | `spool.rs` | `append_sync` — fsync 付きジャーナル（conversations・commitments・tasks・approvals 等の重要台帳で使用） |
| | `dialogue.rs` | 受理記録 — ターン実行前に `received`、完了/失敗で `answered`/`failed` を `conversations/dialogue` に残す（応答不能になった発言が歴史から消えない） |
| | `probes.rs` | nas-sync が `current_state/*.json` を `state/<node>/` へステージして配送 |
| | `tasks.rs` | board task `kind=commitment` を再起動一律 failed 化から除外 |
| | `task_orchestrator.rs` | engine が使う accessor、継続時の `external_session_id` フォールバック、status 応答への `external_session_id` 追加、cancel の終了済み判定 |
| | `tools.rs` | `post_peer`（ピアへの `/v1/agents` 転送で共用） |
| | `server.rs` / `tools.rs` / `client.rs` / `main.rs` | `GET/POST /v1/commitments`、`commit_create/list/show/update/resume/close/abandon` tools、`kamimusuhi commit` CLI |

## API / tool / CLI

- HTTP: `GET /v1/commitments`（一覧）、`POST /v1/commitments`（`{"action": …}` で create/update/resume/abandon/close/metrics/list/show）
- 澪の tools: `commit_create` `commit_list` `commit_show` `commit_update` `commit_resume` `commit_close` `commit_abandon`
- CLI: `kamimusuhi commit new <goal…>` / `list [--all]` / `show <id>` / `note <id> <text…>` / `resume <id>` / `close <id>` / `abandon <id>` / `metrics`

`create` は goal・success_condition・事前分解 steps・budget・`resume_after_restart` を受ける。`update` は checkpoint 系フィールド（note/doing/current_state/worked/steps/next_action）の編集。`resume` は escalated commitment への operator の回答で、L0 から再武装する。`update`/`resume` の `budget`（指定フィールドだけ現予算へマージ）は operator 限定 — 予算枯渇で escalate した commitment は予算を上げて resume しないと再 escalate する。`abandon` は実行中の委譲タスクもキャンセルする（harness プロセスを孤児にしない）。

## 未検証事項

mac ノードでの実機 E2E（実 `opencode` 委譲→close、再起動→外部セッション継続、予算枯渇→escalate→budget 付き resume、abandon→実行中キャンセル）、Pi→mac / Pi→llm_master の実ピア委譲 E2E（委譲→close、双方の再起動→継続観察/リモート continue、abandon→リモート cancel）は検証済み。残る課題は `notify_subject` 通知ターンの実機検証、および 24 時間ストレス（LLM 障害 3 回・レート制限・ホスト再起動を入れて同じ目的を追い続けるか）。
