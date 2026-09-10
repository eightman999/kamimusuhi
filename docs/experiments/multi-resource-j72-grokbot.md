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

`/v1/chat/completions` の実測（`max_tokens=32`）:

```text
HTTP 200            OpenAI 互換 schema
finish_reason       "stop"
usage               prompt/completion/total すべて -1（未計測）
timings.total_s     あり（独自フィールド）
wall                2.65s
```

`usage` が使えないので、token 会計は Kamimusuhi 側から取得できない。

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

境界の実機テストは実施済み。結果は「**J72 は拒否しない**」であり、詳細は
[実測: context 境界](#実測-context-境界--両者で正反対)に記す。
宣言値 4096 は据え置く。サーバが受理してしまう以上、下げても上げても
守るのは router だけであり、モデルが訓練された窓に合わせるのが正しい。

## capability 宣言（実測後）

| 項目 | j72 | grokbot | 根拠 |
| --- | --- | --- | --- |
| `locality` | `external` | `external` | 同じ第三者 VM 上 |
| `modalities` | `text` | `text` | |
| `context_capacity` | `4096` | `4096` | checkpoint config / `n_ctx`（実測、後述） |
| `latency` | `slow` | `slow` | 実測。Kamimusuhi 経路で 6.0–11.2s |
| `cost` | `free` | `free` | 課金 API を経由しない |
| `quality` | `basic` | `basic` | 語彙上の最下位。J72 は下記のとおりこれでも過大 |
| `health` | `healthy` | `healthy` | 両者とも応答する |
| `precedence` | **`last_resort`** | `ordinary` | 実測。サイズではない |

`precedence` が入れ替わった点が今回最大の修正である。単一資源実験では
grokbot が「唯一の借り物マシン」だったので `last_resort` だった。現在は
**両方が同じ借り物 VM 上**にあり、precedence はもはやその懸念を表現できない。
両者を互いに順序づけるだけの軸になり、実測は Qwen を先に取れと言っている。

J72 を `last_resort` にした根拠は測定であって 150M というサイズではない
（0/36、Qwen の約 8 倍の median latency）。health や cost に嘘を書いて
同じ順序を作ることはしていない。その軸の読みが全て壊れるからである。

## 実測: latency

`scripts/compare-cognitive-resources.py`、12 タスク × 3 回 = 各 36 サンプル、
`max_tokens=64`、`temperature=0`。

| | min | median | p95 | max |
| --- | --- | --- | --- | --- |
| j72 (150M, CPU) | 5.02s | **6.82s** | 10.09s | 15.60s |
| grokbot (3B Q4, llama.cpp) | 0.554s | **0.86s** | 2.83s | 3.51s |

**150M の CPU モデルが、3B の量子化モデルより約 8 倍遅い。**
`LatencyClass::Instant` は使用していない（別ホスト上の network resource のため）。

ただし Kamimusuhi の実経路ではどちらも `slow` である:

```text
Kamimusuhi turn (grokbot)   6054 ms / 11229 ms   生成長無制限
bounded 64 tokens (grokbot)  860 ms (median)
```

差の原因は **adapter が `max_tokens` を送っていない**ことである
(`crates/kamimusuhi-resource-http/src/openai.rs`)。生成が無制限なので、
shallow な分類タスクでも長文が返る。宣言は実経路に合わせて `slow` とした。
出力長を束縛できるようにすることは未解決事項として残す。

## 実測: context 境界 — 両者で正反対

同一の過大入力（`あ` × N、`max_tokens=8`）を両 endpoint に送った。

| chars | j72 (custom PyTorch) | grokbot (llama.cpp) |
| --- | --- | --- |
| 1,000 | HTTP 200 / 1.50s | — |
| 8,000 | **HTTP 200 / 4.41s** | **HTTP 400** `exceed_context_size_error` (8029 tokens > n_ctx 4096) |
| 40,000 | **HTTP 200 / 18.83s** | **HTTP 400** (40029 tokens > n_ctx 4096) |

llama.cpp は明示的に拒否し、`n_ctx: 4096` を実測で裏づけた。

**J72 は一度も拒否しない。** 4096 token を大きく超える入力を 200 で受理し、
latency は入力長に比例して伸びる（切り捨てて即座に捨てているわけではない）。
外部からは truncate か劣化生成かを区別できないが、Kamimusuhi にとっての事実は
ひとつである:

```text
J72 について、4096 の宣言だけが唯一の防御である。
サーバ側には境界が無い。
```

これは「declared, not discovered」が冗長な二重化ではなく、
実際に効いている資源があるという実例である。

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

実機での確認（`scripts/multi-resource-smoke.sh`、両 endpoint とも live）:

```text
1. no-external-service   refused                        両方 PRIVACY_EXCLUDED
2. local-only            refused                        両方 PRIVACY_EXCLUDED
3. unconstrained         selected: grokbot
                         considered: grokbot SELECTED / j72 NOT_PREFERRED
```

`local_only` が `LocalHost` までしか許さないのは現行定義どおりで、
**Tailscale 接続だから通す、という特例は作っていない。**

`unconstrained` で j72 が勝つのは precedence による。grokbot が
`last_resort` を宣言しており、j72 は何も宣言していない（既定 `ordinary`）ためで、
品質比較の結果ではない。

現構成では、**行き先を分けているのは locality ではなく precedence** である。
そしてその precedence は実測に基づいている。
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

## 実測: J72 vs Qwen2.5-3B 同一入力比較

同一 prompt・同一パラメータ。shallow 8 種 + 能力境界用 deep 4 種、各 3 回。

| | shallow | deep | call failures |
| --- | --- | --- | --- |
| j72 | **0/24** | **0/12** | 0 |
| grokbot | 18/24 | 9/12 | 0 |

### J72: instruct モデルではない

J72 は指示に一切従わない。`temperature=0` で全 12 タスクに対し

```text
、その、その、その、その、その、その、その…
```

という退化した反復を返した（`stable=True`、つまり再現性はある）。
別の短い呼び出しでは青空文庫由来と思われる日本語の小説的テキストを生成した。

これは故障ではない。`novllm` = **novel LM**、日本語小説コーパスの
base language model であり、instruction tuning を受けていない。
classification / extraction / summarization といった現行の `TaskClass` 語彙は、
このモデルが訓練された作業ではない。

したがって、`quality: basic` は語彙上の最下位でありながら **なお過大**である。
現行の routing 語彙には「これは continuation model であって instruct model ではない」
を表現する軸が無い。今回は precedence で最下位に置くことで実害を避けているが、
本来は task class 側の問題である（未解決事項）。

### Qwen: shallow は強く、厳密操作は弱い

| task | 結果 |
| --- | --- |
| classify-sentiment / intent / salience / provenance | 3/3 |
| extract-date | 3/3 (`2026-03-14`) |
| summarize / longer-summary / ambiguous / knowledge | 3/3 |
| **contradiction** | 0/3 — 18時閉店と20時営業を「矛盾しない」と回答。不安定 |
| **transform-case** | 0/3 — `kamimusuhi` → `KAMIMUSHI`（音節が落ちている） |
| **multi-step** | 0/3 — 10 のところ 14 |

分類・抽出・要約は実用域、算術と厳密な文字列変換は不可。
**大文字化のような自明な変換で入力を壊す**点は、出力を検証せず取り込む設計が
危険であることの具体例になっている。

## 役割の考察 — 仮説は反転した

事前の仮説は次のものだった。

```text
J72-30M     small / cheap / potentially interactive / shallow cognition 候補
Qwen2.5-3B  larger / stronger / measured slow / background cognition 候補
```

**実測はこれを反転させた。** J72 は遅く（8x）、現行のどの task class も遂行できない。
Qwen は速く、shallow タスクの大半を正しく処理する。

先に決めずに測ったことがそのまま結論になっている:

```text
K-Edge / K-Core / K-Deep != parameter count

role is determined by
latency / capability / locality / cost / task requirements
```

150M という数字は、CPU 実行と instruction tuning の不在の前では
何の役割も保証しなかった。現時点で J72 に割り当てられる cognitive role は無い。
将来 continuation / 文体生成のような、このモデルが実際に訓練された task class が
語彙に入れば話は変わる。

## 実機での canonical state 不変性と failure isolation

`unconstrained` の実 turn（grokbot が応答）:

```text
resource.slot             grokbot
resource.implementation   openai-compatible
attempts                  1
latency_ms                6054                実クロック
routing_decision          grokbot SELECTED / j72 NOT_PREFERRED
head_before == head_after true
individual_id             fixture phase と同一
relationship              {"preference": "ほうじ茶"}   DB から復元
workspace item            EXTERNAL_RESOURCE_RESULT / external_material
```

続いて J72 を死んだポートへ向け、precedence を `preferred` にして
**必ず選ばれてから失敗する**状況を作った:

```text
stderr   [TRANSPORT] resource ...0472 transport failure after 1 attempt(s):
         connect: Connection refused
exit     1

inspect  individual_id / head.commit_id / relationship.active
         いずれも実行前と完全一致            CANONICAL STATE UNCHANGED

resource_calls  ...0472 | TRANSPORT        失敗は 1 行記録される
```

grokbot は live のままだったが**呼ばれていない**。
失敗した資源の代わりに別の資源へ回す挙動は無い。

## limitations / 未解決

- **J72 には現在割り当てられる cognitive role が無い。** instruct モデルではなく、
  現行の `TaskClass`（summarize / generate / classify）のいずれも遂行できない。
  routing 語彙に「continuation model」を表す軸が無いため、precedence で
  最下位に置いて実害を避けているだけである。
- **adapter が `max_tokens` を送らない。** そのため shallow なタスクでも生成が
  無制限になり、bounded 0.86s の endpoint が実経路では 6s になる。
  出力長の束縛は未実装で、`latency` 宣言はこの制約込みの値になっている。
- **J72 サーバは context 超過を拒否しない。** 宣言 4096 だけが防御である。
- **J72 の `usage` が全て -1。** token 会計を Kamimusuhi 側から取れない。
- **`no_external_service` で使えるモデルが無い。** 両モデルが同じ第三者 VM 上に
  あるため。J72 を owned hardware へ戻すまで解消しない。
- `quality` の語彙が粗い。`basic` が最下位だが、J72 にはそれでも過大である。
- `health` は静的宣言で、health check による自動降格はない。
- context compressor は未実装。capacity 超過は router が拒否するだけで分割されない。
- 比較 harness の採点は部分一致ベースの粗いもの。監査可能性を優先している。
- Qwen は自明な文字列変換を壊す（`kamimusuhi` → `KAMIMUSHI`）。出力を検証せずに
  取り込む設計が危険であることの具体例。
