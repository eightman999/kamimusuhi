# Linux Mio の公開Web参照・統合検証（2026-09-22）

Pi を正本個体の実行先とし、chrome-web-mcp を Pi の resident に登録した。
llm_master は既存の peer 転送で同じ3ツールを利用する。
導入手順・固定revision・sandbox条件は [運用手順](../../deploy/resident/chrome-web-mcp.md)、
元の待ち時間の内訳は [遅延調査](resident-dialogue-latency-2026-09-22.md)、
モデル比較とHAIの失敗は [HAI比較](resident-hai-comparison-2026-09-22.md) を参照。

## 実装と運用への反映

- DB/libraryに情報がない公開情報の質問では、Web検索・本文取得を使い、実際に取得したURLを出典にする指示を追加。
- `google_search` / `fetch_url` / `health_check` の3ツールだけを明示許可。
- Googleは実機でCAPTCHAになったため、Bing通常検索ページをmarkdownで開き、検索結果の公式本文を読む手順を設定。検索ページのAI要約は一次資料として扱わない。
- operator descriptionを各function定義にも渡す。以前はserver状態の説明にだけ載り、モデルには見えていなかった。
- `chrome_web`固有の `success:false` をHTTP応答・監査ログとも失敗にする。他MCPが読み取るJSONファイルの内容には適用しない。
- ChromeのJSON envelopeを構造化して渡す。Markdownにないリンクは最大8件／3000文字のfollow_up_linksへ絞り、残りの索引を省略。markdownは6000文字、JSON escape後のサイズも確認して本文を追加短縮する。切断を明示し、最終URL・title等を保持。
- 対話向けlibrary検索を既定8件／最大20件に制限。直接library APIの既定100件は維持。
- tool一覧と実呼出しのprefix allowlist判定を統一。以前から設定されていたContext7/GitHub等も一覧に反映されるため、以前とtool schema数は同一でない。
- HAI primaryを維持し、HAI待機30秒／llm_master待機120秒へ変更。空本文・正規tool callなし・拒否なしの応答もfallbackする。reasoning本文を回答に転用しない。

HAIのタイムアウトはmodel呼出しごとの値で、複数のmodel/toolを含む対話全体の180秒保証ではない。
HTTP 429・上流の空応答・CAPTCHAの解消自体を保証する変更でもない。

## サービス・peerの実機確認

| 確認 | 結果 |
|---|---|
| Pi systemd内のMCP起動 | PASS、3ツール登録、PrivateTmp環境で稼働 |
| llm_masterのtoolカタログ | PASS、Pi由来の同じ3ツールを確認 |
| llm_master → Pi `health_check` | PASS、52 ms |
| llm_master → Pi Rust公式本文取得 | PASS、5678 ms、本文取得と最終URLを確認 |
| private URLへのfetch | `http://127.0.0.1:7860/health` を55 msで拒否。外側okと監査ログともfalse |
| masterのlibrary検索 limit=8 | PASS、8件・truncated=true、5 ms |
| Piのresident全体メモリ | 途中確認258 MB、peak287 MB、MemoryMax 1 GiB |

これらはモデルを介さない実tool呼出しの測定。standalone Chromeの起動成功とは分けて確認した。
llm_master上のChromeは既存AppArmorで起動できず、その専用prefixは非稼働のまま。
セキュリティ設定は変更せず、Piのsandbox付きChromiumをpeer経由で使う。

## Mioの実model・toolループ

SQLite backup APIでPi正本をRAM上の一時ディレクトリへコピーし、新規test subjectで
実配置の `kamimusuhi-runtime talk` を実行した。一時DBは終了時に削除。
canonical会話への試験発話・人格記憶の追加は行わない。residentの既存監査ログには呼出しが残る。
質問は公開のRust所有権ルールについて、`jp_market_vis` を検索し、資料がなければWeb公式資料を読む内容。

| 段階 | 結果 |
|---|---|
| HAI primary・当初設定 | 約193秒でクライアントtimeout。residentは300秒のHAI待機後にlocal成功、計317.091秒で遅すぎた |
| HAI primary・待機期限修正後 | 12.526秒で空応答FAIL。初回HAIはlibrary検索を要求、2回目はreasoningのみで空本文・tool callなし |
| local強制・4ツール限定 | PASS、46.816秒。library検索0件→Google CAPTCHAを失敗扱い→Rust公式本文取得→短い日本語と実取得URL |
| HAI優先・本番と同じtool設定 | PASS、106.983秒。library検索0件→Bing取得→Rust公式本文取得→日本語と実取得URL。途中HAI空応答を検出してlocalへfallback |

local強制試験のtool時間はlibrary605 ms、Google9068 ms（失敗）、公式ページ3191 ms。
出典は `https://doc.rust-lang.org/book/ch04-01-what-is-ownership.html`。
この試験では12000文字指定により結果がpartial_jsonになり、運用メモもモデルに見えていなかったため、
上記のoperator description提示・構造化・本文上限を追加した。成功例を修正後の性能測定としては扱わない。

最終の本番設定試験は `allowed` / `max_rounds=8` を変更せず、新しいsubjectとRAM上のDBだけを使用。
提示toolは69件、定義85,292文字、最初のpromptは27,826 tokensだった。
全toolの提供・実行環境は本番と同じだが、過去の対話履歴は持たない。

| 本番設定試験の経路 | 秒 |
|---|---:|
| 初回HAI → library_search要求 | 27.569 |
| HAI空応答（finish_reason=stop）→local → Bing取得要求 | 46.737 |
| local →公式本文取得要求 | 9.989 |
| HAI →最終回答 | 7.304 |
| library / Bing / 公式本文 | 0.661 / 11.611 / 2.846 |
| その他 | 0.266 |

外部検索と引用付き回答は実機で成立したが、**低遅延化は未達**。
初期の少数tool試験21.888秒と、69種類を提示する実設定の結果を混同しない。
HAIの空応答・モデル切替に加え、大きいtool定義と入力処理が引き続き負荷となる。
元の76.302秒とは質問・Web取得の有無が違うので、単純なモデル速度比較には使えない。

Bingの日本語検索結果はsnippetを取得できたが、markdown内にURLが無かった。
Mioは既知の公式URLを開き、実取得本文に基づいて回答した。
未知の資料でも候補を辿れるよう、この観測後にfollow_up_links保持を追加した。
Bingでは生HTMLに実在したhrefのredirect先を復号して候補にし、検索ナビゲーション・privacyリンクを除外する。推測URLは生成しない。
本文取得の構造化結果は6577文字で、partial_jsonにはならなかった。

follow_up_links追加後の最終配置では、llm_master→Pi経由で同じBing検索を再確認。
11.288秒、本文711文字、候補8件、構造化結果2848文字でPASS。
取得HTML由来の候補に `https://doc.rust-jp.rs/book-ja/ch04-01-what-is-ownership.html` 等を確認した。
この最後の差分はtool実呼出しで確認し、106.983秒のmodel E2Eは繰り返していない。

Piのメモリ再確認ではresident稼働中、自動再起動カウンタ(NRestarts)0、cgroup OOM/killは0、検索・取得タブ残留なし。
MemoryCurrentは約1GiB上限付近で、anon約310MB＋shmem約191MB、残りの多くはfile cache。
memory.events.maxの増加があり、reclaim圧力はある。設定変更や強制cleanupは行っていない。
この圧力の遅延への寄与・連続利用時の余裕は未測定。

## 検証範囲

実API・実ブラウザー・Linuxサービス・peer転送と、GUIの描画／音声開始は別の確認である。
本試験は前者を対象とする。初回応答の逐次配信、reasoning_effort変更、長期連続利用の成功率・P50/P95は未検証。
HAI初期の21.888秒は履歴・toolを限定した単発値であり、常時の待ち時間として提示しない。

設定とbinaryは反映前にバックアップし、SHA-256照合・check-config・atomic replace後にresidentを再起動した。
resident配下のK-core子プロセスも再起動される。GPU/llamaサービスの再起動やDB初期化はしていない。

最終配置のバックアップ:

- master: `/srv/kamimusuhi/cache/web-deploy-backup-20260922T105003Z`
- Pi: `/srv/kamimusuhi/cache/web-deploy-backup-20260922T105038Z`

配置SHA-256:

| 対象 | SHA-256 |
|---|---|
| master resident | `232d6afe5c3b75ab759979356ac49bcb9297d202c570e857cab09a7e692a1ba0` |
| Pi resident | `c31917b97277ed9099931112bcfa78c1088154ba1cd752b5c7a392a94a61da67` |
| Pi runtime | `0a40aa33c82b066c510098d02073e8254c5833de18bed340d058501b29a17b51` |

ローカル検証: persona-http 38、resident 39、desktop 10テストPASS（計87）。
関連3 crateのall-targets clippy（warnings禁止）、fmt、差分検査、4本の運用scriptのbash構文検査PASS。
並行mergeの競合10ファイルも解消し、未解決indexは0。サイト固有設定はGit外のlocal設定へ保全。
merge commit・今回変更のcommit/pushは行っていない。
