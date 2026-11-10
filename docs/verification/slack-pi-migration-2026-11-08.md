# Slack窓口のPi移行と一貫性確認（2026-11-08 JST）

## 結果

MacのSlack LaunchAgentを停止・自動起動無効化し、Pi上のsystemd窓口へ移行した。
Slack窓口とresidentはともにenabled/active。PiのOS起動時にログイン不要で起動する。
Macのスリープ・ログアウトを必要としない構成とし、実際のOS再起動やMacスリープは行っていない。

- Pi: `homelab_pi`、aarch64、既存node `pi` / `continuity`。
- 既存resident: `/srv/kamimusuhi/runtime/bin/kamimusuhi serve`。変更・再起動していない。
- 既存正本: `/srv/kamimusuhi/runtime/individual`。コピー・初期化・migrationは行わない。
- Slack binary: `/srv/kamimusuhi/runtime/bin/kamimusuhi-slack`。
- unit: `/etc/systemd/system/kamimusuhi-slack.service`。
- credentials: `/srv/kamimusuhi/config/slack.env`、所有者eightman、600。config directory 700。
- 操作者・チャンネル制限と`SLACK_OPERATOR_SUBJECT=eightman`を維持。
- Mac: `com.eightman.kamimusuhi-slack`をbootout、print-disabledで`disabled`を確認。
  Macの保存済み600設定・binaryは復旧用に保持するが窓口は起動しない。

## ビルド・配置

Slack crateだけの専用workspaceを
`/home/eightman/src/kamimusuhi-slack-20261108`に作成。
元のworkspace lockfileから不要なworkspace package項目だけを整理し、
全依存のversion/checksumが元lockfileと一致することを検証した。
既存resident/runtime成果物や他checkoutは変更しない。

- ソースarchive SHA256: `333cb5b0f248a696666423b7953f76a4a706b888e7a4b1be8932ed40e4038948`。
- Pi Rust `1.92.0`によるrelease build: PASS（6分04秒）。
- Pi native release tests: 8 PASS / 0 FAIL / 0 ignored。
- `install-slack-pi.sh --prepare-only`でunit/binaryを配置し、systemd unit verify: PASS。
- backup directory: `/srv/kamimusuhi/backups/slack-20261107T173104Z`。
  初回追加なので既存Slack binary/unitの置換対象はない。
- 配置binaryとビルド成果物のSHA256一致: PASS。
- binary SHA256: `93710a39bbbd56a3d78e950ec7f434050bd62a6a3373102925e7ddb97f387b51`。
- Mac停止後、Piのunitをenable --now。二重稼働を避けた。
- Pi窓口だけをSIGKILLし、自動復旧・Slack再接続を確認（PID 961475 → 961691）。
  復旧後も既存resident active。

## 実Slackと履歴の一貫性

Chromeの既存テストスレッドへ02:33:01に`@Mio`でテストを送信し、
02:33:26に`KAMIMUSUHI-SLACK-PI-20261108 OK`をMioから受信。
この間Mac窓口は停止済み。

[検証投稿](https://eightman.slack.com/archives/C0CFKMJA5TL/p1794072781570989?thread_ts=1794071550.966049&cid=C0CFKMJA5TL)
／[Pi窓口の返信](https://eightman.slack.com/archives/C0CFKMJA5TL/p1794072806459319?thread_ts=1794071550.966049&cid=C0CFKMJA5TL)

Piの`/v1/kamimusuhi/history`をsubject `eightman`で照会し、同じ検証入力と
完全一致する返信が保存されていることを確認した。
このAPIはデスクトップも使う履歴APIで、デスクトップ既定のnodesファイルはPiを指し、
既定subjectはMacユーザー名`eightman`。移行後のSlackと既定デスクトップは
同じ正本・同じ操作者の履歴・同じresidentの承認規則を使う。
デスクトップで`--local`や別の`--url`/`--subject`を指定した場合は別領域になる。

Piの設定済み推論tierは外部サービスを指し、Macに依存しない。
実返信は`hai` / `qwen3.8-27b-uncensored`、約23.8秒のroute記録を確認した。
個々の依頼でMac専用ツールを使う場合、そのツールはMac停止中には利用できない。

前回のMac窓口はMac側の別の対話領域を使っていたため、過去のMac側履歴は
今回のPi正本履歴へ自動統合していない。既存データを勝手に混合・削除しない。

## 復旧

Pi側Slackサービスを停止・無効化した後、Mac側をenable/bootstrapすれば旧経路へ戻せる。
同じアプリの窓口を両方で起動しない。既存個体DBやresidentを戻す操作は不要。
認証値は本記録・ソース・回答に含めない。
