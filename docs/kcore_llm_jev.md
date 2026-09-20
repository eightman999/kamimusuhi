# Kamimusuhi: 会話側K-CORE + LLM + Jev

## 位置づけ

この実装では `DialogueSession` を会話側K-COREのインターフェースとして扱う。
数値実験系の `KCore::tick()`、16次元観測、J72のtokenizer/学習系列には接続しない。

会話の閉ループは次の順序で進む。

```text
User
  -> DialogueSession / conversation-side K-CORE state
  -> Jev /v1/systemone: invocation_gate
  -> language provider /v1/chat/completions
  -> Jev /v1/systemone: response_gate
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
`SPEAK / WAIT / OBSERVE_MORE`、`response_gate` は `ACCEPT / RETRY / REJECT`
だけを受け付ける。返却の `choice`、`confidence`、確率分布はスキーマと意味の
両方を検証し、壊れた応答は1回だけ再試行する。

## Language provider

現行の確定経路はGrokbot VMの3B系言語器官である。

```bash
KAMIMUSUHI_LLM_PROVIDER=grokbot
KAMIMUSUHI_LLM_BASE_URL=http://100.120.99.78:8080/v1
KAMIMUSUHI_LLM_MODEL=lfm2.5-2.6b-qad-q4_0
GBVM_API_KEY=
```

言語器官のwire endpointは、既存Persona HTTP transport経由の
`POST ${KAMIMUSUHI_LLM_BASE_URL}/chat/completions` である。`:8081`/`:8082`の
J72は今回の経路で使わない。`KAMIMUSUHI_LLM_PROVIDER=mock`を指定すれば外部API
なしで決定論的Mockを使える。provider名を変えても会話側は
`LanguageProvider`のままで、endpointやmodelは環境設定から解決する。

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
Jevのnormalized decision、provider/model、latency、fallback、state transition
だけをstderrまたはtalkのJSONに含める。`--debug-context`は従来どおり、言語器官に
渡したworkspace contextを別途表示する。

## fallback

Jevが未設定ならrule-based decision providerを使う。Jevが設定されていて通信・TLS・
credential・形式検証に失敗した場合も、同じ候補語彙を持つ決定論的fallbackへ退避し、
normalized resultの `fallback` と `fallback_reason` に分類を残す。候補が有効な
`WAIT`/`OBSERVE_MORE`/`REJECT`の場合は、勝手にLLMを呼ばずgateの結果をエラーとして
返す。`RETRY`は1回だけ言語器官を再呼出しする。

## 検証範囲と制限

- Rust unit test: Jev choice schema、確率検証、fallback。
- ローカルfixture: Jevの `/v1/systemone`、LLMの`/v1/chat/completions`、2ターンの
  request順序、state伝播、state update。
- Mock closed-loop: 外部APIなしのK-CORE state → Mock language → state update。
- 実APIは大量リクエストを避け、credentialが現在の実行環境に明示的に公開されて
  いる場合だけ単発smokeを行う。値はログ・trace・runtime.jsonへ保存しない。

未実装の範囲はJ72学習、`KCore::tick()`との統合、音声/TTS、長期記憶の全面再設計、
実モデルの意味品質評価である。
