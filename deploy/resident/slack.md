# Slackから澪と会話する

`kamimusuhi-slack` はSlack Socket Modeで受け取ったメンションを、同じホストの
`POST http://127.0.0.1:7860/v1/kamimusuhi/talk` に渡すRustの会話窓口です。
既存residentの個体・Persona・記憶を使い、元投稿のスレッドに返答します。
公開HTTPポートは不要です。

## Slack側の準備

1. [Slack App管理画面](https://api.slack.com/apps)で
   [slack-manifest.json](slack-manifest.json)を使ってアプリを作成する。
2. Basic Information → App-Level Tokensで`connections:write`付きのトークン
   (`xapp-…`)を作る。Socket Modeが有効であることを確認する。
3. 対象ワークスペースにインストールし、Bot User OAuth Token (`xoxb-…`)を取得する。
   Bot scopeは`app_mentions:read`と`chat:write`、Bot eventは`app_mention`のみ。
4. 操作者専用の非公開チャンネルへBotを招待し、workspace・user・channelのIDを確認する。
   DMは対象外。Botの返信に続ける場合も毎回`@Mio`を付ける。

トークンの作成・権限設定・インストールは管理者が実施してください。
実装作業だけでは認証情報やSlack設定は変更しません。
[Socket Modeの公式仕様](https://docs.slack.dev/apis/events-api/using-socket-mode/)を参照。

## 設定と起動

[slack.env.example](slack.env.example)を参考に、Git外の専用ファイルへ設定する。
`SLACK_TEAM_ID`、`SLACK_ALLOWED_USER_IDS`、`SLACK_ALLOWED_CHANNEL_IDS`は必須。
ユーザーとチャンネルはカンマ区切りで複数指定できる。空なら起動しない。
起動時に`auth.test`でBotのworkspaceを確認する。

`SLACK_OPERATOR_SUBJECT`を指定すると、デスクトップ等と同じ操作者の会話履歴を使う。
許可ユーザーが1人のときだけ設定できる。未指定ならworkspaceとuserの組み合わせを
ハッシュしたSlack用subjectを使う。同一ユーザーは許可チャンネル間で会話履歴を共有する。
subjectは関係性の区分で、個体の資料を利用者別に隔離する認可境界ではない。
**返信はチャンネル閲覧者に見えるため、最初は操作者本人と専用チャンネルだけを許可する。**

```sh
cargo build --release --locked -p kamimusuhi-slack
# 実行前にGit外の設定ファイルからSLACK_*を環境変数に設定する
./target/release/kamimusuhi-slack
```

Piへの配置用に[kamimusuhi-slack.service](kamimusuhi-slack.service)を用意しています。
本番配置・設定ファイル作成・サービス有効化・実送信は別途承認後に実施します。
設定ファイルは`/srv/kamimusuhi/config/slack.env`、権限600で管理してください。

## Piでの常駐起動（常用構成）

Macのスリープ・ログアウトから独立させる場合は、正本個体を持つPi上で起動する。
Slack窓口自身は個体DBを所有せず、Piのloopback residentへ入力を渡す。
デスクトップと同じ`SLACK_OPERATOR_SUBJECT=eightman`を維持すれば、
同じ正本の関係性・履歴・承認規則を使う。Macの別の個体DBをコピー・初期化しない。
過去にMac側で蓄積された別領域の履歴を自動統合する機能はない。

1. Piの専用ビルドディレクトリでaarch64向け`kamimusuhi-slack`をreleaseビルドする。
2. 発行済みの2トークンをSSHで`/srv/kamimusuhi/config/slack.env`へ転送する。
   ディレクトリ700、設定ファイル600、所有者eightman。値をコマンド引数・ログへ出さない。
3. このスクリプトとsystemd unitを同じディレクトリに置いて実行する。

```sh
./install-slack-pi.sh /absolute/path/to/target/release/kamimusuhi-slack --prepare-only
# Mac窓口を停止・自動起動無効化してから、Piの窓口を有効化する。
sudo systemctl enable --now kamimusuhi-slack.service
systemctl is-enabled kamimusuhi-slack.service
systemctl is-active kamimusuhi-slack.service
```

`--prepare-only`を省略すると配置直後に有効化・起動する。
二重応答やイベントの分散を避けるため、同じアプリのMac窓口とPi窓口を併用しない。
Mac側は`launchctl disable gui/UID/com.eightman.kamimusuhi-slack`と
`launchctl bootout gui/UID/com.eightman.kamimusuhi-slack`で自動起動・実行を止める。
Pi側はOS起動時にsystemdが起動し、ログイン不要。既存residentを再配置する必要はない。
Pi自身・ネットワーク・利用する推論先が停止した場合の応答は保証しない。

移行前にMac窓口を停止し、Pi側の接続と実返信を確認する。
ロールバックはPiのSlackサービスを停止・無効化した後、Mac側をenable/bootstrapする。
既存Piバイナリ・unitがある場合はinstallerが`/srv/kamimusuhi/backups/slack-TIMESTAMP/`へ保存する。

## Macでの常駐起動

既存Mac residentと同じログインユーザーのLaunchAgentを利用する。
`~/Library/Application Support/Kamimusuhi/slack/config/slack.env`へ
上記`SLACK_*`を保存する。専用ルート・configディレクトリは権限700、
設定ファイルは所有者本人・権限600とする。Gitやplistにはトークンを書かない。

```sh
deploy/resident/install-slack-mac.sh
```

releaseビルドを専用ルートの`runtime/bin/`へ配置し、
`~/Library/LaunchAgents/com.eightman.kamimusuhi-slack.plist`を登録する。
`RunAtLoad`・`KeepAlive`を有効にし、終了時は最短15秒の間隔で再起動する。
OS再起動後は**ユーザーログイン時**に起動する。ログアウト中・Macスリープ中は応答しない。
既存residentのLaunchAgentも有効であることが前提。

```sh
launchctl print "gui/$(id -u)/com.eightman.kamimusuhi-slack"
launchctl kickstart -k "gui/$(id -u)/com.eightman.kamimusuhi-slack"
# 停止と自動起動登録の解除。保存済みトークンは残す。
deploy/resident/install-slack-mac.sh --uninstall
```

ログは専用ルートの`logs/slack.log`。起動スクリプトは設定の所有者と600を照合してから読み込む。
初期認証の一時的なネットワーク障害で終了してもlaunchdが復旧する。
受信キューはメモリ内なので、会話処理中の再起動を避ける。

## 動作と制約

- workspace・user・channelがすべて許可された人間の元投稿だけを受け付ける。
  Bot、編集、削除、許可外投稿、メンションなし投稿は無視する。
- 全ユーザー共有のFIFOで1件ずつ処理し、待機は最大16件。
  resident呼び出しと切り離してWebSocketのACK・Pingを処理する。
  満杯時はACKせず再接続し、Slack側の再配送を待つ。再配送にはSlack側の上限があり、
  受付や永続配送を保証するものではない。
- Socket Modeの切断・refresh通知時は5秒後に新しい接続URLを取得する。
  最近4096件のevent IDをプロセス内で保持し、再接続時の再配送を抑止する。
- キューと重複抑止はメモリ内。再起動で消える。停止時の未処理入力は失われ、
  再起動後の再配送は重複する可能性がある。resident側に処理が残る場合もある。
- resident要求は最大660秒。生成・送信の曖昧な成功による重複を避けるため、
  resident呼び出しと`chat.postMessage`は自動再試行しない。
  投稿失敗（429を含む）はログに記録し、分割返信の残りは送らない。
- 返信は`response`文字列だけ。内部tool結果やHTTPエラー本文をSlackやログへ転送しない。
  2500文字ごとに分割し、後続投稿の前に1秒待つ。plain text blocksでモデル出力内の
  メンションやリンクの解釈を抑制する。
- 添付、スレッド履歴の取得、自発通知、タスク開始・取消の専用コマンドは未対応。
  通常の会話APIを通じた依頼は既存residentの権限・承認規則に従う。

## 検証

```sh
cargo test --locked -p kamimusuhi-slack
cargo clippy --locked -p kamimusuhi-slack --all-targets -- -D warnings
```

テストはローカルHTTP/WebSocketのモックだけを使用し、Slackには接続しません。
実接続確認では、許可チャンネルで`@Mio こんにちは`へのスレッド返信、許可外ユーザーの
無視、再接続後の応答を確認してください。実送信には対象workspaceへの承認が必要です。
