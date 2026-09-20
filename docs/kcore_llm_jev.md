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

通常は発話前と生成後の2回、言語器官のRETRY時は3回の論理呼出しになる。
各batchの形式修復を含めたHTTP試行は最大4回／6回。通信障害は自動再試行しない。
同じrequest内の質問は独立しているため、各評価質問に候補ID・index・attempt・本文digest・
根拠digestを明示し、他の質問で選ばれた候補を参照させない。

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

生成後は候補選択と全候補の5観点を同じbatchで取得する。

| 質問 | 選択肢 |
| --- | --- |
| `grounding_i` | `SUPPORTED / CONTRADICTED / INSUFFICIENT / NOT_APPLICABLE` |
| `attribution_i` | `CONSISTENT / CONFLICT / UNCLEAR / NOT_APPLICABLE` |
| `task_fit_i` | `MET / UNMET / UNCLEAR` |
| `response_gate_i` | `ACCEPT / RETRY / REJECT` |
| `repair_reason_i` | `NONE / GROUNDING / ATTRIBUTION / TASK_FIT / LANGUAGE` |

未選択の候補も含め、質問キー・選択肢・確率ラベルを過不足なく検証する。
選んだ候補に根拠不足、帰属不明、依頼未達などがあれば、raw gateが`ACCEPT`でも
ホスト側で`RETRY`へ変更する。具体的な修正理由と`ACCEPT`の矛盾も同様に扱う。
`REJECT`は維持する。confidenceは正解率とみなさず、未調整の閾値には使わない。

RETRYでは理由コードから固定の日本語指示を作り、元の依頼・根拠に加えて
`RESPONSE_GUIDANCE`へ渡す。棄却候補の全文とdigestは「修正対象の未信頼データ」として
JSONで分離し、命令や根拠にしない。選択器官だけを1回再生成し、更新した候補群を再評価する。
採用された候補の`response_generated.input_digest`は修正指示を含む実生成入力に対応する。
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

`HAI_API_KEY` が実行環境に設定されている場合、次の2つのOpenAI-compatible言語器官を
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
理由付きretry生成は含めない。器官ごとの遅延の合計ではない。preparationと各assessmentの
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
      "primary": "Evaluate the generated response from the primary organ.",
      "backup": "Evaluate the generated response from the registered backup organ."
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
エラー分類、観測値を保持する。採用後もHTTP呼出し自体は強制cancelしないため、遅れて完了した
器官はこのターンの候補記録・telemetryへ入らない。
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
設定済みJevで通信・TLS・credential・形式検証に失敗した場合、障害をエラーとして
扱い、primaryの自動選択や自動ACCEPTには置き換えない。normalized resultが
`fallback` / `fallback_reason` を持っても、失敗したJev判定を承認の根拠にはしない。
ローカル互換経路の根拠・帰属・依頼適合性は`NOT_EVALUATED`とし、Jev評価済みとは扱わない。
`WAIT`と観測先のない`OBSERVE_MORE`は生成前に停止し、`REJECT`は生成済み応答を採用しない。
`RETRY` は上述の選択器官1回の再生成・候補群全体の再選択に限る。

## 検証範囲と制限

- Rust unit test: 全batch質問のschema・確率検証、根拠／候補binding、判定矛盾、
  recallの出典・subject・サイズ境界、設定済みJev失敗時の非承認、Mock互換。
- ローカルfixture: Jevの `/v1/systemone`、LLMの`/v1/chat/completions`、2ターンの
  preparation→並行生成→assessmentの順序、理由付きretry後の再選択、state伝播、state update。
- Mock closed-loop: 外部APIなしのK-CORE state → Mock language → state update。
- GUIの検証はdesktopの静的検査・ローカルテストと、アプリの実表示確認を区別する。
  実APIのsmokeは対象への明示的な実行承認がある場合だけ行う。credentialの値は
  ログ・trace・runtime.jsonへ保存しない。

未実装の範囲はJ72学習、`KCore::tick()`との統合、音声/TTS、長期記憶の全面再設計、
実モデルの意味品質評価である。
