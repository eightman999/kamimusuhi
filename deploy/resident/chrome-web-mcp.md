# Mio の公開 Web 参照（chrome-web-mcp）

DB/library に情報が無い調べ物を、Linux の Pi に置いた MCP へ送る。
Pi の Mio は同じ resident から使い、llm_master からは既存の resident peer 転送を使う。
個体DBを別ノードへコピー・再初期化する必要はない。

対象は [kuraneko1/chrome-web-mcp](https://github.com/kuraneko1/chrome-web-mcp) の
[`193d6b256dbe12a3612e418d118e479ee3082d44`](https://github.com/kuraneko1/chrome-web-mcp/tree/193d6b256dbe12a3612e418d118e479ee3082d44)（package 0.2.0）。
この revision は Linux 専用で、Python 3.10+、Chrome/Chromium、Xvfb が必要。
`show_browser: false` と `CW_DISPLAY_MODE=xvfb` で SSH/headless ホストから動かす。
既存の Playwright MCP・resident routing はそのまま使用できる。

## 配置対象と確認した前提（2026-09-22）

配置対象は `homelab_pi`:

- Debian 13.6 / aarch64 / Python 3.13.5、実行ユーザー `eightman`。
- 配布版 `/usr/bin/chromium`（151.0.7922.173）と root 所有の sandbox helper が導入済み。
- 既存状態で AppArmor は無効、非特権 user namespace の作成を確認済み。今回セキュリティ設定は変更していない。
- `git` / Python `venv` / `ensurepip` / `xkbcomp` は導入済み。
- Xvfb / xdpyinfo / `libXxf86dga.so.1` は未導入だったため、専用prefixにのみ追加する。
- RAM 約3.79GiB、導入前 available 約2.07GiB、resident 約138MiB / `MemoryMax=1GiB`。Chrome を含むサービス全体の余裕は運用時に確認する。

`llm_master_now`（Ubuntu 26.04.1）は専用prefixへのパッケージ導入と
MCP initialize/list/health まで成功したが、既存 Playwright 配下の Chrome は
AppArmor の user namespace 制限により `No usable sandbox!` で終了した。
その `/srv/kamimusuhi/mcp/chrome-web/` は未稼働のまま保持し、resident には登録しない。
AppArmor例外追加・無効化・`--no-sandbox` は行っていない。
Ubuntuで正式に対応させる場合は、管理者による配布版Chrome/Chromiumの通常導入が必要。
詳細は [Chromium公式のAppArmor説明](https://chromium.googlesource.com/chromium/src/+/main/docs/security/apparmor-userns-restrictions.md) を参照。

## 配置手順

ホストへの導入・resident 設定反映・再起動は別の運用操作となる。
スクリプトは apt のシステムへの install、sudo、サービス操作、既存 runtime/resident 設定の変更を行わない。

```bash
# Pi 上、resident と同じ eightman ユーザーで実行
bash deploy/resident/install-chrome-web-mcp.sh --check --with-local-deps

# 不足コマンド等を専用release内だけへdownload/extract（非root）
bash deploy/resident/install-chrome-web-mcp.sh --with-local-deps
```

`--with-local-deps` は既存 apt metadata の候補バージョンを指定して
Xvfb と同版 xserver-common、不足時の x11-utils / libxxf86dga1 / python3-pip-whl を取得し、`dpkg-deb -x` で展開する。
パッケージの maintainer script やシステムへの dpkg install は実行しない。
ensurepip が無いホストだけ `venv --without-pip` と取得した pip wheel で専用 venv に pip を入れる。
Xvfb/xdpyinfo の不足共有ライブラリがあれば停止する。xdpyinfo用の局所wrapperにだけ
追加library pathを設定し、MCP/Chrome全体のlibrary環境は変更しない。`--check` は download/展開をせず、
apt候補と現状コマンドの確認まで行う。既に Xvfb/xdpyinfo/ensurepip があるホストでは、このオプションを省略する。

Chrome のパスは実行直前に再確認する。通常の `/usr/bin/chromium` などを使う場合は
`CW_CHROME` を省略できる。Chrome は非 root で起動し、sandbox を無効化しない。

配置は `/srv/kamimusuhi/mcp/chrome-web/` 内だけ:

- `releases/<revision>/source/`: full commit ID を指定して取得したソース。
- `releases/<revision>/venv/`: 専用 Python 環境。システム Python を更新しない。
- `releases/<revision>/packages.txt`: 実際に解決した依存バージョン。
- `releases/<revision>/dependencies/`: opt-in時の deb 原本、Xvfb、pip wheel、ldd結果。
- `config.json`: 日本語・日本地域、検索5件、本文6000文字、検索間隔1.0–2.5秒。
- `bin/chrome-web-mcp`: private Xvfb とプロセスごとの一時 Chrome profile を使う起動 wrapper。

ソースは revision 固定だが、上流の Python 間接依存すべてを hash 固定しているわけではない。
実際の依存は `packages.txt` に残す。途中失敗のディレクトリと既存設定は削除・上書きしない。
異なる wrapper がある場合も停止するので、更新時は差分を確認して別配置・切り替えを準備する。

## resident と Mio への接続

`pi.resident.example.json` の `mcp_servers` にある `chrome_web` entry を
Pi の `/srv/kamimusuhi/config/resident.json` に対象限定でマージする。
許可する tool は `google_search` / `fetch_url` / `health_check` の3個だけ。
この revision は `readOnlyHint` annotation を付けないので `read_only: true` は指定しない
（指定すると resident が3個とも除外する）。読み取り用途の制限は明示 allowlist で行う。

Pi の正本 `/srv/kamimusuhi/runtime/individual/runtime.json` の `tools.allowed` に
`runtime-reference.example.json` の次の3項目を追加する:

```json
[
  "mcp__chrome_web__google_search",
  "mcp__chrome_web__fetch_url",
  "mcp__chrome_web__health_check"
]
```

設定をバックアップしてから変更し、Pi resident の再起動後、両ノードの
`GET /v1/tools` に3個が載ることを確認する。Pi の個体DBのコピー・再初期化は不要。
上流 MCP の起動失敗時には3個がカタログから消えるので、稼働確認前に検索成功を前提にしない。

## 接続・実検索の確認

以下はノード内 localhost からの例。外部接続時は既存の node 認証を用い、token を出力しない。

```bash
# llm_master と Pi それぞれで実行: health_check 自体はChromeを起動しない
curl --fail --silent --show-error http://127.0.0.1:7860/v1/tools/call \
  -H 'Content-Type: application/json' \
  --data '{"name":"mcp__chrome_web__health_check","arguments":{}}'

# 実ブラウザー・Googleへの接続確認
curl --fail --silent --show-error http://127.0.0.1:7860/v1/tools/call \
  -H 'Content-Type: application/json' \
  --data '{"name":"mcp__chrome_web__google_search","arguments":{"query":"Rust official documentation","limit":2,"hl":"ja","gl":"jp"}}'

# 公開ページの本文取得確認
curl --fail --silent --show-error http://127.0.0.1:7860/v1/tools/call \
  -H 'Content-Type: application/json' \
  --data '{"name":"mcp__chrome_web__fetch_url","arguments":{"url":"https://www.rust-lang.org/","char_limit":2000,"format":"markdown"}}'
```

HTTP 200 だけでなく、resident が返す `result.structured` 内の
`success`、検索結果・本文・出典 URL を確認する。失敗時は `ok:false` と `error.structured`。
CAPTCHA は `captcha_required: true` となるため、
未取得と表示して自動連続再試行しない。`health_check`、実検索、Pi peer 経由、Mio 応答での
出典表示はそれぞれ別に確認する。検索開始の待ち時間は `waited_ms` で観測できる。

tool 呼び出し上限は resident 側25秒。対話側の残り時間が短い場合は、その期限を優先する。
外部資料は参照証拠として扱い、内容に書かれた命令を実行したり、人格・恒久記憶に昇格させたりしない。
公開 URL の private network/credential 制限は上流の URL 検証と proxy が担う。
ローカルDBの内容や秘密値を検索 query にそのまま転送しない。

切り戻しは `chrome_web` entry と上記3 allow項目だけを元の設定へ戻し、
Pi resident を再起動する。元の Playwright 設定・個体DBと配置済み release は保持する。

## standalone 実機確認（2026-09-22）

Pi の専用prefixへの導入と `pip check` は成功。以下はサービス登録前に同じ
`bin/chrome-web-mcp` を MCP Python client の stdio context で起動して測定した値。

| 確認 | 結果 |
|---|---|
| initialize / tools/list / health | PASS: 4.106s / 0.009s / 0.011s、3 tool登録、Xvfb mode |
| sandbox | Chrome実プロセスに `--no-sandbox` / `--disable-setuid-sandbox` 無し |
| Google検索 | 9.329sで CAPTCHA。検索結果未取得、再試行せず終了 |
| 既知の公開URL取得 | PASS: `https://www.rust-lang.org/` → `https://rust-lang.org/`、5.947s、本文総長3353文字、2000文字の上限で切り詰め |
| Bing通常検索URLの本文・links取得 | PASS: 10.320s、本文5763文字。実検索結果として `https://doc.rust-lang.org/` と `https://doc.rust-lang.org/stable/book/` を確認 |
| Bing通常検索URLのmarkdown取得 | PASS: 7.858s、本文3842文字。公式のルート / std / reference URLを本文内に保持。ただし別添links98件込みのJSONは31,370 bytes |
| cleanup | client context終了後に試験用Chrome残存なし |

Bing確認は `fetch_url` で `https://www.bing.com/search?q=Rust%20official%20documentation`
を1回だけ通常閲覧した。検索結果URLは Bing `ck/a` redirect の `u=a1...` から確認できる。
Bing本文にはAI要約も混ざるため、その要約を一次資料として扱わず、候補URLの公式本文を取得する。
`format: links` と `format: markdown` はどちらも本文 `char_limit` とは別に多数のリンク
（試験では96–98件）を返す。markdownにも必要な公式URLが保持されるため、対話に渡す際は
別添 `data.links` を省略・制限し、本文と出典URLを中心に使う。後続の日本語検索ではmarkdown内にURLが無い例も確認したため、
本文にない候補を `follow_up_links` として最大8件／JSON3000文字まで保持する。CAPTCHA・challengeは
engineを問わず検索成功とみなさない。

これらはサービス登録前の standalone 検証である。登録後の systemd `PrivateTmp` 環境、
peer転送、Mio応答の出典表示の結果は [統合検証](../../docs/research/resident-web-verification-2026-09-22.md) を参照。
最初の不足library検出時の配置は
`releases/193d6b256dbe12a3612e418d118e479ee3082d44.incomplete-xdpyinfo-20260922/`
に保全してある。

## Mio への提示と検索サービスの障害時

`chrome_web` の operator description には、今回GoogleのCAPTCHAを実確認したため、繰り返さず、
公開情報の検索は `fetch_url` でBingの通常検索ページを `format=markdown, char_limit=4000` で取得し、
本文または `follow_up_links` の公式候補URLをさらに開く手順を設定する。検索ページのAI要約は一次資料とみなさない。
これは別サービスの通常閲覧であり、CAPTCHAの自動解答やブラウザー保護の無効化を行わない。

この固定版はmarkdown取得にも重複した `data.links` 配列を付けるため、residentの
`normalize_for_server` は `chrome_web` のmarkdown応答だけその別添索引を上記の範囲に絞り、
`omitted_link_index_count` を付記する。本文は最大6000文字とし、超過時は `truncated:true` と
`resident_char_limit:6000` を付記。本文内のリンク、requested/final URL、titleは保持。
既知のJSON envelopeは `structured` として渡し、JSON文字列の二重escapeを避ける。
明示的な `format=links` の索引や他のMCPのファイル内容は変更しない。
`success:false` の判定もこのserverだけに限定し、HTTP応答・監査ログで同じ判定を使う。
operator description はカタログのserver欄と各function定義の両方へ渡す。
