# Provider レイテンシ比較 + routing 基盤（2026-09-23）

Kamimusuhi対話の低遅延化のため、複数 OpenAI-compatible provider を同じ
prompt・同じ計測経路で比較した。目的は最速モデル探しではなく、
local / fast cloud / reasoning を状況で使い分ける基盤の検証である。

## 実装

- `kamimusuhi-resource-http`: `post_json_stream` を追加。chunked /
  content-length 双方を逐次配送し、headers到着時刻（TTFB）と最初の
  body byte を返す。buffered経路と接続・TLS・framingを共有。
- `kamimusuhi-persona-http`: `request_body` を公開。benchは本番と同一の
  prompt assembly（persona seed・履歴・observed_runtime・conversation
  core）を使う。実測 prompt は約1,520–1,735 tokens。
- `kamimusuhi-runtime::provider_bench`: provider registry（base_url、
  モデル候補、USD/1M価格表、capability flag）、`KeyStore`（鍵は
  プロセスメモリのみ・Debugはprovider名のみ）、`BenchClient`
  （chat / chat_stream / probe / catalog を全providerで共有）、
  `CostGuard`（request $0.02・total $0.50の事前estimate検査）、
  `classify_route`（Jev `/v1/systemone` への shadow routing）、
  p50/p95 集計。`provider-bench` bin が実行入口。
- Jevの分類結果は shadow のみ。実route選択には一切影響しない。

秘密情報は `--key provider:path` で実行時にファイルから読み、レポート・
stdout・error・fixture に出力しない。`GET /models` probe は無料。

## 計測条件

- prompt: 「こんばんは」「今日も終わるからご挨拶をと思ってね」
  「もう少しお返事が早くならないかと思ってるんだけどどうかな」
- 各 provider × prompt × 3 rep、stream、max_tokens 768、
  reasoning系は low/disabled 相当を要求
- llm_master は夜間休業のため不実行。HAI は当環境に credential なし。

## 結果（persona prompt 約1.6k tokens）

| provider | model | p50 TTFT | p50 total | decode tok/s | cost/turn |
|---|---|---:|---:|---:|---:|
| groq | gpt-oss-20b | ~390ms | ~405ms | ~2100 | ~$0.00013 |
| groq | gpt-oss-120b（429 fallback） | ~520ms | ~620ms | ~690 | ~$0.00027 |
| cerebras | gpt-oss-120b | ~410ms | ~440ms | ~4300 | ~$0.00042 |
| cerebras | qwen-3.8-27b（fallback） | ~505ms | ~535ms | 高速 | 価格不明 |
| openrouter | qwen3.8-27b:free | ~11s | ~11.2s | ~1900 | $0 |
| orcarouter | glm-5.3-flash | ~18.6s | ~19.3s | ~500 | ~$0.0002 |
| orcarouter | deepseek-v4-flash-free | ~4.8s | ~4.8s | burst | $0 |
| orcarouter-auto | auto → qwen3.7-plus | ~16.4s | ~16.6s | ~3900 | ~$0.0013 |

p95は3サンプルの参考値。総支出 $0.018（cap $0.50）。

- first event（network+queue）と TTFT（最初のcontent delta）を分離した
  ことで、orcarouter 系の遅さが upstream の **thinking時間** であると
  特定できた。`reasoning_chars` は glm で575–2215、deepseek-free で
  53–2853に達し、回答本文より思考が支配的だった。
- groq / cerebras は `reasoning_effort: low` が効き thinking は17–108字に
  抑制。free/ルーティング経由の upstream は reasoning 抑制を無視した。
- OpenRouter free tier はこの時刻帯で飽和気味（429、そして通っても
  thinking込み8–22s TTFT）。
- `orcarouter/auto` は全promptを qwen3.7-plus（思考型）へrouteし、
  TTFT 13–28s・$0.001–0.002/req・TIMEOUT 1件。

## Jev shadow routing

| prompt | route | confidence | latency |
|---|---|---:|---:|
| greeting | FAST_CHAT | 0.96 | 592ms |
| closing_report | FAST_CHAT | 0.98 | 609ms |
| latency_question | FAST_CHAT | 0.92 | 745ms |

Jev分類は約0.6–0.7sで安定。実routingには未使用（shadowのみ）。
classify_route は生成呼出しと独立しており、将来 prompt assembly と
並列実行できる形状。

## HAI/llm_master との対比

既存調査（resident-dialogue-latency / resident-hai-comparison,
2026-09-22）: llm_master local 約76s/turn（~25 tok/s）、HAI
qwen3.8-27b-uncensored 4–22s（429・空応答を観測、stream無効）。
今回の groq/cerebras は実persona promptで TTFT 0.3–0.9s。
prompt規模差（本番16–28k vs 計測1.6k tokens）を割引いても、
fast cloud tier が会話応答の主経路候補として成立する。

## 観測された互換性差

- `chat_template_kwargs`（vLLM固有）は Groq/Cerebras/OrcaRouterで400。
  spec側で `accepts_chat_template_kwargs` flag を設けてstrip。
- reasoning抑制の方言: gpt-oss系は `reasoning_effort`、GLM系は
  `thinking.type`、vLLM系は `chat_template_kwargs.enable_thinking`。
  providerが upstream 互換でない限り統一fieldは存在しない。
- usage内のcost field: OrcaRouter `cost_usd`、OpenRouter `cost`。
- SSE deltaのthinking field: `reasoning`（gpt-oss）/ `reasoning_content`
  （GLM・deepseek系）。

## 次に価値がある変更

1. FAST_CHAT経路の本実装: Jev分類→groq/cerebrasの非思考fast laneを
   shadow→限定的な本番で評価（人格・memoryは不変、provider層のみ差替）。
2. OrcaRouter autoのrouting policy指定（fast/non-thinking優先）の
   有無を公式docsで確認し、route先制約を試す。
3. 本番prompt規模（16–28k tokens）での再計測 — prompt処理速度差は
   この規模で効いてくる。
4. OpenRouterはfree帯を外すか、429時のretry/backoffを入れる。
5. HAI credentialが利用可能な環境で同一計測を行い直接比較する。

## 根拠

- レポート: `.local/provider-bench/report-1790099822.json`,
  `report-1790100404.json`, `report-orca-nonthinking.json`
  （gitignore対象、secret-free）
- 実装: `crates/kamimusuhi-runtime/src/provider_bench.rs`,
  `src/bin/provider-bench.rs`,
  `crates/kamimusuhi-resource-http/src/http.rs`,
  `crates/kamimusuhi-persona-http/src/lib.rs`
