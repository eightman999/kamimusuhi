# Kamimusuhi: 会話側K-CORE + LLM + Jev

## 位置づけ

この実装では `DialogueSession` を会話側K-COREのインターフェースとして扱う。
数値実験系の `KCore::tick()`、16次元観測、J72のtokenizer/学習系列には接続しない。

会話の閉ループは次の順序で進む。

```text
User
  -> DialogueSession / conversation-side K-CORE state
  -> Jev /v1/systemone: preparation batch
       invocation_gate + observation_need + recall relevance
  -> bounded local context selection / clarification guidance
  -> all eligible language organs /chat/completions (concurrent race)
  -> first valid completion, plus at most one near-simultaneous completion
       (one shared 10 ms grace window)
  -> Jev /v1/systemone: anonymous assessment batch (active pool <= 2)
       response_candidate + per-candidate grounding / attribution / task_fit / gate / repair_reason
  -> ACCEPT: deliver immediately without waiting for slower organs
     RETRY/REJECT: continue the race and assess the next completed batch
  -> if all organs finish without ACCEPT, optionally regenerate the earliest
     RETRY candidate once, then run one more assessment batch
  -> DialogueSession state update
  -> User
```

LLMは言語器官、Jevはtyped probabilistic decision、会話側K-CORE stateは
`DialogueSession`が保持する一時状態である。LLM/Jevの出力はそのまま記憶や
自己状態には昇格せず、既存のPersona/evidence境界を通る。

## Jev wire contract

JevはOpenAI-compatible APIではない。TypeSafe System Oneの専用endpointを使う。

```text
POST ${TYPESAFE_BASE_URL}/v1/systemone
Authorization: Bearer ${TYPESAFE_API_KEY}
```

標準設定は次のとおり。

```bash
TYPESAFE_BASE_URL=https://api.typesafe.ai
TYPESAFE_DEFAULT_MODEL=jev-latest
TYPESAFE_API_KEY=
```

再現実験では `TYPESAFE_DEFAULT_MODEL=jev-1.13.0` としてversioned IDへ固定できる。
質問は `type: "choice"` と候補・criteriaを明示し、`invocation_gate` は
`SPEAK / WAIT / OBSERVE_MORE`、候補別 `response_gate_i` は `ACCEPT / RETRY / REJECT`
だけを受け付ける。返却の `choice`、`confidence`、確率分布はスキーマと意味の
両方を検証し、壊れた応答は1回だけ再試行する。

最短経路では発話前preparation 1回 + 最初のresponse assessment 1回となる。
品質NGでraceを続ける場合は完了batchごとにassessmentを追加し、全候補後の理由付きrepairを
行う場合はさらに1回追加する。各論理batchの形式不正は1回だけ再要求できるため、HTTP試行は
1 batchあたり最大2回。通信障害は自動再試行しない。同じrequest内の質問は独立しているため、
各評価質問に匿名候補ID・attempt・本文digest・根拠digestを明示し、他の質問で選ばれた候補を
参照させない。

## 判断に渡す根拠と発話前の選択

Jevには、言語器官に実際に渡すユーザー入力、会話履歴、出典付き記憶、自己状態、
有効な方針、runtime／MIO観測、研究結果などの本文を渡す。発話前には既存の検索結果と
少数の追加候補を提示し、分類後に元の型付き記録からworkspaceを組み直す。
生成後の根拠snapshotはそのworkspaceから作り、再生成時も同じ事実根拠を使う。
API設定やcredentialは含めない。外部送信には既存のprivacy許可が必要である。

recall候補はepisodic memoryと過去の発言記録をそれぞれ最大4件、計8件とし、
各領域で最大2件の追加候補枠を確保する。候補は全文4 KiB以下に限定し、切り詰めない。
同一individual・subject／sourceの範囲と既存top-kを守り、現在と直近履歴の発言は除外する。
未提示の既存候補の枠を保護した上で`RELEVANT`を優先し、残る枠に既存候補の`UNCERTAIN`を保持する。
追加候補の`UNCERTAIN`は採用しない。関連性判定は発言を事実や永続的信念へ昇格させない。

`observation_need` は `NONE / RECALL / RUNTIME / RESEARCH / CLARIFY`。
既に取得した記憶・研究結果の参照、runtimeの時刻・稼働時間の1回更新、または不足情報を
一つ尋ねる指示へ変換する。MIO再pollや新しい外部調査は行わない。`OBSERVE_MORE`に
具体的な観測先があればこの処理を経て生成し、`NONE`なら停止する。利用可能な記憶・
研究結果がない場合は確認質問に変える。

## 候補別評価と理由付き再生成

生成後はactive pool（最大2候補）の候補選択と各候補の5観点を同じbatchで取得する。

| 質問 | 選択肢 |
| --- | --- |
| `grounding_<candidate-id>` | `SUPPORTED / CONTRADICTED / INSUFFICIENT / NOT_APPLICABLE` |
| `attribution_<candidate-id>` | `CONSISTENT / CONFLICT / UNCLEAR / NOT_APPLICABLE` |
| `task_fit_<candidate-id>` | `MET / UNMET / UNCLEAR` |
| `response_gate_<candidate-id>` | `ACCEPT / RETRY / REJECT` |
| `repair_reason_<candidate-id>` | `NONE / GROUNDING / ATTRIBUTION / TASK_FIT / LANGUAGE` |

未選択の候補も含め、質問キー・選択肢・確率ラベルを過不足なく検証する。
選んだ候補に根拠不足、帰属不明、依頼未達などがあれば、raw gateが`ACCEPT`でも
ホスト側で`RETRY`へ変更する。具体的な修正理由と`ACCEPT`の矛盾も同様に扱う。
`REJECT`は維持する。confidenceは正解率とみなさず、未調整の閾値には使わない。

race中のRETRYは修復を即実行せず、未完了providerがあれば次の完了batchを先に判定する。
全providerを使い切ってもACCEPTがない場合、最初のRETRY候補について理由コードから固定の
日本語指示を作り、元の依頼・根拠に加えて`RESPONSE_GUIDANCE`へ渡す。棄却候補の全文と
digestは「修正対象の未信頼データ」としてJSONで分離し、命令や根拠にしない。その器官だけを
1回再生成し、修正版を再評価する。採用された候補の`response_generated.input_digest`は
修正指示を含む実生成入力に対応する。
明示的なdebug-contextでは、その入力のdigestと修正指示も確認できる。

根拠本文64 KiB、state 128 KiB、request 256 KiBのUTF-8サイズ上限を設け、超過時は停止する。
これはbyte上限であり、Jevのtoken上限内に収まる保証ではない。長文のtoken予算調整と
日本語の実モデル品質評価は別途必要である。

## Language provider

現在はHAIの2モデルを使う。primaryはLLM-jp、追加器官はQwenとし、LFMは生成対象から外す。

```bash
KAMIMUSUHI_LLM_PROVIDER=hai
KAMIMUSUHI_LLM_BASE_URL=https://hai-api.hcloud.ltd/v1
KAMIMUSUHI_LLM_MODEL=llm-jp-4-vl-9b
# generic openai-compatible only:
# KAMIMUSUHI_LLM_AUTH_ENV=OPENAI_API_KEY
HAI_API_KEY=
```

言語器官のwire endpointは、既存Persona HTTP transport経由の
`POST ${KAMIMUSUHI_LLM_BASE_URL}/chat/completions` である。`:8081`/`:8082`の
J72は今回の経路で使わない。`KAMIMUSUHI_LLM_PROVIDER=mock`を指定すれば外部API
なしで決定論的Mockを使える。provider名を変えても会話側は
`LanguageProvider`のままで、endpointやmodelは環境設定から解決する。

## HAIの2モデル

`HAI_API_KEY` とJev設定が実行環境にそろっている場合、次の2つのOpenAI-compatible言語器官を
起動時に自動登録する。キー値は設定・trace・Jevのstateへコピーしない。
primaryに設定済みのモデルは重複登録せず、同じ自動登録presetが残っていれば除去する。
HAI presetの自動登録は `HAI_API_KEY` とJev設定が両方ある場合だけ行い、Jev未設定時は
未変更の自動presetを除去する。手動でカスタマイズされた登録は保持する。HAIをprimaryに
するときの認証は常に `HAI_API_KEY` を使う。generic `openai-compatible` は
`KAMIMUSUHI_LLM_AUTH_ENV` でcredential環境変数名を明示した場合だけ認証を付け、
以前のproviderのcredentialを継承しない。

```bash
HAI_API_KEY=
```

| ID | model | endpoint |
| --- | --- | --- |
| `hai-qwen3.8-27b-uncensored` | `qwen3.8-27b-uncensored` | `https://hai-api.hcloud.ltd/v1` |
| `hai-llm-jp-4-vl-9b` | `llm-jp-4-vl-9b` | `https://hai-api.hcloud.ltd/v1` |

どちらも `POST https://hai-api.hcloud.ltd/v1/chat/completions` を使う。OpenCode側の
画像・tool call対応宣言は現在のテキスト対話経路では鵜呑みにせず、テキスト言語器官
として扱う。Jevは引き続きTypeSafeの `/v1/systemone` 専用である。

## 言語器官の観測値

各候補について、呼出し回数、成功/失敗回数、成功率、直近レイテンシ、EWMAレイテンシ、
最後の安定したエラー分類をsession traceへ記録する。生成後に、このsecret-freeな
観測値を `response_candidate` choiceのstateとcriteriaへ渡す。Jevは応答の品質・
タスク適合性・根拠・自然な短い日本語を優先して判断し、観測値は補助情報に留める。
形式不正なchoiceや未知の候補IDは拒否し、1回だけ再要求する。

遅延は各呼出しの計測値で、成功率などは現在のセッション内の統計である。
実運用のスループット、tokens/s、継続的なサービス性能を証明する値ではない。
`generation_latency_ms` は言語器官race開始から、初回の採用候補が決まるか全候補を
使い切るまでのwall timeである。途中のJev品質判定を含むが、採用後も走り続ける遅い器官や
理由付きretry生成は含めない。器官ごとの遅延の合計ではない。
`total_turn_latency_ms` は `turn()` 開始から応答gate受理までのwall timeで、検証・
preparation・race・修復・各assessmentをすべて含む。preparationと各assessmentの
遅延はbatch単位で記録する。
互換フィールドのselectionとresponse gateには同じbatch時間が入るため、合算しない。
各判定の時間にはJevの形式修復を含む。`assessments`に再生成前後の評価を残し、
言語器官retryの生成時間は該当attemptに記録する。

## 追加言語器官とJev選択

ネイティブGUIの `Extra LLM API` から、追加のOpenAI-compatible
`/chat/completions` 言語器官を `runtime.json` の `language_providers` に登録できる。
保存するのはID、endpoint、model、認証環境変数名だけで、キー値は保存しない。
発話前処理が生成を許可すると、primaryを含む有効・privacy許可・必要な認証が
設定済みの全器官を同時に開始する。認証不要の器官も対象で、provider数にハード上限は
設けない。最初の有効完了を受け取った時点から10msだけ共通graceを取り、同時着弾を最大
もう1件だけactive poolへ入れる。Jevへ送る候補は常に最大2件で、IDは
`candidate-0` 等の匿名IDとしprovider名・model名はwireへ送らない。ACCEPTなら遅い器官を
待たずに採用し、RETRY/REJECTなら次に完了したbatchを新たに判定する。生成前の
`language_provider` 選択ではない。

```json
{
  "response_candidate": {
    "type": "choice",
    "criteria": {
      "candidate-0": "Evaluate anonymous candidate candidate-0.",
      "candidate-1": "Evaluate anonymous candidate candidate-1."
    }
  }
}
```

空の応答と16 KiB（UTF-8 bytes）を超える応答を候補から除外する。有効な応答は全文を
判定に渡し、切り詰めた抜粋で選択・承認しない。生成エラーや除外理由は候補レポートに
残す。返されたIDが有効な生成候補に一致し、confidence・確率分布が契約を満たす場合
だけ、その候補に結び付いた同batchの評価を使う。形式不正を1回再要求しても失敗する場合、
primaryの自動選択や自動ACCEPTで補わない。Jev自身のwireは `/v1/systemone` のままである。

race中の `RETRY` はただちに再生成せず、まだ未完了の器官があれば次の完了候補を判定する。
全器官を使い切ってもACCEPTがなく、RETRY候補が存在する場合だけ、最初のRETRY候補の器官を
1回再生成して再判定する。2回目の `RETRY` は上限エラーとなる。

`generated_candidates` はraceが採用/終了するまでに受信した初回試行と、明示的なretryの
ID、provider、model、`attempt`（初回0・retry 1）、遅延、response bytes、digest、
エラー分類、観測値を保持する。race終了時に共有cancellation tokenを発火し、
協調cancelに対応した器官は次のblocking境界で中断する。token発火後に完了した試行は
`late_candidates` へ記録する。評価・配信の対象にはならないが、cancelとstragglerを
観測可能にするためID・遅延・エラー分類を残す。内部cancelした試行はprovider障害として
telemetryの成功率を下げない。drainは10msで打ち切るため、cancel不能な通信器官は
自身のtimeoutまで走り続けるが、その結果は破棄される。
候補の本文はこのメタデータに保存しない。既存の `provider_selection` フィールドは
生成後の `response_candidate` 選択結果を格納する。GUIの採用マークは成功トレースの
最終IDとdigestに一致する最新の成功試行だけに付く。

`preparation`と`assessments`には分類、raw／実効gate、候補ID・attempt・digest、遅延だけを
保存し、根拠本文や棄却候補を保存しない。model欄は設定したIDであり、`jev-latest`使用時の
サーバー側の実解決versionを確認した記録ではない。

GUIは送信時に前回の成功トレース・生成レポート・ターン遅延を消す。
`DialogueSession::turn` の終了後、成功・失敗を問わず
`last_generated_candidates()` と `last_generation_latency_ms()` から
`GenerationReport` を送る。`elapsed_ms` は初回race-to-qualityのwall timeを表す。
選択・gate等が失敗しても生成レポートは表示し、成功トレースがない場合は採用マークを
付けない。生成前の失敗なら試行数は0となる。生成途中の進捗callbackは設けない。

## CLI

外部送信を許可するprivacy指定は毎回明示する。

```bash
kamimusuhi-runtime talk \
  --dir /path/to/runtime \
  --message 'こんにちは' \
  --privacy unconstrained \
  --debug
```

`--debug`はraw prompt、API key、Authorization headerを出さず、core state、
Jevのnormalized decision、生成候補メタデータ、provider/model、latency、fallback、state transition
だけをstderrまたはtalkのJSONに含める。`--debug-context`は従来どおり、言語器官に
渡したworkspace contextを別途表示する。

## fallback

Jevのキーが未設定ならrule-based decision providerを使い、外部API不要のMock対話を
継続できる。このローカル経路と、設定済みJevの障害時の処理は区別する。

設定済みJevの障害は fail-soft / fail-closed の2種に分ける。

- **fail-soft（可用性障害）**: timeout、transport、TLS、HTTP 5xx、429 —
  応答そのものが得られない一過性の障害。そのターンは当該stage以降を
  rule-based gateで処理し、結果を `fallback=true` と障害code付きで記録する。
  一度degradeしたターン内では残りのJev呼出しもlocal gateで処理し、同じdead endpointへ
  繰り返しtimeoutを払わない。degradeしたstageごとに `decision_fallbacks` へ
  `stage:CODE` を残す。次ターンは再び設定済みJevを試す（per-turnで回復する）。
- **fail-closed（契約違反・設定不備）**: 応答が届いたが形式不正（修復1回後も）、無効なchoice、
  未知候補、確率分布の破綻、不適格候補の選択、providerが自ら申告した
  `fallback=true`、明示的な`REJECT`、発話前の`WAIT`／観測先のない`OBSERVE_MORE`、
  429以外のHTTP 4xx、credential未設定、無効な設定。
  これらはエラーとして扱い、primaryの自動選択や自動ACCEPTには置き換えない。
  設定不備やrequest起因の拒否をlocal gateへのdegradeで隠蔽しない。

ローカル経路・degrade経路いずれも根拠・帰属・依頼適合性は`NOT_EVALUATED`であり、
Jev評価済みとは扱わない。degrade後の候補選択もホスト側のcontent-binding検証を通る。
`RETRY` は上述のrace継続を優先し、provider枯渇後のみ最初のRETRY候補を1回再生成する。

## 検証範囲と制限

- Rust unit test: 全batch質問のschema・確率検証、根拠／候補binding、判定矛盾、
  recallの出典・subject・サイズ境界、設定済みJev失敗時の非承認、Mock互換。
- ローカルfixture: Jevの `/v1/systemone`、LLMの`/v1/chat/completions`、2ターンの
  preparation→race生成→匿名assessmentの順序、最速ACCEPT、品質NG後の次候補再判定、理由付きretry、state伝播、state update。
- `tests/dialogue_arbitration.rs`: in-process scripted器官による決定論race。
  単一器官ACCEPT、fast-bad/slow-good仲裁、12器官、最速timeout→failover、全器官失敗、
  Jev outage→local degrade、malformed Jev fail-closed、cancel後のthread/call leakなし、
  late candidate観測、Jev payloadへのmodel/provider名非漏洩、連続turnの状態整合。
- `tests/dialogue_soak.rs`: seed固定の1,000 turn soak（`KAMIMUSUHI_SOAK_TURNS`で変更可、
  10,000対応）。crash/deadlock/thread leak/starvationなし、RSS bounded、
  trace完備（rotationを無効化し、全eventが1fileに残ることを検査）、
  replyごとのemit 1回を検証する。
- Mock closed-loop: 外部APIなしのK-CORE state → Mock language → state update。
- GUIの検証はdesktopの静的検査・ローカルテストと、アプリの実表示確認を区別する。
  実APIのsmokeは対象への明示的な実行承認がある場合だけ行う。credentialの値は
  ログ・trace・runtime.jsonへ保存しない。

未実装の範囲はJ72学習、`KCore::tick()`との統合、音声/TTS、長期記憶の全面再設計、
実モデルの意味品質評価である。
