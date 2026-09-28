# computer-mcp — computer use（GUI操作）MCP

澪が各ノードのデスクトップ画面を観測し、GUI を操作するための MCP サーバー。
標準入出力の改行区切り JSON-RPC 2.0（protocolVersion `2025-06-18`）。
実装は `deploy/resident/computer-mcp.py` 単一ファイル、Python 標準ライブラリのみ。

## ノード別の提供形態

| ノード | MCP名 | 画面 | 配置 |
|---|---|---|---|
| mac | `computer_mac` | 実デスクトップ（Aqua） | `install-mac.sh` が `$ROOT/mcp/computer/` に配置 |
| llm_master | `computer_llm` | Xvfb 仮想ディスプレイ `:99` | `install-computer-mcp.sh` が `/srv/kamimusuhi/mcp/computer/` に配置 |
| pi | `computer_pi` | Xvfb 仮想ディスプレイ `:99` | 同上 |

サーバー名はノードごとに一意にする（同名は peer 転送のカタログで衝突する）。
同一ノード上の同名サーバーはローカル解決が優先される。

## ツール面

観測系（`readOnlyHint: true`、審査なしで即時実行）:

- `health_check` — プラットフォーム、ディスプレイ、必要ツールの有無、
  macOS では `accessibility_trusted`（AX 許可）を返す
- `list_windows` — ウィンドウ/アプリ一覧
- `ui_tree` — macOS 専用。AX ツリー（role/name/value/位置）を depth≤6, limit≤1000 で返す
- `screenshot` — PNG を `COMPUTER_DIR/screens/` に保存しパスを返す
  （画像はモデルに返さない。記録と操作者確認用）
- `wait` — 最大 10 秒

操作系（`judge_required` に登録し、実行前に判定モデルが審査）:

- `mouse_move` / `click`（button: left|middle|right, clicks≤3）/ `scroll`
- `key_press` — `return` `tab` `escape` `f1`..`f12` `cmd+space` `ctrl+l` 形式
- `type_text` — 最大 8000 文字。非 ASCII はクリップボード経由（旧内容を復元）
- `activate` — アプリ/ウィンドウを前面へ
- `open_application` — macOS は `open -a`、Linux は管理ディスプレイ上で起動

## 審査（tool_judge / judge_required）

resident の `mcp_servers[].judge_required` に挙げた操作系ツールは、実行前に
`tool_judge.model`（既定 `hai` tier）へ ALLOW/DENY/ESCALATE を問い合わせる。
実運用ではローカル tier を指す（無料・非外部送信・速い）:
Pi と Mac は `llm_master` tier（= llm_master resident 経由で llama.cpp）、
llm_master は `local` tier（自前の llama.cpp）。`hai` のような推論する
モデルは判定語に到達する前に `max_tokens` を使い切りやすく、判定不能に
なりがち（その場合は承認キューに流れるので安全だが審査にならない）。

- `ALLOW` → 実行
- `DENY` → 理由付きで拒否（モデルへ返る）
- `ESCALATE` または審査不能 → `tool_judge.on_unavailable`（既定 `approval`）
  に従い operator 承認キューへ送る。つまり判定不能は「黙って実行」ではなく
  「人に聞く」に倒れる。`deny` を選ぶと即拒否。
- `judge_required` と `approval_required` の両方に載ったツールは
  直接 operator 承認へ行く（審査を潜り抜けない）。

設定例は `deploy/resident/*.resident.example.json` の `tool_judge` と
`computer_*` エントリを参照。

## バックエンド

### macOS

- `osascript`（System Events の AX 問い合わせ、JXA の ObjC ブリッジ経由
  `CGEventPost`）、`screencapture`、`pbcopy/pbpaste`、`open`。外部依存なし。
- **前提**: アクセシビリティ（System Settings > Privacy & Security >
  Accessibility）と画面収録の許可を、責任プロセス（resident から起動した
  `python3`）に操作者が付与する。未許可でも `CGEventPost` 系は動くことが
  あるが、System Events 経由の呼び出しはタイムアウトまでハングするため、
  本サーバーは AX 依存操作の前に `AXIsProcessTrusted` を確認し即時失敗する。
- `health_check` の `accessibility_trusted` で許可状態を確認できる。

### Linux

- `DISPLAY` が生きていればそれを使う。なければ `COMPUTER_DISPLAY`
  （既定 `:99`）に Xvfb を spawn し、常駐させる（pid は `COMPUTER_DIR/xvfb.pid`、
  ログは `xvfb.log`）。`-nolisten tcp`、同一ユーザの Unix ソケットのみ。
- 入力は `xdotool`、スクリーンショットは `scrot`（なければ ImageMagick
  `import`）、ウィンドウ列挙は `wmctrl` → `xdotool` の順。
- `install-computer-mcp.sh` は必要 deb を `apt-get download` →
  `dpkg-deb -x` で `/srv/kamimusuhi/mcp/computer/dependencies/` に展開する
  （apt install / sudo / dpkg データベース変更なし。chrome-web と同じ方式）。
  ランチャー `bin/computer-mcp` が PATH / LD_LIBRARY_PATH / COMPUTER_* を立てる。

## 環境変数

| 変数 | 既定 | 用途 |
|---|---|---|
| `COMPUTER_DIR` | `~/.local/state/kamimusuhi/computer` | スクショ・pidfile・ログ |
| `COMPUTER_DISPLAY` | `:99` | Linux で管理する仮想ディスプレイ |
| `COMPUTER_SCREEN` | `1280x800x24` | Xvfb の画面ジオメトリ |
| `COMPUTER_XVFB` | PATH/`$PREFIX/dependencies` 探索 | Xvfb バイナリの明示指定 |

## 運用メモ

- systemd ユニットは `PrivateTmp=yes`。resident が spawn した Xvfb の
  ソケットは resident のプライベート `/tmp` にあるため、シェルから直接
  `DISPLAY=:99` では見えない。仮想画面を手で触りたいときは resident の
  マウント名前空間に入る（例: `sudo nsenter -t $(pgrep -f 'kamimusuhi serve' | head -1) -m -U --preserve-credentials env DISPLAY=:99 xdotool getdisplaygeometry`）。
  インストーラのスモークテストが起動した Xvfb は実 /tmp 側に残るため、
  インストーラは検証後にそれを止める（resident 側が自分用に spawn する）。
- スクショは `screenshot` がローカルパスを返すので、操作者はそのファイルを
  開いて確認できる。モデルには画像を返さない（コンテキスト肥大と情報漏洩の
  抑制）。
- 仮想ディスプレイ上のアプリは `open_application`（例: `xterm`）で起動する。
  仮想画面上の状態を端末から見たいときは `DISPLAY=:99 xdotool ...` 等を
  サービスユーザで実行する。
- Mac の実デスクトップは操作者と共有の画面である。操作系は必ず
  judge_required 経由にし、危うい操作は承認キューに落ちる設計を維持する。
