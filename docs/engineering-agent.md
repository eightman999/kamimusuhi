# resident のエンジニアエージェント

`engineering` は resident 自身が開発目標を追う実行系です。外部 coding CLI や
SKILL.md を必須にせず、設定された推論経路へ一手ずつ判断を求め、隔離した
git worktree で編集・実行・検証を進めます。会話が終わっても resident の
スケジューラが続行します。

## 実行の流れ

1. 目標、対象 workspace、元の HEAD、候補のパスとブランチを保存する。
2. 記録した HEAD から worktree を用意する。元の作業ツリーの未コミット変更は移さない。
3. 目標、作業メモ、直近の観測と検証結果から、モデルが次の行動を一つ選ぶ。
4. 行動を `pending` として保存できた場合にだけ実行する。
5. 実際の結果を保存し、次の判断へ進む。検証失敗なら同じ候補で修正する。
6. `finish` は設定された検証コマンドを実行する。全て成功し、検証中に候補が
   変わっていなければ `ready` になる。

目標全体の固定ターン数・固定試行回数は設けていません。操作ごとの期限、出力量、
並行実行の排他と取消は別に管理します。`ready` は検証済み候補がある状態です。
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

## 継続と再開

`current_state/engineering.json` に目標・候補・作業メモ・直近の観測・検証証拠・
行動の cursor を保存します。重要な更新はファイルと親ディレクトリを同期して
から、次の操作へ進みます。保存失敗時に処理を継続せず、元の checkpoint を
壊れた内容で上書きしません。

`preparing` の途中で再起動した場合は、同じ候補の準備を検査して再開します。
完了済みの行動は再実行しません。書き込み途中なら、保存された候補を保持し、
結果が不明だったことを次の判断へ渡します。コマンド・検証・finish の途中で
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

## 検証の範囲

`engineering::tests` はモデルの判断だけを fixture に置き換え、一時 git、
実 worktree、実コマンド、ディスクへの保存・再読込を使います。成功を申告した
モデルでも検証が失敗すれば `ready` にならず、修正後の再検証で初めて到達することを
確認します。同時 tick の重複実行、再開、保存失敗、元の未コミット成果の保持も
対象です。実daemonの試験では、完了後の再起動に加え、Active の推論中断、
同rootの別residentによる操作拒否、所有者終了後の再取得と未コミット成果の継続を
確認します。外部モデルのコーディング能力や、本番環境の動作を測る試験ではありません。

実装は `crates/kamimusuhi-resident/src/engineering.rs` と
`engineering/workspace.rs`、制御の fixture は `engineering/tests.rs` にあります。
