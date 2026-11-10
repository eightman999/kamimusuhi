# resident のエンジニアエージェント

`engineering` は resident 自身が開発目標を追う実行系です。外部 coding CLI や
SKILL.md を必須にせず、設定された推論経路へ一手ずつ判断を求め、隔離した
git worktree で編集・実行・検証を進めます。resident を稼働させている間は、その
スケジューラが会話の終了後も続行します。

## 実行の流れ

1. 目標、対象 workspace、元の HEAD、候補のパスとブランチを保存する。
2. 記録した HEAD から worktree を用意する。元の作業ツリーの未コミット変更は移さない。
3. 目標、作業メモ、直近の観測と検証結果から、モデルが次の行動を一つ選ぶ。
4. 行動を `pending` として保存できた場合にだけ実行する。
5. 実際の結果を保存し、次の判断へ進む。検証失敗なら同じ候補で修正する。
6. `finish` は設定された検証コマンドを実行する。全て成功し、検証終了時の
   HEAD・branch・git status の検査も通れば `ready` になる。

目標全体の固定ターン数・固定試行回数は設けていません。操作ごとの期限、出力量、
並行実行の排他と取消は別に管理します。`ready` は、保存した commit がその時点の
検証設定で成功した状態です。現在の候補の状態は、下記の `verification` 表示と
区別して確認します。
候補ブランチを残し、マージ・push・デプロイはこのエージェントでは実行しません。

## 設定

resident.json に `engineering` を加えます。省略したノードでは起動しません。
推論には既存の `tiers` / `routing` が必要です。以下は設定部分の例です。

```json
{
  "engineering": {
    "model": "auto",
    "tick_secs": 2,
    "operation_timeout_secs": 120,
    "validation_timeout_secs": 900,
    "max_output_bytes": 32768,
    "max_tokens": 4096,
    "workspaces": [
      {
        "name": "project",
        "path": "/srv/workspaces/project",
        "checks": [
          ["cargo", "test", "--offline"],
          ["cargo", "fmt", "--all", "--", "--check"]
        ],
        "commands": ["cargo", "rg"]
      }
    ]
  }
}
```

`path` は git リポジトリの最上位の絶対パスです。候補は resident の
`paths.root/engineering-worktrees/` に作成します。そこを元リポジトリの内側に
置く設定は拒否します。

`checks` は操作者が選ぶ argv 配列で、少なくとも一件必要です。シェル展開は
行いません。モデルは検証コマンドの設定を書き換えられません。判定は実際の
終了コードと候補の状態に基づきます。検証項目の妥当性までは保証しないため、
目的に対応した回帰試験や独立した受入試験を設定してください。

`commands` は、モデルが任意の引数で実行できるプログラム名の完全一致リストです。
空なら追加のコマンド実行を許可せず、ファイル操作と設定済み検証だけを使います。
これは OS の sandbox ではありません。許可されたプログラムや検証コードは
実行ユーザーの権限で動くため、ホスト側の実行環境と権限を選んでください。
子プロセスには既存 runner の最小環境だけを渡し、resident の API キーや
ノードトークンの環境変数は追加しません。

## 大きなソースの調査と修正

小さいファイルは `read` と全文の `write` を使えます。`read` の結果が切り詰められた
場合は、`read_range` で必要な行を読みます。行番号は 1 から始まり、1 回に 1〜200 行を
指定できます。返却する JSON 全体を出力枠に収め、UTF-8、CRLF、改行を途中で切りません。
`next_line` はまだ返していない最初の行で、末尾まで読めば `null` です。
1 行だけで枠を超える場合は、内容を飛ばさずエラーを返します。

```json
{"action":"read_range","path":"src/lib.rs","start_line":6000,"max_lines":100}
```

`replace` は候補ファイルの中で `old` が一意に一致するときだけ置換します。
全文をモデルへ渡す必要はありません。空の文字列、重なりを含めた複数一致、変更のない
置換は拒否します。保存直前にも元内容との一致を確かめ、既存の原子的な書き込みを使います。
入力ファイルは UTF-8 で 1 MiB 以下、`old` と `new` はそれぞれ 16 KiB 以下です。

```json
{"action":"replace","path":"src/lib.rs","old":"let answer = missing_value;","new":"let answer = 42;"}
```

## 起動と観測

```sh
kamimusuhi engineer start project '不具合を修正し、回帰試験を追加する' --url http://127.0.0.1:PORT
kamimusuhi engineer list --url http://127.0.0.1:PORT
kamimusuhi engineer show JOB_ID --url http://127.0.0.1:PORT
kamimusuhi engineer pause JOB_ID --url http://127.0.0.1:PORT
kamimusuhi engineer resume JOB_ID '保存済みの候補を確認して続ける' --url http://127.0.0.1:PORT
```

HTTP は `GET /v1/engineering` と `POST /v1/engineering` です。
既存 resident と同じ loopback / bearer 認証境界に置かれます。

```json
{
  "action": "create",
  "workspace": "project",
  "objective": "失敗する回帰試験を修正する",
  "request_id": "operator-request-001"
}
```

同じ `request_id` と同じ内容の再送は既存 job を返します。異なる内容への
使い回しは拒否します。作成後は返された `job.id` で `show` / `pause` / `resume`
を呼びます。

主推論 Mio から起動する場合は、個体の `tools.allowed` に `engineering` を
許可します。通常の tool catalog 経路から管理用の入口を発見できます。
目標の遂行は、resident のエンジニアスケジューラが引き受けます。

## 検証した commit と設定の表示

`validation` は検証結果の保存先で、`head`、各 check の結果、任意の `contract` を
持ちます。新しい成功・失敗の検証 receipt には、`contract.schema_version: 1`、
設定の `policy_digest`、検証処理が終了した Unix 秒の `completed_at` を保存します。
この時刻は検証時の記録であり、現在の候補を観測した時刻としては使いません。

receipt がある job には、表示用の `verification` も付きます。
`scope: recorded_commit` と `head` が検証した過去の commit を示し、`passed` は
その検証結果を示します。show、list とその summary、resident 全体の status、
同じ `request_id` の再送結果、モデル context で同じ意味を使います。

`verification.policy.status` は、保存した検証設定と現在設定の対応です。

| 値 | 意味 |
|---|---|
| `matching` | 保存済み digest と現在の検証設定から計算した digest が一致する |
| `changed` | 両方の digest があり、現在の検証設定が変わっている |
| `unknown` | 旧 receipt に contract がなく、検証時の設定が記録されていない |
| `unavailable` | contract はあるが、その job の workspace に対応する現在設定がない |

比較には、workspace の名前・設定された path、順序を含む checks の argv、検証の
期限、出力枠を使います。モデル、tick 間隔、トークン枠、Run の期限、追加コマンドの
allowlist はこの digest の対象外です。外部 script や実行ファイルの内容、PATH、
toolchain、環境変数、外部サービスの状態も比較していません。`matching` が示す
範囲は、これらの記録対象となった設定値の一致です。

現在の候補については、`verification.candidate.status: not_observed` と
`observed_at: null` を表示します。これらの表示経路では git、ファイルの有無、実検証、
モデル呼び出しを追加しません。外部から候補を編集・commit した場合や、ディレクトリを
移動した場合も、保存された成功と現在状態を分けて `not_observed` を維持します。
`policy.status: unavailable` は設定の対応を表し、候補ディレクトリの有無は別の確認事項です。

設定変更後も phase と validation は過去の記録として保持します。現在の設定で
再検証する場合は、Ready から明示的に `resume` して作業を続けます。resume は
以前の validation を消し、その後の Verify / Finish が新しい検証結果を作ります。

旧 checkpoint の contract 省略は `None` として読み込めます。現在の設定や時刻から
過去の情報を補わず、`recorded_digest` と `completed_at` は `null`、policy は
`unknown` になります。contract 自体がある場合は、版・digest の形式・必須 field を
検査します。不正な contract は job の phase にかかわらず、pending の回復より前に
検出して store を隔離し、原本を保持します。

## 継続と再開

`current_state/engineering.json` に目標・候補・作業メモ・直近の観測・検証証拠・
行動の cursor を保存します。重要な更新はファイルと親ディレクトリを同期して
から、次の操作へ進みます。保存失敗時に処理を継続せず、元の checkpoint を
壊れた内容で上書きしません。

schema v2 の `next_action_sequence` は、完了件数とは独立した次の行動番号です。
claim を保存するときに番号を消費し、結果が不明でも再使用しません。完了観測と
trace はその claim と同じ番号を持ち、準備・推論失敗などの制御イベントは 0 を使います。
`metrics.actions` は結果を保存できた行動件数なので、中断を挟むと番号とは一致しません。

旧 schema v1 は、全 job を検査した後に保存済み番号と計測値から cursor を導出し、
中断の回復を終えてから全体を一度だけ v2 として保存します。目標・候補・既存の
履歴番号は変更しません。v2 を再び読むときは、履歴の間引きによって cursor を戻しません。
v2 対応前の resident へ戻すと、この状態ファイルは未対応版として実行を拒否します。

`preparing` の途中で再起動した場合は、同じ候補の準備を検査して再開します。
完了済みの行動は再実行しません。書き込み途中なら、保存された候補を保持し、
結果が不明だったことを次の判断へ渡します。部分置換・コマンド・検証・finish の途中で
再起動した場合は `paused` にして、結果不明の副作用を自動で再実行しません。
実行先を確認した後に `resume` できます。

読み込みより前に、専用の `current_state/engineering.lock` を OS の排他ロックで
取得し、store の生存中は保持します。このファイルは削除・置換しません。同じ root
の後続 resident は engineering の load / create / pause / resume / 実行を拒否し、
HTTP の status.error から理由を確認できます。resident全体の起動排他ではなく、
engineering の状態と実行の所有権です。所有プロセスの終了後は次の resident が
取得できます。現在の実装は Unix 対象で、ネットワーク filesystem の分散ロックは
検証していません。

JSON を読めることに加え、全 job の ID、候補のパスとブランチ、pending cursor、
計測値、Ready の検証証拠を回復前に検査します。一部だけ回復してから不整合に
気づく順序にはしません。不整合や保存失敗はその store を隔離し、scheduler を
停止します。HTTP は状態を表示できます。候補や破損した原本は自動削除しません。


作業メモはモデル自身の未検証なメモとして扱います。ファイル内容やコマンド出力も
データとして渡し、そこから workspace・検証設定・実行権限を変更しません。
長期タスクでは直近の観測を間引きますが、目標、次の作業、候補の識別子、検証の
結果は別フィールドで保持します。

モデルへ渡す observations 配列は、JSON のエスケープと区切り文字を含めて 32 KiB
以内に収めます。新しい観測を優先し、入らない古い項目は省略します。ReadRange は
内容と行 cursor を途中で分断しません。他の大きい診断は元 detail を 8 KiB までに
切り詰め、`context_truncated` を付けます。
`observations_omitted` は、保存済みの直近最大12件のうち今回の context で省略した
件数です。既に履歴から間引かれた過去の総数は含みません。この 32 KiB は観測配列の
枠であり、目標・設定・作業メモを含むリクエスト全体の上限ではありません。

実チェックの失敗診断は `validation.failure_evidence` に別途残します。長い調査で
失敗観測が直近12件から消えても、保存済みreceiptから次の判断へ渡せます。成功checkを
除外し、integrity失敗も含め、各診断は最大1024byteのUTF-8の抜粋、配列全体は
JSON encodedで最大4KiBです。文字列の途中へ説明を追加せず、`context_truncated` と
`failure_evidence_omitted` で欠落を知らせます。抜粋自体は完全なJSONとは限りません。
診断は未信頼の外部データで、目標や検証設定を変更する指示として扱いません。
この4KiBもリクエスト全体の上限ではなく、32KiBの観測配列とは独立した枠です。

## コマンドの寿命と結果不明の扱い

Run・Verify・Finishは、resident自身と同じbinaryの専用workerで実行します。追加の
外部coding CLIは必要ありません。workerはresidentのconfig・token・HTTP serverを
初期化せず、親が保存したclaimと実行設定を確認してから操作します。

実行中の worker が Verify / Finish の検証結果を返す場合、`validation.contract` が必要です。
親は request と reply の job・claim 番号・操作種別に加え、contract の版・形式と、
その request の検証設定 digest を照合します。成功・失敗のどちらにも適用し、
欠落・不正・別設定の receipt は結果不明として扱います。旧 checkpoint の
`contract: None` 互換は、この実行中の返信を受け入れる条件には適用しません。

親との制御pipeが閉じるとworkerは取消を通知し、commandのprocess groupを停止して
direct childを回収します。操作の期限もworker内で監視するため、residentだけが
終了しても停止処理を続けられます。親側も取消時は先にpipeを閉じ、有限のcleanup
猶予を与えます。workerが返す失敗receiptと、worker自体の消失で結果が不明な場合を
区別します。後者はjobをPausedにし、完了actionsへ加算せず、行動番号を保持します。

worker自身も異常終了した場合や、commandが別process groupへ離脱した場合の全子孫
回収は保証しません。開始前のclaim検査と、その直後の再開操作の間にも完全なfencingは
ありません。結果不明のjobを明示resumeするときは、候補と旧実行の停止を確認します。
現在のtransportはUnix向けで、request/replyは各1MiB、workerが読むcheckpointは
64MiBまでです。上限超過は実行前拒否または結果不明として返し、resident内で同じ
操作を代わりに実行するfallbackはありません。

`tests/engineering_worker.rs` は実binaryのpipe切断・期限・claim不一致を検査します。
`tests/engineering_http.rs` は実residentの終了とworker消失後の候補保持・一時停止・
明示再開を検査します。

## 推論障害の記録

`model_recovery` は直近の推論障害の経緯です。最初と最新のエラー、各時刻、
その期間の失敗回数、回復時刻を保存し、短い観測履歴とは別に次の判断へ渡します。
最初のHTTP503の後に「利用可能な送信先なし」のエラーが繰り返されても、最初の原因を
上書きしません。`metrics.model_failures` は全期間の失敗数で、HTTP送信しなかった
local routingの失敗も含みます。

回復時刻は、有効な次のactionをclaimとして保存できた時点です。そのactionの成功や
開発目標の検証成功を意味しません。pauseで見送った提案や保存に失敗したclaimでは
回復にせず、後続actionでも時刻を書き換えません。新たな推論障害が起きたときに
次のepisodeへ置き換わります。旧v2の状態にはこのfieldがなくても読み込めます。

## 検証の範囲

`engineering::tests` はモデルの判断と実行場所の境界をfixtureに置き換え、一時 git、
実 worktree、実コマンド、ディスクへの保存・再読込を使います。成功を申告した
モデルでも検証が失敗すれば `ready` にならず、修正後の再検証で初めて到達することを
確認します。同時 tick の重複実行、再開、保存失敗、元の未コミット成果の保持も
対象です。実daemonの試験では、完了後の再起動に加え、Active の推論中断、
同rootの別residentによる操作拒否、所有者終了後の再取得と未コミット成果の継続を
確認します。不正な行動JSONと一時的なprovider HTTP503からの回復も、実routerと
loopbackのfixtureで検証します。外部モデルのコーディング能力や、本番環境の動作を
測る試験ではありません。

実装は `crates/kamimusuhi-resident/src/engineering.rs` と
`engineering/workspace.rs`、検証の表示は `engineering/verification.rs`、制御の
fixture は `engineering/tests.rs` にあります。

検証設定の同一性は `engineering::tests::verification_identity`、実行中の receipt の
照合は worker の codec fixture で検査します。測定記録は
[verification-identity-20261110.json](../evaluation/harness/verification-identity-20261110.json)
を参照してください。この記録は元コンテナの結果と復元版の実行状況を分けています。
旧ログから復元した PASS を、新しい環境で再実行した結果としては扱いません。

### 観測に基づく継続性の評価

`engineering::tests::evaluation` の模擬モデルは、受け取った目標・作業メモ・実検証・
読取結果だけから次の行動を選びます。実検証失敗からの修復、作業メモを保存した後の
新しいモデルでの継続、結果未確認の置換が適用済み／未適用の4ケースを検査します。
置換の中断2ケースでは操作者の明示resumeを含み、現在のソースを読んでから判断します。
成功率、model calls、decisions/actions、実check、重複作用、resume数を一つのJSONで
出力します。実LLMのコーディング能力を測るベンチマークではありません。

```sh
cargo test -p kamimusuhi-resident --lib engineering::tests::evaluation:: -- --nocapture
```

保存済みの測定は `evaluation/harness/native-reactive-20261110.json` を参照してください。
