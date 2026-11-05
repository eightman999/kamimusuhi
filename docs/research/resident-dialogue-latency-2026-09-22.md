# Resident 対話の待ち時間調査（2026-09-22）

ユーザー報告の「約72秒」に近い実ログは、2026-09-22 18:10:22–18:11:38 JST の
turn `b7e2be9f3c634ba8afa7a31e5d8d3124`（76.302秒）。報告値との完全一致は未確認。
会話本文・推論本文・認証値を本書へ転記していない。

| 段階 | 秒 | 実測 |
|---|---:|---|
| LLM 1 | 45.733 | 入力処理6.750、生成38.811（977 tokens、25.15 tokens/s） |
| library_search 3回 | 1.451 | 544 + 524 + 383 ms |
| LLM 2 | 28.867 | 入力処理17.015、生成11.687（279 tokens、23.79 tokens/s） |
| その他 | 0.251 | 起動・記録・受渡し等の残差 |
| 合計 | 76.302 | LLMが97.8% |

実モデルは llm_master の `qwen3.8-27b-q4_K_M`。両呼出しともlocal成功。
HAI fallback、タイムアウト、Jev再試行はなく、Jevはrule-based（0 ms）。

初回入力は16,338 tokens（キャッシュ11,199、新規5,139）。検索後は28,894 tokens
（キャッシュ16,334、新規12,560）。検索2件の大きな結果を各12,000文字で
`partial_json`へ切断し、再JSON化後は14,862／14,682文字に増大していた。
表示回答230文字に対し、非表示reasoningは3,188＋704文字、合計生成1,256 tokens。
主因はローカル生成速度と、検索結果の再入力処理であり、tool実行そのものではない。

調査時の提示toolは36件・定義36,411文字、当該履歴は12 messages。
tool数は調査時点のカタログを当時の完全一致filterで集計した値で、当該ターン時点との厳密一致は未確認。
llama-serverはcontext65,536、parallel1。streamはupstreamでも無効で、完成応答を待つ。

## 根拠

- Pi: `/mnt/kamimusuhi/conversations/dialogue/pi/2026-09-22.jsonl` 行13。
- Pi: `/mnt/kamimusuhi/logs/routing/pi/2026-09-22.jsonl` 行47–48。
- Pi: `/mnt/kamimusuhi/conversations/pi/2026-09-22.jsonl` 行47–48（本文を含むため集計だけ使用）。
- Pi: `/srv/kamimusuhi/runtime/individual/trace.jsonl` 行249・253。
- llm_master: `llama-master.service` journal 18:11:08／18:11:38 JST の
  `prompt eval time`、`eval time`。

## 対応

- ユーザー指定によりPiのroutingをHAI `qwen3.8-27b-uncensored`優先、llm_master fallbackへ変更。
  比較条件と実測は [HAI比較](resident-hai-comparison-2026-09-22.md) を参照。
- 対話向け `library_search` は既定8・最大20件。出典path/line/snippetを有効なJSONのまま保持し、
  `truncated` なら語やpathを絞る。直接library APIの既定100件は維持。
- tool一覧のprefix allowlist不一致を修正。意図されていたContext7/GitHub等も提示されるため、
  提示定義量は従来と変わる。旧36件の測定と変更後を同一条件の比較にしない。
- DBで不足する公開情報はPiのchrome-web-mcpで検索・閲覧し、出典URLを付ける。
  CAPTCHAや `success:false` は失敗として扱う。

本文の逐次配信、reasoningの無効化、品質とのトレードオフ評価は本変更に含めない。
