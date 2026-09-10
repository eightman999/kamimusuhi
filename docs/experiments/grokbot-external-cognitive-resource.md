# 実験: Grok Bot VM の 4B モデルを external cognitive resource として借りる

実装済みの内容のみを記述する。設計仮説と実測を混同しない。

## 実験目的

Kamimusuhi の設計命題

```text
identity != model
identity != machine
identity != inference provider
```

を、**operator が所有していない計算機** で実機確認する。

確認したいのは「無料の CPU を使えるか」ではない。外部の計算能力を一時的に借りても、
continuity lineage と canonical self を移動させる必要がないこと、
そして借り先が消えても individual が継続することである。

借りた 4B モデルの位置づけは **temporary external cortex** であり、交換可能な認知資源にすぎない。

## 対象環境

```text
Grok Bot VM
  OS        Debian 13 (trixie)
  CPU       8-core Intel Xeon / KVM
  RAM       15 GiB
  Network   Tailscale
  Runtime   llama.cpp (OpenAI-compatible server)
  Model     Qwen3-4B-Instruct Q4_K_M
  Context   4096 tokens
```

Kamimusuhi 側は既存の `kamimusuhi-resource-http` の `OpenAiCompatibleResource` をそのまま使う。
この実験のために新しい HTTP client も新しい provider 実装も追加していない。
追加したのは runtime config 上の宣言と、`--urgency` フラグ 1 つだけである。

## Persona Core ではなく Cognitive Resource とする理由

```text
Persona Core
    │  continuity / self / relationship / expression
    │
    └── Cognitive Action Router
             ├── local resources
             ├── frontier resources
             └── grokbot-qwen3-4b   ← 今回
                    │ Tailscale
                    │ llama.cpp
```

Persona Core は individual として喋る主体であり、Kamimusuhi の runtime config では
`persona` という **resources とは別の名前空間** に置かれる。router の候補には決してならない。

Grok Bot の 4B モデルはその逆側、すなわち turn が subtask を委譲する先である。
想定する task class は summarization / classification / short generation /
information extraction / lightweight reasoning / background cognition のような補助認知であり、
identity、canonical state、最終的な人格表現の所有者にはしない。

理由は「4B では品質が足りないから」ではない。品質が十分でも同じ結論になる。
individual の所有者を他人の VM に置くと、その VM の停止・削除・モデル交換が
individual の存否と結びついてしまい、証明したい命題そのものが崩れる。

## capability 宣言

`ResourceCapabilities` は **operator が宣言するもの** であり、測定でも provider の自己申告でもない
（`crates/kamimusuhi-core/src/routing.rs` の "Declared, not discovered"）。
今回の宣言は保守的に次のとおり。

| 項目 | 値 | 根拠 |
| --- | --- | --- |
| `locality` | `external` | VM は operator の管理下にない（後述） |
| `modalities` | `text` | text のみ |
| `context_capacity` | `4096` | llama.cpp を 4K context で起動しているため。provider 既定値 8192 は使わない |
| `latency` | `slow` | 実測していないので `fast` と仮定しない |
| `cost` | `free` | 追加課金のある API を経由しないという事実 |
| `quality` | `basic` | 4B Q4_K_M。benchmark を取っていないので上げない |
| `health` | `healthy` | 起動している前提の初期値 |

`cost: free` は「無料だから優先」を意味しない。router の hard constraint は
privacy → modality → health → context → cost → latency → quality の順で評価され、
cost は eligible な候補が複数ある場合の順位付けに使われるだけである。

`latency: slow` は装飾ではなく効いている。`Urgency::Interactive` の turn は
`slow` な資源を `TOO_SLOW_FOR_URGENCY` で除外するので、人が待っている turn は
実測が済むまでこの VM に届かない。届かせるには `--urgency background` を明示する必要がある。

## privacy boundary

```text
Tailscale connectivity does not imply Kamimusuhi LocalNetwork locality.

Locality describes trust/operator control of the compute resource,
not merely the network transport used to reach it.
```

すなわち

```text
private transport      != trusted/local compute
Tailscale              != operator-controlled infrastructure
```

Tailscale は「経路が公開インターネットを通らない」ことを与えるが、
「その計算機を誰が管理しているか」は変えない。
`LocalityClass::LocalNetwork` は *operator が管理する機械* を指すので、Grok Bot VM は該当しない。

結果として、既存の router の privacy filtering がそのまま境界になる。

| request の privacy | 判定 | 理由コード |
| --- | --- | --- |
| `local_only` | 除外 | `PRIVACY_EXCLUDED` |
| `no_external_service` | 除外 | `PRIVACY_EXCLUDED` |
| `unconstrained` | 候補に入る | `SELECTED`（他条件を満たす場合） |

Grok Bot 専用の例外処理も bypass も追加していない。除外は router の一般規則の結果である。

## 4K の扱い

`context_capacity: 4096` を超える request は `CONTEXT_TOO_LARGE` で除外され、
**自動 truncate はしない**。入りきらない部分を黙って落とすことは、
誤った答えを自信満々に生成する典型的な経路だからである。

今回は新しい context compressor を実装していない。
大きな workspace を 4K 以下の task へ切り出す処理は将来の作業であり、
現時点で保証しているのは「router が capacity を守る」ところまでである。

```text
large workspace
      ↓ task extraction / context slicing   ← 未実装
      ↓ <= 4096
   grokbot 4B
      ↓ typed ResourceResult
   Kamimusuhi integration                    ← 実装済み
```

## endpoint failure

Grok Bot VM は常時稼働が保証されたインフラではない。
借りた認知が失敗しても、それは失敗した借用であって事実ではない。

```text
resource failure != canonical state corruption
```

connection refused / timeout / 5xx / malformed JSON / model unavailable のいずれでも、
individual、continuity head、relationship memory、Library は変化しない。
失敗した call は `resource_calls` に error_code つきで 1 行記録される
（試行が「あった」ことは見えるべきなので、記録しないのではなく失敗として記録する）。

## secret の扱い

`runtime.json` に保存するのは環境変数の **名前** だけである。

```json
"auth_env": "KAMIMUSUHI_GROKBOT_API_KEY"
```

値は call 時に環境から読まれ、request とともに破棄される。
config・trace・SQLite のいずれにも token は書かれない
（`crates/kamimusuhi-runtime/tests/grokbot_external_resource.rs` で実際に検証している）。
なお素の llama.cpp server は認証を要求しないので、通常は `auth_env` 自体が不要である。

## 再現手順

```bash
# 1. VM 側の疎通確認（model 名はここで確認する。推測しない）
curl http://<GROKBOT_TAILSCALE_IP>:8080/v1/models

curl http://<GROKBOT_TAILSCALE_IP>:8080/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"qwen3-4b-instruct",
       "messages":[{"role":"user","content":"Reply with exactly: KAMIMUSUHI_GROKBOT_OK"}],
       "max_tokens":32}'

# 2. Kamimusuhi adapter 経由
./scripts/grokbot-resource-smoke.sh http://<GROKBOT_TAILSCALE_IP>:8080/v1 qwen3-4b-instruct
```

endpoint は引数であり、コードにも script にも埋め込まれていない。

smoke script は runtime を fixture で初期化してから `grokbot-qwen3-4b` slot **だけ** を
resource として登録する。general slot を残さないのは、VM に届いた turn が
「条件を満たしたから」届いたのであって「他に候補がなかったから」ではないことを明確にするためである。
続けて `--privacy no-external-service` の turn を実行し、拒否されることを確認する。

## 自動テスト

`crates/kamimusuhi-runtime/tests/grokbot_external_resource.rs`。
実 VM ではなくローカルの実ソケット fixture server に対して実行するので、CI は VM に依存しない。

| test | 主張 |
| --- | --- |
| `the_endpoints_four_thousand_token_window_is_what_the_config_declares` | config から組み立てた descriptor が 4096 / external / slow / free / basic であること |
| `the_router_holds_the_four_thousand_token_line` | 4096 は通り、4097 と 32000 は `CONTEXT_TOO_LARGE` |
| `a_private_transport_does_not_make_someone_elses_vm_local` | `local_only` / `no_external_service` は `PRIVACY_EXCLUDED`、`unconstrained` のみ候補 |
| `a_resource_declared_slow_is_unreachable_from_an_interactive_turn` | interactive turn は `TOO_SLOW_FOR_URGENCY` |
| `a_background_turn_reaches_the_endpoint_and_the_answer_is_only_material` | 応答は `EXTERNAL_RESOURCE_RESULT` / `external_material` にとどまり、head は動かず、token は config・trace・DB に残らない |
| `constrained_material_never_reaches_the_vm_at_all` | 制約付き turn では fixture server の request count が 0 |
| `a_vm_that_is_simply_off_costs_nothing_canonical` | 停止・503・応答遅延・非 JSON のいずれでも canonical state は不変、失敗は 1 行記録 |
| `removing_the_vm_leaves_the_individual_where_it_was` | slot を削除しても individual・head・relationship memory は同一 |

## 実測記録 (2026-09-10)

Grok Bot VM そのものは、この作業を行った machine の tailnet から到達できなかった
（該当ホスト名がなく、4096 ctx の Qwen3-4B-Instruct を出している endpoint も見つからなかった）。
そのため adapter 経路の実機確認は、同じ tailnet 上の別の llama.cpp host
(`llm-machine`, `qwen3-4b`) を代役として実施した。**Grok Bot VM 自体での実測ではない。**

代役 endpoint は ctx 32768 なので、`context_capacity: 4096` の宣言は
その endpoint に対しては過小申告である。過小申告は安全側なので実験は成立するが、
Grok Bot VM に対して実行するときの 4096 は実際の起動値と一致していなければならない。

確認できたこと:

```text
resource.slot            grokbot-qwen3-4b
resource.implementation  openai-compatible      既存 adapter を使用
routing_request.urgency  background
routing_decision.reason  SELECTED
attempts                 1
latency_ms               2076                   実クロックで測定
head_before == head_after                       true
individual_id                                   fixture phase と同一
```

同じ runtime に対する `--privacy no-external-service` の turn は拒否され、
trace に `"outcome":"refused"` と `PRIVACY_EXCLUDED` が記録された。
`runtime.json` と `trace.jsonl` に `Bearer` は 1 件も現れない。

モデルの応答内容は「evidence ID について何をしたいのか」を尋ね返すもので、
task として有用ではなかった。これは prompt shaping の問題であり、
今回の境界（capacity / privacy / failure / identity 不変性）とは独立である。
4B モデルに実際に有用な補助認知をさせるための task shaping は未着手である。

## limitations

- `latency` / `quality` は宣言であり測定ではない。benchmark は取っていない。
- automatic capability benchmark は実装していない。値の更新は operator の設定編集である。
- context compressor は未実装。4K を超える task はルーティング時点で拒否されるだけで、分割はされない。
- health は静的宣言であり、health check による自動降格はない。VM が落ちていても宣言は `healthy` のままで、失敗は call 時に検出される。
- 実 VM に対する疎通は smoke script で手動確認する範囲であり、CI には含まれない。
- Grok Bot 固有の情報を architecture の中心概念にはしていない。この文書は 1 つの実験記録である。
