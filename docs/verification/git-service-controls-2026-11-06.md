# Git操作・サービス自己再起動の稼働検証（2026-11-06）

## 修正内容と原因

本人が直接使えるGitとサービス制御toolが無く、Pi配置用ソースも `.git` を持っていなかった。
登録repoを扱う `git` と、登録serviceだけを扱う `service_control` を追加した。
Piには元のGit履歴と既存originを持つ `/home/eightman/src/kamimusuhi-git` を作成し、
現在のコード変更を未コミットで反映。元のPiソースとMac開発checkoutの編集は保全した。

Pi residentは `NoNewPrivileges=yes` のため内部sudoを利用せず、既存の
`Restart=always` / `RestartUSec=5s` による自己再起動を利用する。
受付応答後にticketをsync保存して終了し、次の起動番号で復帰を観測する。
Macは既存launchdの登録targetへkickstartする。マシン本体のrebootは行っていない。

## 検証

- ローカル221テスト（217単体＋4統合）、Clippy全target、書式チェックに成功。
- Piネイティブreleaseビルド成功（5分28秒、既存cacheを利用）。
- 両ノードの実repoでstatusと、既存originへの実fetchが成功。
- 両ノードの使い捨てrepoでadd/commit/diff/branch/checkout/logを実測。
- 明示fileだけのcommitで、別fileのstaged変更を保全。
- local bare originへのpush/fetch/ff-only pullが成功。実remoteへのpushは未実行。
- dirty checkout、認証fileのcommit、force/reset、未登録repo/serviceを拒否。
- 再起動検証後、検証repoの登録は削除済み。

## ツール自身からの最終再起動

- Mac：boot_epoch 43 → 44、ticket `519377dd70df04d533b7b994d8ae8af4`
- Pi：boot_epoch 59 → 60、ticket `49a55ff54e4ad1ec2d9080513e1141ea`
- 両方で `observed_restart` と再送重複抑止、再起動後Git利用を確認。
- PiのNoNewPrivilegesはyesのまま。権限保護・sudoers・認証情報は変更していない。
- Pi residentと既存AgentHub Mio workerはactive、AgentHub本人認証も維持。

Macの私有運用cloneは既存自己改善経路を保つため、今回のコードsnapshotへ更新した。
Mac開発checkout・Piの実repoでcommit/pushは行っていない。fixtureのcommit/pushと、
私有運用cloneのsnapshot commitだけが今回のGit書き込み検証・運用更新である。
