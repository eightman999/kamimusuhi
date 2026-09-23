# Kamimusuhi resident（常設構成）

Raspberry Pi と llm_master の両方で `kamimusuhi-resident`（バイナリ名 `kamimusuhi`）を systemd 常駐させる。
自己改善・自動実験はこの構成が安定してから追加する。

```text
             NAS (NFS export → /mnt/kamimusuhi, 共有全体は /mnt/nas-share)
          ↗       ↖
        Pi ←────→ llm_master            peer: Tailscale IP :7860 (/health)
        │            │
   continuity     cognition
   scheduler      local LLM (llama-master :8080, qwen3.8-27b)
   K-CORE監督     GPU状態
   state/snapshot
        └──── HAI ───┘  https://hai-api.hcloud.ltd/v1
```

## 役割と書き込み境界

| ノード | role | 担当 | 書くもの |
|---|---|---|---|
| Pi | `continuity` | 24h継続、scheduler、K-CORE監督、authoritative state、routing | `current_state/`、個体 (`runtime/individual/kamimusuhi.sqlite`)、NASの `*/pi/` |
| llm_master | `cognition` | local LLM、GPU、将来の reflection / embedding / 学習・評価 | NASの `*/llm_master/` のみ |

- 正本 state（個体DB）は Pi だけが持つ。llm_master は個体を初期化・更新しない。
- NAS 上の journal は `<category>/<node>/<date>.jsonl` とノード別パスにしてあり、2ノードが同じファイルへ追記しない（single writer）。
- resident は個体を**初期化しない**。初期化は `install-node.sh pi` が「DBも runtime.json も無い場合のみ」一度だけ行う。

## Routing

自動 routing（`kamimusuhi` / `auto`）は、設定順をそのまま試すのではなく
runtime と共通の `RouteGate` で候補を絞ってから実行する。判定順は
**privacy → context/TPM → live health → billing class → latency**。
persona / memory を含む通常の対話は既定で private とし、
`privacy_ok_for_private_memory=true` を明示した tier だけに送る。

- FAST_CHAT の production latency ceiling は既定 **4,000ms**。resident は非streaming upstreamを使うため、ここでは実リクエストの全応答時間をTTFTの保守的proxyとしてEWMAに戻す。4秒を継続して超えた経路はFAST_CHAT候補から外れる。
- FreeTier がprivate条件を満たす場合は先行し、Local / Subscription は同じ zero-marginal-cost band で実測 latency（未計測なら設定順）を使う。Metered は最後の fallback。
- probe（`/v1/models`）の healthy/unhealthy も併用し、known-down tier は healthy 候補がない場合だけ試す。実リクエスト失敗も即 health に反映し、429 は cooldown 対象になる。
- Pi の既定 zero-marginal 経路: `hai`（`qwen3.8-27b-uncensored`）と `llm_master`（peer resident 経由、`X-Kamimusuhi-Route: local` で peer 側 fallback を抑止）。未計測時は設定順、観測後は同band内で latency が効く。
- llm_master の既定 zero-marginal 経路: `local`（llama-master :8080）と `hai`。
- `condition: small_request` の tier は短い要求（既定1200文字以下）か `model: "k0"` のときのみ使う。 `k0` と `X-Kamimusuhi-Route: local` は LocalChat として local tier のみに制限する。
- model 名で `hai` や `hai/glm-5.3` のようにtierを強制できるが、local-only / privacy wall / production cost guard は迂回できない。
- `stream: true` は完成応答を SSE として返す（逐次生成ではない）。

従量 tier は送信前に production `CostGuard` を通る。既定上限は
**$0.02/request / $0.50/daemon session / $0.10/day / $2.00/month**。
daily/monthly ledger は `current_state/routing-cost-ledger.json` に保存され、
再起動後も継続する。従量 tier は input/output の単価宣言が必須で、
未宣言なら起動時設定検証で拒否する。これは設定された単価に基づくローカル上限なので、
provider側の価格変更まで請求額を保証するものではない。従量provider側の spending limit も併用する。

OpenAI互換で使う例（ノード上では token 不要、他ホストからは `Authorization: Bearer $KAMIMUSUHI_NODE_TOKEN`）:

```bash
curl http://127.0.0.1:7860/v1/chat/completions -d '{"model":"kamimusuhi","messages":[{"role":"user","content":"こんにちは"}]}'
```

既存 runtime の対話も resident 経由にできる: `kamimusuhi-runtime chat --persona openai-compatible --persona-url http://127.0.0.1:7860/v1 --persona-model kamimusuhi ...`

## ストレージ

```text
/srv/kamimusuhi/            (ローカルSSD)
  runtime/bin/              kamimusuhi, k-core, kamimusuhi-runtime
  runtime/individual/       個体の正本 (Piのみ)
  config/resident.json      構成 (deploy/resident/local/<node>.resident.json から, git 管理外)
  config/secrets.env        mode 600, Git外
  current_state/            state.json, boot_epoch
  cache/
  spool/                    NAS 未配送分

/mnt/kamimusuhi/  (NAS; マーカー .kamimusuhi-nas がある時だけ使用)
  memory/ conversations/ logs/ artifacts/ experiments/ datasets/ models/ checkpoints/
```

- 書き込みは必ず spool → 別スレッドで NAS へ配送。NAS 断・NFS ハングでも会話・heartbeat は止まらない。
- マーカーが無い（未マウントで空のマウントポイントが見えている）場合は NAS 扱いしないので、SSD に誤書き込みしない。
- journal は追記配送（at-least-once、各行に一意 `id`）。snapshot 等の丸ごとファイルは tmp+rename。
- NAS 断中の snapshot は spool に最新 `spool_keep` 個だけ保持。
- mount は `soft,nofail,x-systemd.automount`。

## 導入手順

前提: 両ノードとも同じサービスユーザー（uid 1000）。NAS 側で共有全体と kamimusuhi 用ディレクトリをそれぞれ NFS export し（Maproot=uid/gid 1000）、resident 用は `/mnt/kamimusuhi` に直接マウントする（子データセットは親 export から見えないため）。ホスト・export パスは `deploy/resident/local/site.env`（`site.env.example` 参照, git 管理外）に書く。

**サイト固有の設定（ホスト/IP・NAS・private リポジトリ名）は `deploy/resident/local/`（git 管理外）に置く。** 追跡されるのは `*.example.json` と `site.env.example` のテンプレートのみ。Mac などのクライアントは `~/.config/kamimusuhi/nodes` に resident の URL を 1 行ずつ書く。

```bash
# 1) ソースをノードへ (Mac から)
rsync -az --exclude target --exclude .git Cargo.toml Cargo.lock rust-toolchain.toml crates deploy knowledge <host>:src/kamimusuhi/
# 2) root 作業を一度だけ (ノード上)
sudo ~/src/kamimusuhi/deploy/resident/install-root.sh pi          # llm_master なら llm_master
# 3) secrets (Mac から。値は ssh stdin のみで転送)
deploy/resident/push-secrets.sh <host>
# 4) ビルド・配置・起動 (ノード上, 要 rustup)
~/src/kamimusuhi/deploy/resident/install-node.sh pi
# 5) NAS 初回だけ: マーカーとディレクトリ
mkdir -p /mnt/kamimusuhi/{memory,conversations,logs,artifacts,experiments,datasets,models,checkpoints}
touch /mnt/kamimusuhi/.kamimusuhi-nas
```

llm_master の local LLM は既存の user unit `llama-master.service`（linger 有効, `systemctl --user enable --now llama-master`）で自動起動する。

## 対話（個体と話す）

`POST /v1/kamimusuhi/talk`（Pi のみ, `dialogue` 設定）は Pi の個体ディレクトリに対して `kamimusuhi-runtime talk` を1ターンずつ実行する。
Persona Core・記憶・連続性を持つ個体として応答し、ターンは runtime 自身が記録する（同時実行は直列化）。LLM は resident の routing（HAI → llm_master）を使う。

Mac などから:

```bash
cargo build --release -p kamimusuhi-resident && install -m 755 target/release/kamimusuhi ~/.local/bin/
kamimusuhi chat                  # ~/.config/kamimusuhi/nodes の URL を順に試す。/status, /quit
kamimusuhi chat --subject <id> --url http://PI_HOST:7860
```

token は `$KAMIMUSUHI_NODE_TOKEN` または `~/.config/kamimusuhi/node_token`（`push-secrets.sh` が生成）。`--subject` は会話履歴の区別であって認証ではない。

## Library（参照用データ）

各ノードの `libraries` に置いたディレクトリを読み取り専用で公開する。自ノードに無い library は peer へ転送されるので、どのノードからでも参照できる。

| name | 実体 | 内容 |
|---|---|---|
| `jp_market_vis` | llm_master 上の library ディレクトリ（git clone, パスはローカル設定） | 日本の上場企業間の資本・取引・提携・役員兼任・グループ関係 |

```bash
kamimusuhi library list
kamimusuhi library tree   jp_market_vis public
kamimusuhi library file   jp_market_vis pipeline/README.md
kamimusuhi library search jp_market_vis トヨタ自動車 [dir]
# HTTP: POST /v1/library {"action":"search","library":"jp_market_vis","q":"7203"}
```

`.git` と library 外のパスは返さない。file は最大 2MiB ずつ（`offset` で続き）、search は 64MiB 以下のテキストファイルから最大100件。
更新は llm_master で該当 clone を `git pull`。

## Tools（MCP 的な tool surface）と個体からの参照

resident は読み取り専用の tool を OpenAI function-calling 形式で公開する。

- `GET /v1/tools` — tool 定義と library カタログ
- `POST /v1/tools/call` — `{"name": "json_find", "arguments": {...}}` → `{"ok": true, "result": ...}`
- tool: `library_list` / `library_tree` / `library_read` / `library_search` / `json_get`（JSON pointer）/ `json_find`（要素のフィールド一致・部分一致）

個体（Pi の `runtime/individual/runtime.json`）には 2 経路で接続する（例: `runtime-reference.example.json`）。

| 経路 | 設定 | 動き | 記録 |
|---|---|---|---|
| A: ホスト参照 | `reference` | 毎ターン runtime がカタログを付け、入力中の証券コードを `json_get` で引いて `REFERENCE_MATERIAL` として Persona に渡す | lookup があれば `LibraryExcerpt` evidence（`reference_consulted`） |
| B: tool calling | `tools` | Persona の LLM に tool を提示し、要求された tool を実行して結果を返す（最大 `max_rounds` 往復） | 呼び出しごとに `ResourceResult` evidence（`tool_called`）、応答 evidence に一覧 |

どちらも外部資料扱い（信念・記憶・経験ではない）で、正本 state は変更しない。tool サーバーが落ちていても参照・tool なしで会話は続く。
GUI の `set_language_provider` で追加した言語器官には tools は付かない（CLI/resident 経路の primary と `language_providers` のみ）。

## MCP サーバー

公開情報のDB検索が空振りした場合は [chrome-web-mcp](chrome-web-mcp.md) を利用できる。
対話用 `library_search` は既定8件・最大20件に絞り、出典を保持してモデルへ返す。

resident は stdio の MCP サーバーを子プロセスとして常駐・監視し（落ちたら backoff で再起動）、tool を `mcp__<server>__<tool>` として `/v1/tools` に載せる。
自ノードに無い tool は peer へ転送されるので、Pi からも llm_master の Playwright を呼べる。呼び出しは `logs/mcp/<node>/` に記録。
npm パッケージは `/srv/kamimusuhi/mcp` にバージョン固定で入れる（起動時にネットワーク不要）。

| server | ノード | 範囲・制限 |
|---|---|---|
| `nas` (filesystem) | 両方 | `/mnt/kamimusuhi` 全体を**読み取り専用**（readOnlyHint の tool のみ） |
| `fs` (filesystem) | 両方 | `workspace/ artifacts/ experiments/ datasets/` のみ読み書き。ログ・記憶・snapshot は不可 |
| `context7` | 両方 | 外部サービス context7.com でライブラリ文書検索 |
| `obsidian` (mcpvault) | 両方 | `/mnt/kamimusuhi/knowledge/obsidian`（操作者の private vault リポジトリの clone）。読み取りは自由。Pi のみ `write_note`/`patch_note`/`update_frontmatter` を**承認制**で提供 |
| `netdata` (nd-mcp) | 両方（各ノード自身） | 読み取り専用。Netdata は localhost bind、キーは secrets.env の `NETDATA_MCP_KEY` |
| `playwright` | llm_master | headless Chromium・隔離プロファイル。`browser_run_code_unsafe` / `browser_evaluate` / `browser_file_upload` は除外 |
| `github` (github-mcp-server v1.12.2) | 両方 | `--read-only --lockdown-mode`、toolsets=context,repos,issues,pull_requests,actions。token は secrets.env の `GITHUB_PERSONAL_ACCESS_TOKEN` |

個体へ見せる tool は `runtime.json` の `tools.allowed`（`*` で前方一致）で絞る（例: `runtime-reference.example.json`, 32 個）。
`tools.core`（同じ構文, 省略時は `mcp__` 以外の組み込み tool）だけが毎ターン最初から提示され、残りの allowed は `tool_catalog`（一覧）→ `tool_enable`（有効化）で必要時に取り出す。定義は送信前に圧縮する（description 200 字・引数 description 120 字・`title`/`examples`/`additionalProperties` 除去、名前順で固定）。
`persona.provider.reasoning` は `off`（既定）/`on`/`auto`（修正・確認指示のあるターンだけ思考）。`persona.provider.extra_body` は provider 固有の要求フィールド（`messages`/`tools`/`model` 等は上書き不可）。プロンプトは静的部分（system, tool 定義, seed, 自己状態, 記憶）→ 動的部分（観測, TURN_CONTEXT, 入力）の順で並び、llama.cpp の prompt cache が効く。

```bash
# 導入（各ノード）
cd /srv/kamimusuhi/mcp && npm install --prefix . @modelcontextprotocol/server-filesystem@2026.8.31 \
  @upstash/context7-mcp@4.1.1 @bitbonsai/mcpvault@0.16.0 [@playwright/mcp@0.0.82]
./node_modules/.bin/playwright-mcp install-browser chrome-for-testing   # llm_master のみ
sudo deploy/resident/install-netdata.sh                                 # Netdata + NETDATA_MCP_KEY
```

GitHub MCP のバイナリは公式 release を checksums.txt で検証して `/srv/kamimusuhi/mcp/github/` に置く。token は `GITHUB_TOKEN_FILE=<file> deploy/resident/push-secrets.sh <host>` で配布（fine-grained・read-only・期限付きを推奨）。

## Obsidian mirror と承認制の書き換え

- Pi の定期ジョブ `obsidian-pull`（15 分ごと, `jobs` 設定）が `runtime/bin/obsidian-git.sh pull` で GitHub から fast-forward する。未コミット変更や分岐があれば上書きせず失敗として status に出す。
- トークンは `GIT_CONFIG_*` 環境変数で git に渡し、argv・`.git/config`・出力には出さない。
- Mac の変更を個体に見せるには Mac で commit/push する（mirror は GitHub が正）。
- 初期化・復旧: `obsidian-git.sh reset-to-remote`（mirror を origin に一致させる）。

### 承認フロー

1. 個体が承認制 tool（例 `mcp__obsidian__write_note`）を呼ぶと、実行されずに `current_state/approvals.json` に保留され、個体には「承認待ち（ID）」が返る。
2. 操作者が承認する: `kamimusuhi chat` 内で `/approvals` → `/approve <id>`（または `/reject <id>`）、CLI なら `kamimusuhi approvals` / `kamimusuhi approve <id>`。
3. resident が要求どおり実行し、`after_approved`（`obsidian-git.sh commit`）で commit → origin に rebase → push。Mac では pull で取り込む。

`POST /v1/approvals/decide` は loopback からでも node token 必須（ブラウザ等のローカルプロセスから承認できない）。承認は tool ではないのでモデル出力からは不可能。保留は 7 日で期限切れ。記録は `logs/approvals/`。

## タスクボード

resident（Pi）は並列に進む作業をタスクとして記録する（`current_state/tasks.json`, 変更は `logs/tasks/`）。

| 自動記録 | 前タスク（depends_on） |
|---|---|
| 対話ターン（`kind=dialogue`） | — |
| その間の tool 呼び出し（`tool`, llm_master 転送分は node=llm_master） | 対話ターン |
| 承認待ち（`approval`, 状態=あなたの判断待ち） | 要求した tool 呼び出し |
| 承認後の反映 commit/push（`commit`） | 承認 |
| 定期ジョブ（`job`, ジョブごとに 1 枚を更新） | — |

- 操作者: `GET /v1/tasks`, `POST /v1/tasks {"action":"create"|"update",...}`（GUI のタスク画面）。
- 澪（個体）: tool `task_list` / `task_create` / `task_update` で自分の計画を載せる。自動記録タスクは澪からは変更不可。
- 再起動時に進行中だった自動記録タスクは「失敗（中断）」になる。完了は 3 日表示、400 件を保持。

## アバター名

個体の `runtime.json` の `avatar_name`（例 `"澪"`）が、Persona が名乗る名前になる（未設定は「かみむすび」）。表示上の名前で、個体の identity は変わらない。

## 状態確認

```bash
kamimusuhi status            # ノード上。0=正常 1=劣化 2=resident不達
kamimusuhi status --json
KAMIMUSUHI_NODE_TOKEN=$(cat ~/.config/kamimusuhi/node_token) \
  kamimusuhi status --url http://PI_HOST:7860     # Mac など別ホストから
kamimusuhi ask "テスト"      # 実際にルーティングして応答と経路を表示
journalctl -u kamimusuhi-resident -f
```

`status` は Pi / llm_master（peer の /health とGPU）/ routing tier / NAS（書込probe・spool backlog・最終同期）/ HAI（API疎通・残高）/ K-CORE / snapshot を表示する。

## 障害時の挙動

| 事象 | 挙動 | 復帰 |
|---|---|---|
| Pi 再起動 | systemd (`Restart=always`, enabled) で自動起動、boot_epoch +1、K-CORE 再開 | 自動 |
| llm_master 停止 | Pi の routing が HAI へ、peer は DOWN 表示 | probe 成功で自動的に llm_master へ戻る |
| llm_master 再起動 | resident・llama-master とも自動起動 | 自動 |
| NAS 断 / ハング | spool に蓄積、probe は別スレッドで timeout | マーカー確認後に自動配送 |
| HAI 断 | 他tierのみ。全滅なら 503 | probe 成功で自動 |
