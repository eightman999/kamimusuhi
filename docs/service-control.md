# 登録サービスの状態確認と再起動

`service_control` は、このノードの操作者が登録したサービスだけを扱います。
任意のshell、コマンド、PID、unit名、OSの再起動は受け付けません。

resident設定の例（既存 `tools` の項目を保持して追加）:

```json
{
  "tools": {
    "services": [
      {
        "name": "resident",
        "manager": "launchd",
        "unit": "gui/501/com.eightman.kamimusuhi-resident",
        "self_service": true
      }
    ]
  }
}
```

Piのresidentには、上のentryを次の形にします。

```json
{
  "name": "resident",
  "manager": "systemd_system",
  "unit": "kamimusuhi-resident.service",
  "self_service": true,
  "restart_via_exit": true
}
```

Piのresidentは既存unitの `NoNewPrivileges=yes` により内部sudoを使えません。
`restart_via_exit:true` は `self_service:true` だけに許可するopt-inで、
再起動受付の後に `awaiting_supervisor` を同期保存してresidentが正常終了します。
既存unitの **`Restart=always`**（または同等の常時再起動するsupervisor）が前提です。
Piでは既存の5秒待機後の再起動を利用し、NoNewPrivilegesやsudoersを変更しません。
この設定を、正常終了を再起動しないsupervisorへ使うとサービスは停止したままになります。

他のsystemd system serviceを既存権限で扱う場合に限り、`privileged:true` は
`sudo -n /usr/bin/systemctl restart <unit>` を選択します。これは既存sudo権限と
NoNewPrivileges等の制約に従い、権限設定の変更やパスワード入力は行いません。
systemd user serviceには `systemd_user` を指定します。
個体のruntime設定で `tools.allowed` を限定している場合は `service_control` を追加します。

```json
{"name":"service_control","arguments":{"action":"list"}}
{"name":"service_control","arguments":{"action":"status","name":"resident"}}
{"name":"service_control","arguments":{"action":"restart","name":"resident"}}
```

再起動の返事は `accepted:true` と `request_id` です。約1秒後に固定のnative
service managerへ要求し、その要求の終了には60秒の上限を設けます。
返事の受付成功は、サービス起動の完了を意味しません。
`restart_via_exit` ではmanager commandの実行を経ず、保存後の正常終了をsupervisorへ通知します。

自身のresidentを再起動すると、現在の会話や接続も中断されることがあります。
接続し直して同じIDを確認してください。`restart` に同じ `request_id` を指定した
再送は保存済みの受付を返し、再起動を繰り返しません。

```json
{"name":"service_control","arguments":{"action":"request_status","request_id":"返された32文字のID"}}
{"name":"service_control","arguments":{"action":"status","name":"resident"}}
```

`command_succeeded` はmanager commandの正常終了です。自己再起動で実行中プロセスが
終了すると、その終了コードを書き残せない場合があります。後続のresident起動で
`boot_epoch` が進んだ場合は `observed_restart` として別に記録します。
これは後続bootを観測した事実であり、manager commandの終了コード0を主張しません。
実際の稼働確認には `status` とノードの `/health` を利用します。

受付記録は `<paths.root>/current_state/service-control/` に保存し、監査イベントを
`logs/service-control` へ記録します。managerのEnvironmentや生のstderr、再起動commandの
stdoutをモデルやjournalへ返しません。状態確認はstate/PIDなどの小さいprojectionだけです。
ホスト本体のrebootや任意プロセスのkill、サービス登録は本ツールの対象外です。

moduleテストは固定argv/不正unit拒否、環境値を除外した状態projection、privateなticketの保存、
fake runnerの成功/失敗、ID再送とboot観測、supervisorへのhandoffを終了前に保存することを確認します。実サービスの再起動は別途の
稼働検証記録に従って確認してください。

2026-11-06にMac/Piへ反映し、両サービスのツール経由自己再起動と起動番号増加を確認済みです。
[稼働検証記録](verification/git-service-controls-2026-11-06.md)を参照してください。
