# APIでも継続する推論

推論先をローカルモデルからAPIに変えても、会話側K-COREは
`判断 → 言語生成 → 回答評価 → 必要なら1回再生成` の閉ループを維持する。
言語生成先だけをAPIへ移し、判断部分を無条件に規則へ置換しない。
個体、記憶、権限、根拠の採否は引き続きホストとContinuity Kernelが所有する。

待機中の自律思考も[入力なしの内省scheduler](idle-cognition.md)から同じAPIへ接続できる。
実会話の新規経験だけを対象にし、source cursor・backoff・日次上限で重複呼出しを抑える。

## 判断先の選択

`runtime.json` の `decision` は次の値を取る。

| 値 | 動作 |
| --- | --- |
| `auto`（未指定時） | Jev設定があればTypeSafe。なければprimary Personaと同じAPI。fake Personaだけは規則ベース。 |
| `persona_api` | primary PersonaのAPIを判断にも使用。Jevの有無には依存しない。 |
| `typesafe` | Jevを使用。未設定なら実行前に構成エラー。 |
| `rule_based` | 決定論的な旧動作。文章生成のみを検証するfixtureなどで明示する。 |

APIのURL、モデル、認証環境変数名、TLS、localityはprimary Personaの宣言を使う。
回答文用のsystem promptやツールは判断要求へ持ち込まない。
推論のため `enable_thinking=true` を要求するが、実際の思考モードは接続先の対応に依存する。
これは隠れた思考本文を取得・記録する機能ではない。

```json
{
  "decision": "auto",
  "persona": {
    "backend": "openai-compatible",
    "provider": {
      "backend_id": "既存のIDを維持",
      "locality": "external",
      "base_url": "http://127.0.0.1:7860/v1",
      "model": "kamimusuhi",
      "timeout_ms": 180000
    }
  }
}
```

上記は設定の抜粋。既存ファイル全体やbackend IDを置換する手順ではない。
APIはOpenAI-compatible Chat CompletionsのJSON modeに対応している必要がある。

## APIに行わせる判断

発話前には、発話するか、追加観測が必要か、提示済み記憶のどれが関連するかを判断する。
生成後には、候補選択、根拠との整合、自己と他者の帰属、依頼達成度、再生成の必要性を判定する。
APIとJevは同じ質問・選択肢・検証・ホストによる採否合成を使う。

返却JSONは全質問、選択肢、確率分布、候補ID、根拠digestと照合する。
形式不正は安定したエラーコードを添えて1回だけ再要求する。
なお不正なら停止し、評価に合格したものとして配信しない。
通信障害時だけ既存の縮退動作を使い、traceへ失敗段階と理由を残す。
同じAPIモデルが生成と評価を行う場合、独立した評価者による検証ではない。

判断要求はツールを持たず、出力上限は1回4096トークン。
文章用の短い出力上限、stop、logit bias、複数候補数は継承しない。
residentのCostGuardにはこの上限が明示され、判断も通常のAPI要求として利用履歴に残る。
通常は発話前・生成後の2要求が加わるため、遅延と利用量も増える。

## 同一性と配備

判断workerは言語workerと同じreview済みroute pinとモデル宣言を受け取る。
宣言外の返却モデル、不正JSON、privacy違反を通信障害として隠さない。
API差替えは計算能力の差替えであり、個体DBを初期化したり履歴を作り直したりしない。

判断の役割、設定、プロンプト実装はFI3 manifestに含む。
既存個体への更新は、本番配備と正確なreview tokenによる明示承認が必要。
自動承認を起動処理へ追加しない。詳細は
[FI2/FI3の配備手順](verification/fi2-fi3-integration-2026-11-08.md)を参照。

## 検証範囲

ローカルHTTP fixtureで、APIだけの判断・生成・評価、品質不足の再生成、
不正応答の停止、worker隔離、privacyとidentityの検証を行う。
fixtureは実モデルの意味理解や品質を証明しない。

会話側は入力されたターンを、待機側は設定subjectの未処理の実会話を扱う。
数値実験の`KCore::tick()`や外部タスクの自動承認には接続しない。
会話ターン数によるreflectionと待機中schedulerは別々に設定できる。
