# Provider routing gate — 低遅延 + 追加課金最小化 (2026-09-23)

`provider-latency-bench-2026-09-23.md` の続き。provider 層に料金区分と
ルーティングポリシーを実装し、本番規模 (16–28k prompt tokens) の
実測で検証した。人格・memory・conversation core のセマンティクスは
不変 — 変更は provider/routing 層のみ。後続の production 接続修正では resident も同じ RouteGate / CostGuard を使用する。

## 実装内容

| ファイル | 内容 |
|---|---|
| `crates/kamimusuhi-runtime/src/route_gate.rs` (新規) | `BillingClass` / `RouteLane` (5 chat lanes + CODE/RESEARCH/BACKGROUND agent lanes) / `ProviderHealth`+`ProviderStateBook` (EWMA TTFT, 429 cooldown, failure rate, cached tokens) / `RouteGate::select` / `probe_agent_lanes` |
| `crates/kamimusuhi-runtime/src/provider_bench.rs` | `ProviderSpec` + `billing`/`privacy_ok_for_private_memory`/`context_limit_tokens`/`approx_tpm_limit`/`reasoning_suppression`; `CostCaps` (request/session/daily/monthly) + `CostLedger` (`.local` 永続化); `prompt_body_sized` (履歴パディングで本番規模 prompt); `BenchSample` + `cached_tokens`/`cold`/`route_reason`; `ProviderSummary` + `cold_ttft_ms`/`warm_p50_ttft_ms`; FreeTier provider が live catalog で有料化されたモデルを選ばない防御 |
| `crates/kamimusuhi-runtime/src/bin/provider-bench.rs` | RouteGate 駆動の候補選択、`--prompt-tokens`/`--private`/`--ledger`/`--max-daily-cost-usd`/`--max-monthly-cost-usd`; Jev shadow は生成と並列のスレッド実行 (hot path 非直列) |

## Provider 料金区分

| provider | class | private prompt 可 | 備考 |
|---|---|---|---|
| llm_master | `Local` | yes | 夜間休業中のため skip 維持 |
| HAI | `Subscription` | yes | marginal cost = 0 (未認証で未計測) |
| Groq | `FreeTier` | **no** | approx TPM 15k。8k+ prompt で実測 HTTP 413 |
| OpenRouter | `FreeTier` | **no** | `:free` モデルのみ宣言 + catalog 有料化防御 |
| OrcaRouter | `FreeTier` | **no** | 上流が reasoning し抑制フィールド無し |
| Gemini (OpenAI 互換 endpoint) | `FreeTier` | **no** | キー未提供のため未計測 |
| Cerebras | `Metered` | yes | $0.25/$0.69 per Mtok (gpt-oss-120b)。唯一の従量 fallback |
| OpenAI | `Metered` | yes | データ共有ベースの無料特典は前提にしない |
| orcarouter/auto | `Metered` | **no** | `--orca-auto` 専用、上流の data terms 不明 |

## Routing 順序 (RouteGate::select)

Chat lanes の候補順序:

```
FastChat:  FreeTier → Subscription → Local → Metered
           (usable free → HAI/local → Cerebras)
LocalChat: Local のみ
Deep*/Tool/Memory: Subscription → Local → Metered → FreeTier
```

Eligibility (順位より先に適用、すべてハード条件):

1. **privacy**: `private` ターン (persona memory / private history /
   durable self) は `privacy_ok_for_private_memory=false` を構造的に除外
2. **context**: `prompt_tokens > context_limit_tokens` または
   `> approx_tpm_limit` → 除外 (Groq を 16k+ に送らない根拠)
3. **health**: 429 cooldown (60s)、window failure rate >50% (min 3 req)、
   FastChat では EWMA TTFT > 30s ceiling → 除外
4. **billing class 順** → class 内は reasoning suppression 可能 →
   実測 EWMA TTFT の順
5. 各候補に `RouteReason` (primary/billing/context/privacy/health
   fallback) を付記

Agent lanes は chat provider に混ぜない。`probe_agent_lanes()` は PATH 上の
公式 CLI の存在だけを記録 — 実環境では `codex exec` / `claude -p` /
`gemini -p` が全て利用可能 (headless 実行は未実装、将来の lane)。
非公式 OAuth 流用・サブスク認証の API キー化は行わない。

Jev 分類は shadow 継続。bin では分類を別スレッドに spawn し生成と
並列実行 — hot path に 0.6–0.7s を直列追加しない構造を実装済み。

## 16–28k prompt ベンチ結果

persona prompt assembly + 履歴パディング。実測 prompt tokens は
usage 記録値 (est は保守的に bytes/3)。

### v1 ラン (2k/8k/16k/24k/28k × 3 reps)

| size | groq | openrouter free | orcarouter free | cerebras (metered) |
|---|---|---|---|---|
| ~1.6k | 484ms TTFT | qwen 429 → llama 1.4s | 8.4–24.5s | 328–2383ms |
| ~10.5k | **413 ×3** | qwen 429 → llama 1.4s | 16.7s (1 malformed) | 457–813ms |
| ~12.6k | gate 除外 | qwen 429 → llama 1.4s | 12.8–48.7s | 453–689ms |
| ~21.6k | gate 除外 | qwen 429 → llama 1.8s | 9.9–23.1s | 1012ms→429→qwen 707ms |
| ~21.6k | gate 除外 | qwen 429 → llama 1.8s | 3.3–24.8s | cooldown 中 |

### v2 ラン (16k/28k × 2 reps、free-only モデルリスト後)

| size | openrouter free | orcarouter free | cerebras |
|---|---|---|---|
| ~12.6k | qwen:free 429 ×2 | 16.2s (1 stream reject) | 617–1142ms |
| ~21.6k | qwen:free 11.5s (429→retry 成功) | 13.1–18.4s (primary) | 915ms→429→qwen 1441ms |

実測の要点:

- **Groq は 8k+ で機能しない**: 実測 HTTP 413 (TPM 超過)。gate の
  `approx_tpm_limit=15k` + health 記録で以後のサイズから自動除外。
  「長文を無条件に Groq へ送らない」は推測ではなく実測。
- **Cerebras が 16–28k 帯の事実上の主経路**: warm TTFT 450–1450ms、
  decode 4.7k–17k tok/s、prompt cache 有効 (cached 11.5k–12.5k)。
  24k で一度 429 → 同provider内 fallback (qwen-3.8-27b) で代替。
- **OpenRouter `:free` は飽和**: qwen:free は 429 常態、偶発成功 11.5s。
- **OrcaRouter free は遅いが生きる**: TTFT 3–48s (thinking 支配、
  rch 最大 2840字)。30s ceiling 内のため eligible 維持だが会話には遅い。
- v1 で OpenRouter の paid fallback (llama-8b) が $0.0046 課金されたことを
  検出 → **ポリシー違反として修正済み**: FreeTier spec は `:free` のみ +
  catalog 有料化ガード (テスト `free_tier_never_selects_a_priced_model`)。

## Cerebras が実際に発火する条件

`billing_fallback` として発火するのは、unmetered 候補が全て ineligible
のときのみ:

- private ターン → free tier 全除外 (persona prompt はデフォルト private)
- prompt が free provider の context/TPM 超過 (Groq: >15k est)
- free provider が 429 cooldown / failure rate >50% / EWMA TTFT >30s
- HAI (subscription) が未認証または不健康、llm_master が休業

今回のランでは Cerebras が発火したのは正にこの条件 —
free が 429/timeout/latency で脱落した各 size で billing_fallback 記録。

## 想定月額コスト

Cerebras gpt-oss-120b @ ~21k prompt + ~70 completion ≈ **$0.0053/turn**。

- 全 turn が Cerebras に落ちる最悪ケース: daily cap $0.10 で ~18 turn/日、
  monthly cap $2.00 で ~370 turn/月のハード上限
- 現実的 (free paths が 6 割供給、HAI 健全): $0–0.5/月
- **上限保証: $2.00/月を ledger が超過拒否** (daily $0.10 も併設)

今回の実測支出: $0.0421/day (session $0.0119 + $0.0303)。

## Gate / 検証

- `cargo fmt --check` / `clippy --workspace --all-targets -- -D warnings` /
  `cargo test --workspace` / core dependency boundary: 全パス
- route_gate テスト 8 件: 順序・privacy・context/TPM・429 cooldown・
  failure rate・latency ceiling・reasoning penalty・LocalChat 制約
- CostGuard テスト: request/session/daily/monthly 窓 + ledger 永続化
- secret leak: 全キーファイル値 × report/tracked file 3269 件照合 → 混入なし

## GUI/TUI への表示 (追記)

ルーティング先と額を各応答に表示する導線を実装した:

- `TierConfig` に `billing` (`local`/`subscription`/`free_tier`/`metered`)
  + `input_usd_per_mtok`/`output_usd_per_mtok` を optional で追加
  (deploy/resident の実設定にも反映: hai=subscription, local/llm_master=local)
- `router.rs`: 上流 `usage` から prompt/completion/cached tokens と
  実費 (`usage.cost_usd`/`cost`) を抽出。実費 > tier価格によるestimate >
  不明(null — 不明を0円とは表示しない) の順で `kamimusuhi_route` メタと
  `RouteEvent` に記録
- `dialogue.rs`: `/talk` 応答と `conversations/dialogue` スプール・history
  に `route` オブジェクトを付与 (旧 `tier` フィールドは互換維持)
- 表示: `status::route_target_label`/`route_cost_label` を共有し、
  TUI 応答行メタ (`via cerebras/gpt-oss-120b · ~$0.0042 · cache 12544tok`)、
  desktop バブルの chip、`status` の last request 行に額・billing を表示。
  metered は `$x.xxxx` / 未確定は `従量`、subscription=サブスク、
  free_tier=無料枠、local=local。料金不明の plan は何も出さない

## Production 接続（後続修正）

resident router も benchmark と同じ `RouteGate` を使用する。persona request は
既定 private とし、tier ごとの privacy/context/TPM/billing/health 条件を送信前に適用する。
FAST_CHAT は production 設定で 4,000ms ceiling。実リクエストの応答時間を保守的な
latency 観測として常駐 `ProviderStateBook` に戻す。

従量 tier は `CostGuard` を production にも接続し、request/session/daily/monthly の
各 cap を送信前に確認する。既定値は $0.02/request, $0.50/daemon session,
$0.10/day, $2.00/month。ledger は `current_state/routing-cost-ledger.json` に永続化する。
従量 tier は input/output 単価の宣言を必須とし、未宣言なら送信しない。

**注意:** これは宣言された単価に対するローカル上限であり、provider が予告なく価格を
変更した場合まで請求額を暗号学的に保証するものではない。実費が response に含まれる場合は
実費で ledger を更新するが、厳密な請求上限は provider 側 spending limit も併用する。

## 残課題 / 次に試す価値がある変更

1. 実 Pi / llm_master へ production 接続版をデプロイし 4s SLA と failover を再計測
2. Jev 分類結果を曖昧ケース限定で本 routing に使用 (現在 shadow / host hint のみ)
3. Agent lane の公式 headless 実装 (codex exec / claude -p / gemini -p)
4. Gemini API free tier 実測 (キー未提供のため未検証)
5. HAI credential 環境での同条件計測 (subscription path の実レイテンシ)
