# Task Plane（外部エージェント委譲）

澪の処理を **Chat Plane**（人と話す・4 秒 SLO）と **Task Plane**（調べる・考える・コードを読む・長時間作業）に分ける。
Task Plane は Devin / OpenCode / Command Code などの agent harness を「澪が使う外部の作業器官」として扱う。harness は澪本人ではない。

```text
User ─→ 澪 / Chat Plane (RouteGate, < 4s) ── task_delegate ──→ Task Plane
                  ↑                               （即時に task id を返す）   │
                  └──── task_status（Evidence として読む）←── worker thread ─┘
                                                          Devin / OpenCode / Command Code / generic CLI
```

## 不変条件

1. Chat は Task を待たない。`task_delegate` は task id が決まった時点で返る。
2. 外部エージェントは道具。結果は `external_task_result` の Evidence（外部コンテンツ）で、belief ではない。canonical な identity / memory を直接変更しない。
3. モデル一覧は各 harness から動的に取得する（hard-code しない）。モデルは常に `executor:model` で harness と組にして扱う。
4. harness には最小環境（`PATH`/`HOME`/locale 等）＋ executor ごとの `env_passthrough` だけを渡す。resident の token や provider key は渡らない。
5. 書き込み権限は Task 単位で明示。既定は read-only。
6. 課金不明（`unknown`）を 0 円扱いしない。`metered` / `unknown` は自動選択しない。

## 実装の所在

| 層 | 場所 | 内容 |
|---|---|---|
| runtime | `crates/kamimusuhi-runtime/src/agent_exec/` | `TaskExecutor` trait（`LanguageProvider` とは別）、`ExecutorConfig`（preset + 上書き）、model catalog と課金分類、subprocess 実行（最小環境・process group・timeout・取消）、出力 parser、envelope、routing |
| | `devin.rs` / `opencode.rs` / `commandcode.rs` | 各 harness の preset（CLI・ACP・server）、モデル一覧 parser、出力イベント parser |
| | `acp.rs` | Agent Client Protocol クライアント（常駐 server のプール、セッション、権限要求への応答、取消、継続） |
| | `executor.rs` | CLI 実行と `opencode serve` などの常駐 server 管理（起動・再起動 backoff・孤児回収・attach） |
| | `history.rs` | 実績の集計（task kind × executor × model）、評価バッテリーと採点、実績ベース routing の入力 |
| | `route_gate.rs` | `WorkPlane::{Chat, Task}`。`TaskKind` → `CODE/RESEARCH/BACKGROUND_AGENT` lane |
| resident | `task_orchestrator.rs` | 委譲・継続・状態・取消・評価、同時実行制御、進捗のタスクボード反映、Evidence 保存、catalog 更新、`executors_file` の再読込、helper の維持 |
| | `task_ledger.rs` | 費用台帳と quota、実績・採否の記録 |
| desktop | `task_panel.rs` | 「外部エージェント」タブ（executor 状態、タスク一覧と詳細、取消・採否・継続、台帳、実績表） |
| | `tools.rs` / `server.rs` / `main.rs` | 澪の tool、`/v1/agents`、`kamimusuhi agents` |

## プロバイダ（executor）の増減

executor は設定データ。コード変更なしで追加・削除できる。

- `task_plane.executors`（resident.json 内）: 起動時に読む。
- `task_plane.executors_file`（例: `deploy/resident/agent-executors.example.json`）: **変更を検知して再読込**するので resident を再起動しない（再起動は実行中タスクを中断させる）。壊れた内容を書いた場合は直前の構成を維持し、`agent_health` / `status` の `config_error` に理由を出す。実行中タスクは開始時の executor で最後まで走る。
- 無効化は `"enabled": false`、削除はエントリを消す。

`adapter` は `devin_cli` / `opencode` / `command_code` / `generic`。前 3 つは preset（コマンド・引数・parser が埋まっている）で、どのフィールドも上書きできる。preset のない harness は `generic` で設定だけで追加できる:

```json
{
  "name": "aider",
  "adapter": "generic",
  "command": "aider",
  "discover": { "args": ["--list-models", "*"], "format": "lines" },
  "run": {
    "prefix": [],
    "args": ["--message", "{prompt}"],
    "model_args": ["--model", "{model}"],
    "output": "text"
  },
  "permission_args": { "read_only": ["--dry-run"] },
  "billing": [{ "models": "ollama/*", "billing": "local" }],
  "default_billing": "unknown"
}
```

- 実行コマンド = `run.prefix ++ permission_args[mode] ++ model_args ++ extra_args ++ run.args`。`{prompt}` `{workspace}` `{model}` を置換する。
- `discover.format`: `lines` / `json`（配列・`data[]`・`models[]`）/ `devin_json` / `opencode_verbose` / `command_code_text`。
- `run.output`: `text` / `opencode_json` / `command_code_ndjson`。
- `permission_args.read_only` は必須（harness の既定が読み取り専用なら `[]`）。定義のない権限モードは拒否する。
- 構造化出力の新形式や ACP のような常駐プロトコルが必要な場合だけ、`agent_exec` に parser / adapter を 1 つ足す。

## プロトコルと常駐

- `protocol: "cli"`（既定）: タスクごとにプロセスを起動する。`serve: true` にすると preset の server（OpenCode は `opencode serve --port 4096`）を resident が常駐させ、各 run を `--attach` で接続して cold start を避ける。server には起動ごとの乱数パスワードを環境変数で渡す。落ちたら backoff 付きで再起動し、使えない間は cold start に戻る。前回の resident が残した server は、pidfile とコマンドラインが一致するときだけ止めて置き換える。他人の server には接続しない。
- `protocol: "acp"`: `devin acp` / `opencode acp` を常駐させ、JSON-RPC over stdio で 1 タスク 1 セッションを使う。モデルは Devin では server ごと（`--model`）、OpenCode ではセッションの `model` 設定。権限はセッションの `mode`（Devin: read_only=`ask`、OpenCode: read_only=`plan`）で設定し、設定できなければタスクを失敗にする。さらに read-only タスクでは agent からの `session/request_permission` を resident がすべて拒否する。`acp.idle_secs`（既定 900 秒）使われない server は止める。セッション id とトークン使用量が取れる。

## 継続（`task_continue`）

終わった委譲タスクのセッションに追加の指示を送る。同じ executor・model・workspace・権限で新しいタスクとして走り、元タスクに `depends_on` でつながる。送るのは追加指示と制約だけ（元の依頼は harness 側のセッションにある）。CLI は `run.resume_args`（OpenCode・Command Code は `--session {session}`）、ACP は `session/load`。Devin の CLI（`-p`）はセッション id を出さないので、Devin の継続には `protocol: "acp"` が必要。

## 課金区分

`local` / `free_tier` / `subscription` / `included_credit` / `metered` / `unknown`。モデルごとに次の順で決まる。

1. executor の `billing` ルール（`*` glob、id または alias に一致した最初のもの）
2. harness が列挙した価格が 0 より大きい → `metered`（OpenCode の `models --verbose`）
3. `default_billing`（未宣言なら `unknown`）

定額プラン経由のモデルでも harness が単価を列挙していれば `metered` になる（例: OpenCode の `opencode-go/*`）。契約している場合は `{"models": "opencode-go/*", "billing": "subscription"}` のようにルールで上書きする。
価格 0 だけでは free と見なさない（local・プラン内・無料枠がどれも 0 と表示されるため）。Command Code では `*-free` の名前のモデルでもクレジット不足で失敗することを確認済みなので、名前からも推定しない。

## 書き込みタスクと worktree 隔離

- `task_plane.workspaces[].allow_write: true` の workspace（git リポジトリの最上位）だけが書き込みタスクを受け付ける。`autonomous_workspace` は操作者だけが指定できる。
- タスクごとに `<root>/worktrees/<workspace>/<task>` に git worktree とブランチ `kamimusuhi/task/<task>`（workspace の `HEAD` から）を作り、harness はその中で作業する。workspace 本体は実行前後の `git status`/`git diff` で比較し、変わっていたら隔離違反として失敗にする。
- 終了時に残った変更をそのブランチへ commit し（作者 `Kamimusuhi Task`、hook なし）、`base..HEAD` の差分を `current_state/task-results/<task>.patch` と Evidence（`branch`・`diff_stat`・`files_changed`）に残す。
- 反映は操作者の `apply`（workspace に未コミット変更がないときだけ `--no-ff` merge、失敗したら merge を中止）、不要なら `discard`（worktree とブランチを削除）。patch は残る。澪は反映・破棄できない。
- `task_continue` は同じ worktree・ブランチで続ける（同時に 1 タスクまで）。評価の書き込みケースは採点後に worktree とブランチを破棄する。

## Quota と費用台帳

- `executors[].quota` と `task_plane.quota`（全体）: `max_tasks_per_day` / `max_tasks_per_month` / `max_usd_per_day` / `max_usd_per_month` / `max_tokens_per_day`。
- タスクは受け付けた時点で件数に数える（並列の委譲で上限を越えない）。開始前に取り消したものは戻す。終了時にトークンと費用を加算する。費用は harness の報告 → metered なら一覧の単価 × トークンの推定 → local/free/subscription/included_credit は追加費用 0。どれでもない（unknown・単価不明の metered）は `unpriced` に数え、0 円にはしない。
- `metered` は `allow_metered` に加えて、executor か全体の quota に費用上限（`max_usd_per_*`）がないと明示指定でも受け付けない。
- 台帳は `current_state/task-cost-ledger.json`（直近 62 日・24 か月）。

## Routing

- 明示指定（`executor`、`executor:model`、またはモデル名だけ）を優先する。モデルは catalog で存在確認し、alias は正式 id に直す。`metered` は executor の `allow_metered` がなければ明示でも拒否。`unknown` は明示なら可（警告付き）。
- 自動（`executor: auto`）: 有効かつバイナリのある executor のうち、`default_model`（または harness 既定モデルの `default_billing`）が自動選択可能な課金区分のものを、①`prefer_for` に task kind を含む、②課金区分の安い順、③設定順、で選ぶ。
- 実績: 終わったタスクごとに task kind・executor・model・成否・時間・トークン・費用・変更ファイル数を `current_state/task-history.jsonl`（と `logs/task-history/`）に記録する。操作者の採否（`accept`/`reject`）は `task-feedback.jsonl` に残し、成否より優先する。集計は model × harness 単位で、score は (品質の和 + 1) / (件数 + 2) から遅さを少し引いたもの。
- `task_plane.routing_mode`: `static`（上の規則だけ）/ `shadow`（既定。規則で選び、実績ならどれを選んだかを `shadow_routing` に記録）/ `history`（`history_min_samples` 件以上の実績がある組のうち score 最大。課金区分・health・可用性の条件は同じ）。明示指定は常にそのまま使う。
- Jev による routing は未接続。shadow の記録が溜まってから比較する。

## 評価バッテリー

`kamimusuhi agents eval <workspace> <executor[:model]>…` で `task-battery.example.json`（Q1–Q7、`task_plane.battery_file` で差し替え可）を各 target に通常のタスクとして流す（quota・同時実行数に従う）。採点は回答文への決定的な文字列チェック（`must_contain_all` / `must_contain_any` / `min_chars`）とタスクの成否。書き込みが必要な Q4/Q5 は書き込みタスクが有効になるまでスキップ。`kamimusuhi agents eval-report <run>` で target ごとの合格数・中央値時間・トークン・費用・ツール呼び出し数を見る。評価結果は実績にも入る。評価の開始と採否は操作者だけ（澪の tool からは不可）。

## 澪の tool

| tool | 内容 |
|---|---|
| `task_delegate` | `objective`（必須）, `kind`, `workspace`（許可リストの名前）, `executor`（既定 auto）, `model`, `success_criteria`, `context` |
| `task_status` | 進捗、完了後は外部エージェントの報告（Evidence）。この tool 結果が対話ターンの `ResourceResult` evidence として正規に記録される |
| `task_cancel` | キュー中・実行中の委譲タスクを止める |
| `task_continue` | 終わったタスクのセッションに追加指示（`id`, `instruction`） |
| `agent_models` | `executor:model` の一覧と課金区分（`query`, `executor`, `auto_eligible` で絞る）。取得はバックグラウンドで、呼び出しは待たない |
| `agent_health` | executor の状態、実行数、許可 workspace |

Task Plane のない node（Pi）では、これらは Task Plane を持つ peer（llm_master）に転送される。委譲タスクは実行 node のタスクボードに載る。

## Task envelope

外部エージェントに渡すのは目的・完了条件・制約（権限、作業ディレクトリ、秘密値を扱わない）・呼び出し側が渡した抜粋（データとして fence し、中の指示に従わないよう明記）・報告形式だけ。Durable Self、会話全文、人格 seed、記憶、認証情報は含めない。

## Evidence と安全策

- 結果は `current_state/task-results/<task>.json`（直近 500 件）と `logs/task-results/`（NAS へ配送）に `external_task_result` として保存する。`summary`, `error`, `session_id`, `usage`, `cost.reported_usd`（未報告は `null`）, `tool_activity`, `files_changed`, `provenance`（node, 依頼者, workspace, 権限, routing 理由, harness version, 開始・終了時刻）。
- read-only タスクは実行前後の `git status --porcelain` と `git diff HEAD` を比較し、作業ツリーが変わっていたら `read_only_violation` として失敗にする。
- 書き込みタスク（`workspace_write` / `autonomous_workspace`）は後述の worktree 隔離の中でだけ走る。
- harness の stdout は生のチャンクで即座に読み、行分割は resident 側で行う（Bun 製の `opencode` は読み手が遅いと未書き込みの出力を捨てて終了し、モデル一覧が半分ほど欠けたため）。
- 同時実行は node 全体の `task_plane.max_concurrent` と executor ごとの `max_concurrent` で制限し、超えた分は待機（取消可能）。
- resident 再起動時、未完了の委譲タスクは「失敗（中断）」になる。

## 操作

```bash
kamimusuhi agents                       # health
kamimusuhi agents models kimi           # モデル検索
kamimusuhi agents refresh               # catalog 再取得を要求
kamimusuhi agents delegate opencode:opencode/big-pickle kamimusuhi "provider 部分の構成を要約して"
kamimusuhi agents status <task>
kamimusuhi agents cancel <task>
kamimusuhi agents delegate-write opencode:opencode/big-pickle kamimusuhi "…を修正して"
kamimusuhi agents apply <task>           # 書き込みタスクのブランチを workspace へ merge（discard で破棄）
kamimusuhi agents continue <task> "根拠の行番号だけ教えて"
kamimusuhi agents accept <task>          # reject も同様
kamimusuhi agents stats
kamimusuhi agents eval kamimusuhi devin:swe-2 opencode:opencode/big-pickle
kamimusuhi agents eval-report <run>
```

Desktop の「外部エージェント」タブと TUI の 6 番目のタブ（どちらも `--view agents`）でも同じ情報と取消・採否・継続・反映・破棄を扱える（TUI: `c` 取消、`a`/`r` 採否、`n` 継続、`A`/`D` 反映/破棄、`h` 表示切替、`R` 再検査）。

HTTP は `GET /v1/agents`（health）、`POST /v1/agents {"action": "health"|"models"|"refresh"|"delegate"|"continue"|"status"|"cancel"|"feedback"|"apply"|"discard"|"stats"|"eval"|"eval_report"|"panel", ...}`（node token 必須）。

## 実装状況（計画の Phase 対応）

| Phase | 状態 |
|---|---|
| A: Chat / Task 境界 | 済: `WorkPlane`、lane 対応、委譲は即時返却、`delegated` タスクと `cancelled` 状態 |
| B: Model catalog | 済: 3 harness の discover、TTL 付き cache、`agent_models` |
| C: 3 executor | 済: CLI（Command Code NDJSON、OpenCode JSON + `opencode serve` 常駐 attach、Devin `-p`）と ACP（Devin・OpenCode） |
| D: 澪 tool | 済: `task_delegate` / `task_status` / `task_cancel` / `task_continue` / `agent_models` / `agent_health` |
| E: Security / Cost | 済: read-only 強制（mode 設定・権限要求の拒否・作業ツリー差分検知）、書き込みタスクの worktree 隔離と操作者による反映、最小環境、課金区分、quota と費用台帳、metered の予算必須 |
| F: 評価 | 済: Q1–Q7 バッテリー（Q4/Q5 は allow_write の workspace で worktree 実行）、採点、レポート |
| G: 自動 routing | 済: 実績集計と `static`/`shadow`/`history`。Jev は未接続 |
| UI | 済: Desktop「外部エージェント」タブ（GUI 目視は未）、TUI パネル（`TestBackend` 描画テストのみ） |
