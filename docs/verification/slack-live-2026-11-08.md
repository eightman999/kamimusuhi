# Slack実接続・送受信確認（2026-11-08 JST）

## 結果

ChromeからMioをメンションし、実Slack → Rust Socket Mode窓口 →
ローカルresident (`127.0.0.1:7860`) → Slackの同一スレッドへの返信まで成功した。
モックによる代替や手動での返信投稿は行っていない。

- Workspace: `eightman` (`T0CFKMEVAAW`)
- App: `Kamimusuhi` (`A0CFT5WNWDQ`)、Bot表示名: `Mio`
- チャンネル: `slack-へようこそ` (`C0CFKMJA5TL`)
- Bot scopes: `app_mentions:read`, `chat:write`
- App-Level Token scope: `connections:write`
- Socket ModeとEvent Subscriptionsが有効であることをChromeで確認。
- 許可対象は操作者本人と上記チャンネルのみ。subjectは`eightman`。
- 使用binary: `target/debug/kamimusuhi-slack`（`cargo build --locked --offline -p kamimusuhi-slack`成功）
- 起動ログ: `Slack Socket Mode connected`
- 送信: 02:12:30 JST。ツール実行・設定変更なしでテスト文字列だけを返すよう依頼。
- 返信: 02:12:35 JST。Mioから `KAMIMUSUHI-SLACK-LIVE-20261108 OK` を受信。

[テスト投稿](https://eightman.slack.com/archives/C0CFKMJA5TL/p1794071550966049)
／[実返信](https://eightman.slack.com/archives/C0CFKMJA5TL/p1794071555262059?thread_ts=1794071550.966049&cid=C0CFKMJA5TL)

## 今回の修正と未検証範囲

Slackのmanifest検証がBot表示名`澪`を拒否したため、登録名を`Mio`へ変更し、
実画面の検証通過とアプリ作成を確認。ローカルmanifest・メンション手順も合わせて修正した。

これはMac上での一時起動による送受信確認。検証用プロセスは終了し、
systemd/launchdへの常駐登録、Piへの配置、再起動後の復旧は実施していない。
実Slack上の許可外利用者テスト、長文分割、切断後再接続の実送受信は未検証。

認証情報は記録に含めない。一時受け渡しには権限600のファイルを使用し、検証後に削除。
Slackアプリとユーザーが発行したトークンは維持する。

## 追加: Mac常駐化（同日02:20 JST）

ユーザーの明示的な保存・常駐化指示により、発行済みの2トークンを
`~/Library/Application Support/Kamimusuhi/slack/config/slack.env`へ保存した。
専用ルートとconfigは700、設定は600。Git・plist・回答にトークンを記録しない。
受け渡し用の一時ファイルは保存後に削除した。

`deploy/resident/install-slack-mac.sh`でreleaseバイナリと専用launcherを配置し、
`com.eightman.kamimusuhi-slack`をguiドメインのLaunchAgentに登録。
RunAtLoad/KeepAliveを有効化、再起動間隔15秒、umask077。
既存のresidentサービスは変更していない。

確認結果:

- releaseビルド、zsh構文検査、plist lint、diff check: PASS。
- インストール済みbinaryがrelease成果物とSHA256一致: PASS。
- 設定所有者・600と親700、plistに認証環境変数がないこと: PASS。
- ログにBot/App tokenの接頭辞がないこと: PASS。
- bootout → bootstrapによる起動: PASS（PID 28358 → 28745）。
- SIGTERMで終了したプロセスのKeepAlive自動復旧: PASS（PID 28745 → 29035）。
- 復旧後、`Slack Socket Mode connected`をログで確認。
- Chromeから同スレッドへメンションし、02:20:10送信 → 02:20:12に
  `KAMIMUSUHI-SLACK-PERSIST-20261108 OK`をMioから受信: PASS。

[常駐サービスの実返信](https://eightman.slack.com/archives/C0CFKMJA5TL/p1794072012864159?thread_ts=1794071550.966049&cid=C0CFKMJA5TL)

最終状態はサービス稼働中。OS全体の再起動・ログアウトは行っていない。
再起動後はユーザーログイン時に起動する方式で、ログアウト中やスリープ中の応答は対象外。
