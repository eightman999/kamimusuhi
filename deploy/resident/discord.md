# Piの澪をDiscordから呼び出す

許可したチャンネルで **`@澪 こんにちは`** とBotを直接メンションする。
Botは受付を返信し、FIFOキューから（subjectごとに順番を守って最大2件並行で）Piの
`POST http://127.0.0.1:7860/v1/kamimusuhi/talk` へ渡す。
返事は元メッセージへの返信になる。既存のPi上の個体・Persona・記憶を使い、別個体は作らない。

## 挙動

- サーバー・チャンネル・利用者IDをすべて照合する。空のallowlistでは起動しない。
  許可外、DM、Bot/Webhook、メンションのない発言は無視する。
- 会話のsubjectは、`DISCORD_OPERATOR_SUBJECT`を設定すると操作者本人のsubject
  （デスクトップが使う`$USER`、例：`eightman`）になり、関係性の記憶・想起・直近の会話を
  デスクトップと共有する。未設定ならサーバー・チャンネル・利用者IDから作るDiscord専用の値で、
  記憶プールがデスクトップと分かれるため、澪から見ると「初対面の相手」に近くなる。
  `DISCORD_OPERATOR_SUBJECT`は許可ユーザーが1人のときだけ使える（他人と記憶を混ぜない）。
  subjectは履歴の区分であり、個体の記憶や参照可能な資料を利用者別に隔離する機能ではない。
  最初は操作者本人と専用の非公開チャンネルだけを許可する。
- **返信と受付表示はチャンネルの閲覧者全員に見える。**
  Discordへ送るのは受付表示と対話APIの`response`のみで、内部tool結果・エラー本文は転送しない。
- キューはプロセス内で共有し、実行中最大2件＋待機最大16件。満杯なら未受付と返信する。
  同じsubjectの発言は1件ずつ受付順に処理し、別subjectの発言だけが並行する。
  編集後の文章や返信先の履歴・添付画像は取り込まず、受付時の文章だけを渡す。
- キューはメモリ内。再起動・停止で未処理分は消える。実行中の要求はPi側で完了している場合がある。
  重複生成を避けるため、再起動時の過去メッセージ取得・自動再実行は行わない。
  同一プロセス内では最近4096件の受付IDを覚え、Gatewayの再配送を抑止する。
- HTTP要求は最大660秒、失敗時の自動再送なし。失敗を返信して次の要求へ進む。
  タイムアウト後もPi側の処理は継続し得る。resident側も全クライアント合計の同時ターン数を
  `dialogue.max_concurrent`（既定2）に制限し、同じsubjectのターンは受付順に1件ずつ実行する。
- 長文はUTF-8の`mio-response.txt`として返す。生成文からのメンション通知とリンク埋め込みを抑止する。
- 返事の下に、そのターンが裏に残した作業を付記する。委譲したタスクは`▶ 裏で実行中：t…（devin）`、
  「調べてみますね」と引き受けたのにタスクを作れなかった場合は操作者をメンションして
  `⚠ 引き受けましたが、タスクはまだ作成されていません（t…）`と開始・取消の方法を示す。

## 判断待ちの通知（メンション付き）

操作者の判断が要るときは、通知チャンネルに**操作者だけをメンション**して投稿する
（`@everyone`・ロール・他の利用者は鳴らさない）。約30秒ごとにローカルAPIを確認する。

| 通知 | 契機 | 操作 |
|---|---|---|
| 未着手の引受 | 澪が作業を約束したが、再要求しても委譲しなかった（board の `commitment`） | `@澪 開始 t…` / `@澪 開始 t… devin` / `@澪 取消 t…`。通知への返信なら`@澪 開始`だけでよい |
| 承認待ち | 書き込み系ツールが承認待ちになった | デスクトップの「承認」画面で判断する（Discordからは承認しない） |
| 委譲タスクの終了 | 外部エージェントのタスクが完了・失敗・取消 | 澪の評価（採用／不採用／保留、最大10分待つ）と報告の要約。ブランチがあれば取り込みはデスクトップで判断 |

- 宛先は`DISCORD_NOTIFY_USER_ID`／`DISCORD_NOTIFY_CHANNEL_ID`（許可リスト内のみ）。
  未設定なら許可ユーザー・許可チャンネルが1つのときそれを使い、複数なら通知しない。
  `DISCORD_NOTIFY=off`で無効。
- **通知には依頼文と澪の返事の冒頭（各300字まで）が載る。** デスクトップで話した内容も
  通知チャンネルに出るため、チャンネルは操作者専用にする。
- 開始時に実行者を指定しなければ、依頼文に実行者名（例：`Devinで調べて`→`devin`）が
  1つだけ書かれていればそれを使い、なければ自動選択になる。
- 投稿済みの通知は`StateDirectory`（`/var/lib/kamimusuhi-discord/notices.json`）に残り、
  再起動しても同じ通知を繰り返さない。委譲タスクの終了通知は通知を初めて有効にした時点
  以降に終わったものだけが対象。
- 開始・取消コマンドはキューを通らず即時に処理する。対象は`commitment`のみで、
  失敗時はHTTP状態だけを返し、内部エラー本文は転送しない。

## 接続

- GatewayはPiからDiscordへ接続するため、公開HTTPポートやInteractions Endpoint URLは不要。
  `GUILDS` / `GUILD_MESSAGES`のみ使用。直接メンションの本文は
  [Discord公式のMessage Content Intent例外](https://support-dev.discord.com/hc/en-us/articles/6383579033751-Message-Content-Intent-Alternatives-Workarounds)
  に該当し、特権Message Content Intentは不要。

## Discord側の準備

1. [Developer Portal](https://discord.com/developers/applications)で澪用のApplication/Botを用意する。
2. Guild Installの`bot`スコープで対象サーバーへ追加する。
   権限はView Channel / Send Messages / Read Message History / Attach Files。
   スレッドを使う場合はSend Messages in Threadsも必要。管理者権限は不要。
3. Discordの開発者モードでサーバーID・チャンネルID・操作者のユーザーIDを取得する。
   スレッドは親チャンネルでなく**スレッドIDそのもの**をallowlistへ追加する。
4. Botトークンは下記のPiの専用設定ファイルへ保存する。会話やGitに貼らない。

## 配置・起動（承認後に実施）

対象はcontinuityノードのPiのみ。返事への付記と判断待ち通知には、`background`と`commitment`を返すresident（未着手の引受・委譲レビュー対応版）が必要。
既存ファイルがある場合は置換前にバックアップする。以下はPi上のソースディレクトリで実行する。

```bash
python3 -m venv /srv/kamimusuhi/runtime/discord-venv
/srv/kamimusuhi/runtime/discord-venv/bin/python -m pip install -r deploy/resident/discord-requirements.txt
install -m 755 deploy/resident/discord_bot.py /srv/kamimusuhi/runtime/bin/discord_bot.py
# 初回のみ。既存のdiscord.envを上書きしない。
test -e /srv/kamimusuhi/config/discord.env || install -m 600 deploy/resident/discord.env.example /srv/kamimusuhi/config/discord.env
```

`/srv/kamimusuhi/config/discord.env`を直接編集して以下を設定する（ID一覧はカンマ区切り）。
所有者はサービスユーザー`eightman`、modeは600にする。

| キー | 内容 |
|---|---|
| `DISCORD_BOT_TOKEN` | Botトークン |
| `DISCORD_GUILD_ID` | 1つのサーバーID |
| `DISCORD_ALLOWED_USER_IDS` | 操作者のユーザーID |
| `DISCORD_ALLOWED_CHANNEL_IDS` | 専用チャンネルID／スレッドID |
| `DISCORD_OPERATOR_SUBJECT` | 任意。デスクトップと同じsubject（例：`eightman`）。ペルソナ・記憶を揃える |
| `DISCORD_NOTIFY` / `DISCORD_NOTIFY_USER_ID` / `DISCORD_NOTIFY_CHANNEL_ID` | 任意。判断待ち通知の有効化・宛先（上記「判断待ちの通知」） |

residentの`secrets.env`はBotに読み込ませない。ローカルAPI接続にはnode token不要。
配置前の設定ファイルをMacで用意する場合はGit対象外の`deploy/resident/local/`に保存する。

```bash
sudo install -m 644 deploy/resident/kamimusuhi-discord.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now kamimusuhi-discord
systemctl is-active kamimusuhi-discord
journalctl -u kamimusuhi-discord -n 30 --no-pager
```

サービスユーザーが異なる環境ではunitのUser/Groupを合わせる。
停止・戻し方は`sudo systemctl disable --now kamimusuhi-discord`。
これはresidentと個体DBを停止・削除しない。Discord側で利用を終了する場合はBotをサーバーから外す。

## 検証

ローカル（Discord接続なし・一時HTTPサーバー）：

```bash
python3 -m venv .venv-discord
.venv-discord/bin/python -m pip install -r deploy/resident/discord-requirements.txt
.venv-discord/bin/python -m unittest discover -s deploy/resident -p 'test_discord_bot.py' -v
```

実接続後は専用チャンネルで2つのメンションを送り、受付→順番どおりの返信、
Pi側の同じindividual_idと会話記録、許可外からは反応しないことを確認する。
ローカルテストだけではDiscord上の配送・Bot権限・Piでの会話品質を検証したことにはならない。
