# 実験: J72-30M と GrokBot Qwen — 二資源ルーティング

実装済みの内容と実測のみを記述する。未測定の項目は未測定と明記する。
単一資源側の経緯は [`grokbot-external-cognitive-resource.md`](./grokbot-external-cognitive-resource.md)。

## 検証したい命題

```text
Kamimusuhi decides where cognition happens.

The cognitive resource performs the cognition,
but does not own routing authority,
identity, provenance authority,
or canonical continuity.
```

モデル性能の競争ではない。同一の individual が、**データ境界と認知要求に応じて
物理的に異なる「外付け脳」を選ぶ**ことを実機で示すのが目的である。

## 構成

```text
                       Kamimusuhi
                           │
                 Cognitive Action Router
                           │
             ┌─────────────┴─────────────┐
          j72                        grokbot
   cursor VM                    cursor VM
   custom PyTorch / CPU         llama.cpp
   J72 / novllm lineage         Qwen2.5-3B Q4_K_M
             │                           │
        external                     external
```

両者とも交換可能な Cognitive Resource であり、Persona Core ではない。

### 構成変更 (2026-09-10)

当初 J72 は `llm-machine`（利用者管理・CUDA）上にあり `local_network` と宣言する計画だった。
現在は **GrokBot Qwen と同じ第三者管理 VM (`cursor`) 上で CPU 実行**されている。

したがって J72 の locality も `external` へ変更した。endpoint だけ差し替えて
`local_network` を残す選択はしていない。**機械に追随しないデータ境界は、
境界が無いことより悪い** — 強制されているように読めて、実際には強制していないからである。

この変更の代償は明確で、隠さずに記録する:

```text
no_external_service で使えるモデルが、現在ひとつも無い。
```

`llm-machine` 上へ戻せば `J72_LOCALITY=local_network` の一行で復帰する。

## locality は機械の管理者に従う

```text
operator-controlled machine  ->  local_network
third-party controlled VM    ->  external
```

**Tailscale で届くから決まるのではない。** 両者とも Tailscale 越しであり、
現在は同じ VM の上にいる。決めるのはその計算機を誰が管理しているかである。

同時に、locality は **データ境界の記述であって truth authority ではない**。
仮に自分の機械で動いていても、モデルの出力は
`EXTERNAL_RESOURCE_RESULT` / `external_material` にとどまる（Task 10）。
借りた認知は、借り先が自分の機械でも借りた認知のままである。

## J72 endpoint

```text
Tailscale hostname   cursor          (旧: llm-machine)
port                 8081
server               "J72 ProbeLM server" 0.1.0 (uvicorn / FastAPI)
routes               /health, /v1/models, /v1/chat/completions
auth                 認証あり（/v1/* は 401、/health は公開）
device               cpu             (旧: cuda:0)
model id             j72-30m
```

同じ VM の 8080 で GrokBot Qwen (llama.cpp) が動いている。

`/health` の実測（CPU 移設後）:

```json
{"ok": true, "model": "j72-30m", "device": "cpu",
 "params": {"total": 150001152, "embedding": 55296000,
            "lm_head": 0, "non_embedding": 94705152}}
```

**モデル名の "30M" はパラメータ数ではない。** 実際は total 150,001,152、
non-embedding 94,705,152 である。名前から能力や役割を推定しないこと
（Kamimusuhi の原則 `K-Edge / K-Core / K-Deep != parameter count` と整合する）。

## context capacity — 推測ではなく設定から

novllm の `config/phase55_probe.json` の `primary`:

```json
{"target_parameters": 150000000, "hidden_size": 768,
 "num_layers": 12, "num_heads": 12, "context_length": 4096}
```

`/health` が返す total params 150,001,152 がこの `primary` 構成と一致するため、
サーバが載せている checkpoint はこれと判断できる（CUDA から CPU へ移しても同一）。したがって

```json
"context_capacity": 4096
```

を宣言する。GrokBot Qwen と偶然同じ値だが、根拠は別（あちらは llama.cpp の `n_ctx`）。

> **未実施:** capacity 境界の実機テスト（capacity 内 → accepted / capacity 超 →
> rejected か silent truncate か）は endpoint 認証待ちで未実行。silent truncate
> するなら宣言値を下げる必要がある。

## capability 宣言

| 項目 | j72 | grokbot | 根拠 |
| --- | --- | --- | --- |
| `locality` | `external` | `external` | 現在は同じ第三者 VM 上。機械の管理者に従う |
| `modalities` | `text` | `text` | |
| `context_capacity` | `4096` | `4096` | checkpoint config / `n_ctx` |
| `latency` | **未測定** | `slow` (11.2s 実測) | 下記。CPU 移設でさらに遅い可能性 |
| `cost` | `free` | `free` | 課金 API を経由しない |
| `quality` | `basic` | `basic` | 150M / 3B Q4。benchmark 未取得 |
| `health` | `healthy` | `healthy` | 静的宣言 |
| `precedence` | `ordinary` (既定) | `last_resort` | 借り物 VM を既定の依存先にしない |

> **未実施:** J72 の latency 実測。**CPU 実行になったため、CUDA 時の想定は使えない。**`LatencyClass::Instant` は使わない
> （別ホスト上の network resource なので意味論上 `fast` が最速候補）。
> 現状の script 既定値は保守的に `slow`。実測後に `J72_LATENCY=fast` で更新する。
> **モデルサイズから決めない。**

## privacy routing matrix

**現在の構成（両方 external）:**

| request privacy | j72 | grokbot | 選択 |
| --- | --- | --- | --- |
| `unconstrained` | eligible | eligible (`NOT_PREFERRED`) | **j72** |
| `no_external_service` | `PRIVACY_EXCLUDED` | `PRIVACY_EXCLUDED` | **拒否** |
| `local_only` | `PRIVACY_EXCLUDED` | `PRIVACY_EXCLUDED` | 拒否 |

**J72 が owned hardware に戻った場合（router の policy として test 済み）:**

| request privacy | j72 (`local_network`) | grokbot (`external`) | 選択 |
| --- | --- | --- | --- |
| `no_external_service` | eligible | `PRIVACY_EXCLUDED` | **j72** |

`local_only` が `LocalHost` までしか許さないのは現行定義どおりで、
**Tailscale 接続だから通す、という特例は作っていない。**

`unconstrained` で j72 が勝つのは precedence による。grokbot が
`last_resort` を宣言しており、j72 は何も宣言していない（既定 `ordinary`）ためで、
品質比較の結果ではない。

現構成では、**行き先を分けているのは locality ではなく precedence** である。
「データ境界で物理的に異なる外付け脳を選ぶ」実証は、J72 が owned hardware に
戻るまで自動テスト内の router policy として保持されているだけで、実機では成立していない。

## routing authority はモデルに渡さない

```text
CurrentInput -> RoutingRequest -> Cognitive Action Router -> resource
```

`RoutingRequest` のフィールドは

```text
task_class / privacy / urgency / required_depth / context_size / cost_budget / modality
```

だけである。候補一覧もモデル名も自由文も含まれない。すなわち
**モデルが「次にどのモデルを呼ぶか」を答えられる欄が構造的に存在しない。**
テスト `nothing_in_a_routing_request_asks_a_model_where_the_thinking_should_happen`
がこれをフィールド集合として固定している。

将来 J72 が novelty / difficulty / ambiguity / salience を
proposal・observation として出す実験は許容されるが、dispatch authority は与えない。

## 自動テスト

`crates/kamimusuhi-runtime/tests/multi_resource_routing.rs`。
実機ではなくローカルの実ソケット fixture server に対して実行するので CI は両ホストに依存しない。

| test | 主張 |
| --- | --- |
| `a_no_external_service_turn_lands_on_the_operators_own_machine` | **Case A (router policy).** j72 を `local_network` と宣言した場合、grokbot `PRIVACY_EXCLUDED` / j72 `SELECTED` |
| `a_no_external_service_turn_has_nowhere_to_go_as_deployed` | **Case A (現構成).** 両方 `PRIVACY_EXCLUDED` で拒否。能力の実損を通過テストとして記録する |
| `a_local_only_turn_excludes_the_operators_own_machine_too` | **Case B.** 両方 `PRIVACY_EXCLUDED` で拒否 |
| `a_turn_larger_than_the_declared_window_is_refused_by_both` | **Case C.** 4097 は両方 `CONTEXT_TOO_LARGE`、4096 は通る |
| `the_same_request_against_the_same_candidates_always_decides_the_same_way` | **Case D.** 17 回・候補順逆転でも同一決定 |
| `urgency_is_answered_by_the_declared_latency_and_nothing_else` | interactive turn は宣言 latency のみで決まる |
| `the_chosen_machine_answers_and_its_answer_is_still_only_material` | 実ソケット経由。j72 が 1 回呼ばれ grokbot は 0 回、出力は `external_material`、head 不変 |
| `a_dead_j72_costs_nothing_canonical_and_is_not_papered_over_by_the_qwen` | 停止/503/遅延/不正 JSON で canonical 不変。**かつ privacy 境界を越えた fallback をしない**（grokbot request count 0） |
| `nothing_in_a_routing_request_asks_a_model_where_the_thinking_should_happen` | routing request のフィールド集合を固定 |

failure isolation で特に重要なのは 2 番目の主張である。
「J72 が落ちたので代わりに外部 VM へ」は最も自然に見えて最も間違った修復であり、
`no_external_service` の turn では発生しない。

## 実機手順

```bash
export KAMIMUSUHI_GROKBOT_API_KEY=...     # 値はシェルにだけ置く

# 1. J72 疎通
curl -H "Authorization: Bearer $KAMIMUSUHI_GROKBOT_API_KEY" \
  http://cursor:8081/v1/models

# 2. 二資源ルーティング（3 つの privacy を順に流す）
AUTH_ENV=KAMIMUSUHI_GROKBOT_API_KEY \
  ./scripts/multi-resource-smoke.sh \
    http://cursor:8081/v1 http://cursor:8080/v1

# 3. 同一入力での比較
./scripts/compare-cognitive-resources.py \
  --auth-env KAMIMUSUHI_GROKBOT_API_KEY --repeat 3 \
  --a j72=http://cursor:8081/v1:j72-30m \
  --b grokbot=http://cursor:8080/v1:qwen2.5-3b-instruct
```

恒久設定例では MagicDNS 名 (`cursor`) を使い、Tailscale IP は書かない。
インフラ固有情報を architecture requirement にしないためである。

## 比較評価 (J72 vs Qwen2.5-3B)

`scripts/compare-cognitive-resources.py` が両 endpoint に**完全に同一の
prompt**を送り、latency・成否・指示追従・出力妥当性・安定性を記録する。

shallow タスク 8 種:
2値/3値 classification / intent / salience / 情報抽出 / provenance label /
短文要約 / 矛盾判定 / 単純 transformation。
能力境界を測るための deep タスク 4 種:
multi-step reasoning / 曖昧な質問 / やや長い要約 / 知識依存質問。

**J72 が deep を落とすこと自体は failure ではない。** 能力境界の測定が目的である。

> **未実施:** 実行は endpoint 認証待ち。結果が出るまで、役割配分について
> 何も結論を書かない。「30M だから K-Edge」「3B だから上位」と先に決めない、
> というのが今回の明示的な制約である。

## limitations / 未解決

- **両 endpoint の認証情報が未取得。** 鍵は rotate 済みで、旧値は 8080/8081 とも 401。
  latency 実測・context 境界テスト・比較評価がこれ待ち。
- **`no_external_service` で使えるモデルが現在ひとつも無い。** 両モデルが同じ
  第三者 VM 上にあるため。J72 を owned hardware へ戻すまで解消しない。
- J72 は CPU 実行になった。CUDA 時を前提とした latency の見込みは使えない。
- `latency` / `quality` は宣言であり、J72 については未測定。
- `health` は静的宣言で、health check による自動降格はない。
- context compressor は未実装。capacity 超過は拒否されるだけで分割されない。
- 比較 harness の採点は部分一致ベースの粗いもの。監査可能性を優先している。
- role 配分（K-Edge / K-Core / K-Deep）の結論は測定後に書く。
