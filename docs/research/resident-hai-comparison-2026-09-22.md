# Pi resident の HAI primary 切替と実 API 比較（2026-09-22）

ユーザー承認により、Pi の resident を HAI `qwen3.8-27b-uncensored` primary、`llm_master` fallback に切り替えた。実 API の隔離 E2E は、短い挨拶 4.072 秒、既存の DB 未収録質問 21.888 秒だった。元の 76.302 秒より短いが、履歴・提示ツール数を減らした条件なので、71.3% の短縮をモデル変更だけの効果とは扱わない。

**21.888 秒は初期の成功例 1 回の値で、常時性能ではない。** 後続の統合 E2E では provider timeout と HAI の HTTP 429 を観測した。resident の fallback 待機時間がクライアント deadline を超える設定も見つかり、HAI primary を維持したまま timeout を短縮した。その後の E2E も HAI の本文が空の応答で失敗したため、空応答を検出して fallback する修正を加えた。この追加修正はローカルfixtureと後述の実API試験で確認した。連続利用時の安定性能は未確認。

最終的には空応答fallbackを実機へ反映し、本番と同じ69 toolの設定でWeb検索・公式本文取得・出典付き回答まで成功した。
所要106.983秒で、低遅延化は未達。経路はHAI→HAI空応答からlocalへfallback→local→HAI。
以下の初期試験・途中経過に続く結果は [統合検証](resident-web-verification-2026-09-22.md) を参照。
安定性能・連続利用の評価は引き続き未確認。

## 初期の実機変更と確認

- 対象: `homelab_pi:/srv/kamimusuhi/config/resident.json` の `tiers` 配列順のみ。
- backup: `/srv/kamimusuhi/config/resident.json.backup-20260922T095231Z`。
- 変更前 SHA-256: `6d76eb60f9756a1f4b9aba62b2dfd9dc9da685621e37fda7a081e8b34acbd1d7`。
- 変更後 SHA-256: `09dc4af811cef9c8e49cbcd81b4bcfd9cc4cce2b46cd3b4b7af1971ec8eb0635`。
- 対話・tool 実行中タスクと pending approval がないことを確認し、一時設定への `kamimusuhi check-config` PASS 後、元ファイルの変更有無を照合して atomic replace。
- 2026-09-22 09:52:31 UTC / 18:52:31 JST に `kamimusuhi-resident` のみ restart。
- `/status`: `order=[hai,llm_master]`、`active_primary=hai`、両 tier healthy、NAS healthy、K-core running。
- resident 配下の K-core 子プロセスは PID `2454421` → `2492957` に再起動。GPU・llama・K-core 個別 unit および他ノードのサービス操作は実施していない。llm_master resident の `started_at=2026-09-22T08:59:12Z` は維持。
- llm_master の local model 設定は `qwen3.8-27b-q4_K_M`。HAI 側とモデル名・量子化・実行基盤が同一とはみなさない。

## 後続の可用性問題と deadline 修正

以下は同日の統合作業担当者からの実行結果報告に基づく追記であり、この追記のための API 再試行・実機再確認は行っていない。上記 SHA-256 と PID は初期切替時点の記録で、以下の追加変更後の状態を表さない。

| 後続確認 | 結果 | 対話への影響 |
|---|---|---|
| 統合 E2E | provider timeout 設定 180 秒に対し、失敗までの実測約 193 秒 | クライアントへ返答できず失敗 |
| 同リクエストの resident routing | HAI を 300 秒待機後、llm_master で成功。計 317,091 ms | fallback 自体は成功したが、クライアントの待機終了に間に合わない |
| 小さい HAI 直接リクエスト | HTTP 429 | 初期の正常応答だけでは継続的な利用可能性を保証できない。制限の具体的な原因・解除時刻は未確認 |

原因の一つは、HAI tier の timeout 300 秒が provider 側の 180 秒より長く、fallback 開始前にクライアントが待機を終える deadline の不整合だった。

統合作業担当者は Pi の `resident.json` を **HAI `timeout_secs=30` / llm_master `timeout_secs=120`** に変更し、resident を再起動した。HAI primary / llm_master fallback の順序は維持。変更前 backup は `/srv/kamimusuhi/config/resident.json.backup-timeouts-20260922T102154Z`。

2 tier の設定 timeout 合計は 150 秒となり、provider の 180 秒より短くなる。ただし実測上の deadline 内完了を保証するものではなく、通信処理の時間や複数回の model/tool 呼び出しを含む対話全体の上限でもない。**設定の修正済みと、修正後の E2E 成功・安定した低遅延は区別する。** HTTP 429 の解消や連続利用時の成功率・P50/P95 は追加検証が必要。後述の実API試験では空応答からのfallback完了を確認した。

## timeout 短縮後の空応答と router 修正

統合作業担当者の公開 Rust 資料による試験は、runtime 全体で 12.526 秒後に `MALFORMED_RESPONSE: empty expression` となった。以下の HAI 応答 metadata は、既存 resident 会話ログの行 56–57 を読み取り再確認した。追加の推論 API 呼び出しは行っていない。

| 時刻 UTC | HAI 推論時間 | prompt / completion tokens | 応答形 |
|---|---:|---:|---|
| 10:25:45Z | 3.699 s | 5,563 / 64 | 正規 `library_search` tool call。参照 DB 検索は成功 |
| 10:25:54Z | 7.967 s | 5,578 / 198 | `content` は空、`reasoning_content` 463 文字、`tool_calls` なし |

最初の tool call は正常に生成・実行されたので、HAI が function calling を一律に扱えないという結果ではない。2 回目は思考内容のみで、表示できる最終応答も実行できる次の tool call もなかった。思考内容は最終回答・実行指示には転用しない。

送信コードは `model` / `messages` と tool schema を渡し、resident がモデル名を置換して `stream=false` にする。`max_tokens` / `max_completion_tokens` / `reasoning_effort` の追加指定はない。当時のログは `finish_reason` を保存していないため、上流の生成終了・parser・出力打切りのどれが原因かは確定できない。

直接のホスト側不具合は、`router::call_tier` が HTTP 200 と非空の `choices` だけを成功条件として、空応答でも fallback を止めていた点。`crates/kamimusuhi-resident/src/router.rs` に次を実装した。

- 最初の choice に非空本文、非空 `refusal`、または正規の function `tool_calls` がある場合だけ応答として受け入れる。拒否応答を理由に別モデルへ回さない。
- 白文字のみの本文で tool call / refusal がない応答は malformed として既存の次 tier へ進む。reasoning のみでも成功にはしない。
- 会話ログに `finish_reason` と `response_kind` を追加。`finish_reason` は既知値のみを採用し、任意の provider 文字列は `unknown` にする。malformed の routing error にもこの閉じた分類だけを記録し、生成本文・思考本文は転記しない。
- SSE にも `refusal` を保持する。

検証は `cargo test -p kamimusuhi-resident router::tests --lib` **9 PASS**、`cargo clippy -p kamimusuhi-resident --lib --tests -- -D warnings` **PASS**。HTTP fixture で空の HTTP 200 から次 tier の正常応答への fallback、正規 tool call / refusal の保持、任意 provider 文字列の診断への非流出を確認した。その後、実機へ配置し、上記リンク先の実API E2Eでも空応答からのfallbackを確認した。

### HAI 固有の設定を追加する必要性

現時点では必要と断定できない。HAI の現行ドキュメントは、Qwen27B の `reasoning_effort` に `none` を含む値を掲載し、省略時はモデル既定値、出力上限パラメータ省略時は追加上限を付けないとしている。`reasoning_effort=none` の比較は今後の候補だが、今回の空応答を直すことは未検証なので設定を追加していない。[HAI API ドキュメント](https://hai.hcloud.ltd/docs)、[モデル仕様](https://hai.hcloud.ltd/models/qwen3.8-27b-uncensored)。

HAI は過去の tool schema / thinking の HTTP 400 問題を修正済みと告知しているが、今回は HTTP 200 の本文欠損で症状が異なる。過去の回避設定をそのまま適用する根拠はない。[tool schema 修正](https://hai.hcloud.ltd/news/tool-schema-400-fix)、[thinking 修正](https://hai.hcloud.ltd/news/qwen-thinking-error-fix)。

9 月 21 日の HAI の HTTP 429 修正告知は、主に `stream=true` の切断による同時実行枠の未解放が対象。こちらの送信は `stream=false` のため今回の 429 と同一原因とは断定しない。[HAI 同時実行枠の修正告知](https://hai.hcloud.ltd/news/concurrent-slot-leak-fix)。

## 比較結果

原典は既存ログのみ。質問本文・元 prompt・返答生ログは本資料へ転載していない。

| ケース | wall time | model 時間 | tool 時間 | 結果 |
|---|---:|---:|---:|---|
| 元 DB 質問・llm_master | 76.302 s | 45.733 + 28.867 = 74.600 s | `library_search` 3 回、1.451 s | 既存実対話ログ |
| HAI・短い挨拶 | 4.072 s | 3.939 s | 0 | PASS、短い日本語挨拶 |
| HAI・同じ DB 質問、隔離 E2E | 21.888 s | 4.647 + 3.979 + 10.790 = 19.416 s | `library_search` 4 回、2.295 s | PASS、DB の未収録と現在情報の未確認を明示 |
| 元 model 入力 47 を HAI 再生 | 12.167 s | 12.162 s | 実行なし | HTTP 200、`json_find` を要求 |
| 元 model 入力 48 を HAI 再生 | 15.725 s | 15.719 s | 実行なし | HTTP 200、未確認と回答 |

HAI E2E の各 `library_search` は 763、738、564、230 ms、全件成功。E2E の応答は資料の範囲外と明示し、未実施の Web 検索や最新情勢を捏造していない。再生 48 の最終応答も現状は未確認と表現した。UI の描画・音声開始時間は本試験の測定対象外。

元入力 2 件の model 時間は合計 74.600 → 27.881 秒。ただし以下の条件差があるため、この 62.6% 短縮もモデル単独の厳密な A/B ではない。

### 入力規模・token 数

| ケース | messages | content 文字数 | prompt tokens | cached tokens | completion tokens | reasoning 文字数 |
|---|---:|---:|---:|---:|---:|---:|
| 元 47・llm_master | 15 | 16,238 | 16,338 | 11,199 | 977 | 3,188 |
| 元 48・llm_master | 19 | 45,895 | 28,894 | 16,334 | 279 | 704 |
| HAI 挨拶 | 3 | 11,897 | 4,492 | 64 | 125 | 443 |
| HAI DB E2E・1 | 3 | 13,134 | 5,122 | 64 | 192 | 408 |
| HAI DB E2E・2 | 6 | 13,364 | 5,304 | 1,856 | 193 | 428 |
| HAI DB E2E・3 | 9 | 28,021 | 11,152 | 5,248 | 518 | 1,673 |
| HAI 元 47 再生 | 15 | 16,238 | 6,904 | 0 | 272 | 642 |
| HAI 元 48 再生 | 19 | 45,895 | 19,548 | 1,856 | 283 | 870 |

content 文字数は `messages[*].content` の文字数合計で、tool schema や `tool_calls` の構造文字数を含めない。provider の tokenizer・テンプレート・キャッシュ条件が違うため token 数の差を純粋な prompt 削減量とはしない。

## 隔離・条件差

- SQLite backup API により canonical DB を `/dev/shm/mio-hai-eval-*` に読み取りコピー。ケースごとに独立コピーと test subject を使用し、終了後に RAM 上のコピーを削除。
- runtime 設定は複製し、提示・実行可能な tool を `library_list`, `library_search`, `library_read`, `json_get`, `json_find` に限定。書き込み・承認・外部 MCP tool を除外。
- `max_rounds=3`（元は 8）、新 subject のため過去の会話履歴なし。基盤の個体 DB と自動参照設定はコピー元を維持。
- 実行 binary は当時 Pi に配置済みの `kamimusuhi-runtime`。新規コードを build/deploy した試験ではない。
- localhost resident の実際の routing・HAI 接続・library 実装を利用。秘密値を取得・表示・保存していない。
- runtime のテスト発話は canonical 個体 DB / canonical dialogue journal へ書き込んでいない。終了時点の canonical dialogue journal は元と同じ 13 行。resident 自身の既存 request・tool・routing ログには試験呼び出しが残る。
- 再生試験は元ログ 47 / 48 の `messages` を無変更で使用。ログに保存されていない `tools` は現行の上記 5 種の schema から再構成した。モデルの tool 要求は実行せず、2 件を独立して再生したため、実際の対話ループ全体の再現ではない。
- 初期比較は一度ずつの観測。後続の統合 E2E / fallback では上記の失敗と deadline 超過、timeout 短縮後の空応答を観測しており、初期の成功結果で置き換えない。空応答fallback修正後の実API E2Eは統合検証で確認。P50/P95、連続利用、GUIは未検証。

## 原典

- 元対話: `/mnt/kamimusuhi/conversations/dialogue/pi/2026-09-22.jsonl` 行 13（76,302 ms）。
- 元推論: `/mnt/kamimusuhi/conversations/pi/2026-09-22.jsonl` 行 47–48。
- HAI E2E の resident request ログ時刻: `09:53:56Z`, `09:54:01Z`, `09:54:07Z`, `09:54:18Z`。
- HAI 再生 request ログ時刻: `09:55:32Z`, `09:55:48Z`。flush 前は `/srv/kamimusuhi/spool/conversations/pi/2026-09-22.jsonl`、flush 後は NAS の同カテゴリ。
- 後続の timeout / HTTP 429 と設定変更: 同日の統合作業担当者からの報告。具体的な後続ログの行番号は本追記では確認していない。
- timeout 短縮後の空応答: `/mnt/kamimusuhi/conversations/pi/2026-09-22.jsonl` 行 56–57。runtime 全体の 12.526 秒と失敗分類は統合作業担当者の報告。

## 秘密値不要の再試験手順

Pi 上で次の Python を実行する。試験対象の resident は既に HAI primary であることが前提。外部検索追加後の試験では `allowed` を承認済みの読み取り tool に明示的に拡張する。空 allowlist は全 tool を許可するため使用しない。出力は集計値のみで、生の返答は保存しない。

```python
import json
import pathlib
import sqlite3
import subprocess
import tempfile
import time
import urllib.request

base = pathlib.Path('/srv/kamimusuhi/runtime/individual')
url = 'http://127.0.0.1:7860'
status = json.load(urllib.request.urlopen(url + '/status', timeout=8))
assert status['routing']['active_primary'] == 'hai'
question = json.loads(pathlib.Path(
    '/mnt/kamimusuhi/conversations/dialogue/pi/2026-09-22.jsonl'
).read_text().splitlines()[12])['message']
config = json.loads((base / 'runtime.json').read_text())
config['tools']['allowed'] = [
    'library_list', 'library_search', 'library_read', 'json_get', 'json_find'
]
config['tools']['max_rounds'] = 3
with tempfile.TemporaryDirectory(prefix='mio-hai-eval-', dir='/dev/shm') as tmp:
    for label, message in [('greeting', 'こんにちは、澪。短く挨拶して。'),
                           ('db_miss', question)]:
        target = pathlib.Path(tmp) / label
        target.mkdir()
        (target / 'runtime.json').write_text(json.dumps(config, ensure_ascii=False))
        src = sqlite3.connect('file:' + str(base / 'kamimusuhi.sqlite') + '?mode=ro',
                              uri=True)
        dst = sqlite3.connect(target / 'kamimusuhi.sqlite')
        src.backup(dst)
        dst.close()
        src.close()
        start = time.monotonic()
        result = subprocess.run([
            '/srv/kamimusuhi/runtime/bin/kamimusuhi-runtime', 'talk',
            '--dir', str(target), '--subject', 'hai-eval-20260922-' + label,
            '--privacy', 'unconstrained', '--message', message
        ], capture_output=True, text=True, timeout=190)
        elapsed_ms = round((time.monotonic() - start) * 1000)
        values = []
        for line in result.stdout.splitlines():
            try:
                values.append(json.loads(line))
            except json.JSONDecodeError:
                pass
        reply = next((v for v in reversed(values) if 'response' in v), {})
        current = json.load(urllib.request.urlopen(url + '/status', timeout=8))
        print(json.dumps({
            'case': label, 'exit': result.returncode, 'total_ms': elapsed_ms,
            'response_chars': len(reply.get('response', '')),
            'tools': [{k: call.get(k) for k in ['name', 'latency_ms', 'ok']}
                      for call in reply.get('tool_calls', [])],
            'route': current['routing']['last_route']
        }, ensure_ascii=False), flush=True)
```

各リクエストの model 時間・usage は上記既存 resident ログから、試験時刻に対応する metadata だけを取得する。`last_route` は最後の model call のみなので、複数 call の合計には使用しない。
