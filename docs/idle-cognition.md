# 待機中のAPI自律思考

residentは設定された相手について、対話がないときに入力なしの`cogitate`を実行する。
runtimeは未処理の実会話がある場合だけ、既存のAPI reflectorで内省する。
会話中の[判断・熟考](api-cognition.md)と併用でき、ローカルGPUの存在には依存しない。

## 有効化

residentの設定に、対象subjectを明示して次の項目を追加する。
未設定時はschedulerを起動しない。既存設定全体をこの抜粋で置き換えない。

```json
{
  "cognition": {
    "subjects": ["eightman"],
    "poll_interval_secs": 30,
    "timeout_secs": 180
  }
}
```

runtimeの`runtime.json`では`c0.idle`で実行条件を調整できる。
未指定時も以下の値を使う。設定・実装の変更はFI3の明示review対象になる。

```json
{
  "c0": {
    "idle": {
      "idle_after_secs": 60,
      "min_interval_secs": 600,
      "retry_after_secs": 600,
      "max_attempts": 3,
      "max_runs_per_day": 24
    }
  }
}
```

`reflection_interval_turns`は会話ターン数による別の起動条件で、0でも上記schedulerは使える。
API先は既存`c0.reflector`に従い、既定の`mirror`はprimary Persona APIを使う。
`cognition.subjects`に入れた相手だけが対象。別相手の会話・metrics・草稿を混ぜない。
個体に共通する自己モデルと既存のmutation/replay gateは維持する。

## 実行条件と記録

- foregroundの対話を処理中ならresidentは起動を見送る。起動後に来た対話も受付を妨げない。
- runtimeは対象subjectの実会話sourceをSQLで限定して取得し、直近32件を内省の材料にする。
  `idle_after_secs`が経過していなければAPIを呼ばない。
- 未処理sourceのdigestが変わったときだけ実行する。内省自身のReflectionやSystemEventは
  新しい経験に数えず、自分の出力で呼び出し続けない。
- 最小間隔と日次上限は個体全体で共有する。失敗した試行も上限へ計上する。
  日次はUTCで区切り、時計が巻き戻っても予算を再付与しない。
  上限は内省cycle数であり、schema修復や自己改善replayのAPI要求数とは異なる。
- `idle-cognition.json`にsubject別cursor・試行回数・状態を原子的に保存する。
  `idle-cognition.lock`により、同じ個体で複数processが重複実行しない。
- API応答前の失敗はbackoff後に最大3回まで試行する。
  intake開始後の失敗やprocess中断は、上流で処理されたか不確定なので同じsourceを自動再送しない。
  `INTERRUPTED_UNCERTAIN`として保持し、新しい実会話があれば次の思考へ進む。

入力なしの単一判定は次のコマンドで行える。周期実行・期限管理はresidentが担当する。

```sh
kamimusuhi-runtime cogitate --dir <既存の個体ディレクトリ> --subject eightman --privacy unconstrained
```

privacyはresidentの既存`dialogue.privacy`を継承する。
reflectorと、auto activation時のreplay用PersonaのlocalityをAPI送信前に照合する。
認証/TLS/route pin/返却モデルの検証は会話側と同じtransportを使う。
自己改善のreplayはtoolsなしで行い、過去のユーザー依頼を外部操作として再実行しない。

`cogitate`はUserUtterance・AgentUtterance・対話turnを作らない。
内省の結果は推論に由来するReflection evidenceとして保持し、事実や発話として扱わない。
canonical draftは既存kernel、改善draftは既存replay gateを経由する。
別subject向けのRelationship/Episodic draftは拒否し、勝手に書き換えて採用しない。
Slack等への自動投稿や外部タスクの自動承認は追加しない。

## 確認

`GET /status`の`cognition`にdisabled/running/idle/cooldown/completed/interrupted等の状態、
対象subject、安定した理由コード、安全な最終要約を返す。
`logs/cognition`にも実行結果を記録する。会話本文・内省の本文・raw stderr・認証値は公開しない。
API要求は通常のresident利用監視とCostGuardを通る。

新規の経験がない状態で無条件に思考を続ける機能ではない。
外部観測だけの変化や、すでに処理した会話を別モデルで再評価する起動条件は今回含まない。
API実モデルの内省品質、本番での連続稼働はfixtureだけでは検証できない。
