# 未解決の制約

承認や環境の不足があっても、独立して進められる候補へ移る。

| ID | 項目 | 理由 | ユーザーが行う具体的な操作 | 現在の扱い |
|---|---|---|---|---|
| B001 | 指定文書 `docs/primary-agent.md` | 起点masterに存在しない | この名称の資料が別ブランチにある場合、そのブランチへ後で追加または場所を記録する | 他の設計文書と実装から把握して続行。回答待ちではない |
| B002 | 24時間の単一セッション継続 | コンテナ/タスクの最大寿命は保証されていない | 中断された場合、実験ブランチをcloneして同じ依頼で再開する | STATEの実測時間を引継ぎ、未達を完了扱いしない |

## 解決済み

- cwdにcloneが無い: 公開repoを認可済みの経路で取得し、実験ブランチを作成した。
- Rust未導入: repo指定の1.92.0を導入した。
- imozuru未導入: 指定URLから導入しSKILL.mdを読んだ。
- `git push`のcredential無し: 既存GitHub connectorのGit Data APIで保存。新規credentialは不要。tree一致を確認し、force更新を使わずrefを同期する。

## 切り分け中

- 初回baseline CIはclippy段階でexit101だが、保持ログに診断本文が無い。元HEADの別worktreeで再実行し、ソースの既存失敗か環境要因かを確認する。現時点でユーザー操作は不要。

## 次回の環境で確認する事項

| ID | 項目 | 観測した理由 | ユーザーが行う具体的な操作 | 続行可能な作業 |
|---|---|---|---|---|
| B003 | imozuru原文の直接保存 | `fetch.py` が7URLでDNS解決失敗。安全設定は解除していない | 直接DNSと公開HTTPSが利用できる作業環境で同じURLを取得する。`IMOZURU_ALLOW_PRIVATE` は不要 | fixture試験、認可済みWeb取得の本文抽出、URL/節名の整理 |
| B004 | RustXNGの検索結果取得 | healthとJSONは200だが不応答engine9、結果0 | 公開検索エンジンへ到達できる環境で同じqueryを実行する | HTTP契約・sidecar lifecycle・schemaをモックで実装 |
| B005（新環境では解消） | 旧環境のfull CI | 新環境のcycle13凍結版で既存attached-server試験を含む全CIがexit0。旧exit137の原因は未確定 | 現在の操作不要。旧環境を再利用する場合のみPID namespace対応を確認する | 全workspace検証済みの基点から継続 |

### B005の原因確認（2026-11-09）

隔離実行で既存attached-server試験の孤児置換だけがFAIL（exit101）。このコンテナはgetpidと/procのPIDの視点が異なり、`ps -p child.id()` が空になる。起動・再利用はPASS。PID namespaceに対応する/procをmountしたコンテナ、または通常のLinux/Mac環境で同じ試験を実行する。今回executor.rsを変更してこの環境を迂回することはしない。

### cycle8凍結版の全CI（2026-11-09）

公開commit `980c564579faa703cf55651c92de3e4e0495f62c` と同じtreeの別worktreeで、build jobs1・incremental無効・offlineで再実行した。fmt、全workspace clippy、provider worker buildはPASS。resident unit269件と実HTTP2件もPASSした。runtimeの既存attached-server FAILの後も試験は進んだが、wrapperがexit137で終了し、最終JSON報告は作成されなかった。終了理由は未確定で、OOMと断定しない。後段gateは未確認。次回はtest threadsも1として終了不能を切り分ける。新しいnative agentの対象検証は継続可能。


### B006: コンテナ再作成に伴う未転送分の消失（2026-11-10）

cycle10までのremote commitは保存済み。cycle11のlocal commitとcycle12〜14の未保存作業、一時ログ、toolchainは現在の環境から失われた。rootが再clone・機能復元・再検証を実施するため、現時点でユーザー操作は不要。再び中断した場合は実験ブランチのSTATEから再開する。旧CIの最終結果は復元できないため、新環境で実行し直す。

### 2026-11-10の検証更新

B005: cycle13凍結版（公開83dd2d3、tree8d830c1）の全CIがPASS、989.095s。既存attached-server試験もPASS。元masterそのものの全CI成功や旧exit137原因の特定とは区別する。B006: cycle11機能とcycle12/13を復元・再検証し公開保存済み。cycle14は新fixtureを実測中。現在ユーザー操作は不要。

### B007（解消）: CI targetの/tmp容量不足

11ef050は/tmp tmpfs99%時にlinker Bus errorでFAIL。空のoverlay targetで同treeの全CIがPASS（1089.798s）。数GiBのbuild targetは/workspace側に置く。ユーザー操作不要。

### B008: 二度目のcontainer再作成（2026-11-10）

元containerのcycle20実装・検証後、commit起動前にexec-server接続断。未転送source/log/toolchainが消失した。09:48 UTCから新containerへ再cloneし、rootが実patchから復元・再検証する。現時点でユーザー操作不要。再び停止した場合は実験branchのSTATE/BACKLOGから再開する。新しい鍵、本番resident接続、新規有料サービスは不要。

### B009（復帰）: 復元後の一時的な環境unavailable

09:56の再検証開始後に実行環境がunavailableとなり、結果を一度UNKNOWNとした。14:31:52の復帰でworkspaceのsource/target/最終log/JSONは残っており、全対象gateのPASSと元fixture hashを確認した。HOME側のRust/cacheだけが消失したため、workspace側の専用CARGO_HOME/RUSTUP_HOMEへ再導入する。現時点でユーザー操作不要。再発時は利用可能containerから実験branchのSTATEを読み、ログが保持されたかを先に確かめる。元file消失や実行結果を推測せず、回収できない場合だけUNKNOWNとする。
