# Issue #58 子プロセスによる強制終了・回収（2026-11-08）

## 変更

前段の[thread隔離の記録](issue58-provider-containment-2026-11-08.md)で残っていた
「永久停止枠を回収できず、64枠で枯渇する」制約を通常の設定経路から取り除いた。
[現行仕様](../provider-fault-containment.md)を参照。

- 設定由来のprimary/追加言語Provider・判断Providerをexec workerへ移した。
- nonblocking Unix socketでstdin/stdoutを扱い、I/O補助threadなしで期限とcancelを監視。
- timeout/cancel/異常終了でグループSIGKILL、直接childのwait完了後に枠を解放。
- raceの勝者決定後・例外returnでも残るprocess supervisorをcancel/joinする。
- repair・判断も同じ期限付きexec境界を通す。
- private Persona入力はローカルIPCのみ。外部API用JSONは従来どおり。
  エラーの生本文は固定enumへ変換し、child stderrも転送しない。
- 通常CLIのfactory経路とmacOS appのworker sidecar同梱を接続した。

元のHEADは`352a6e8a4c7feaea84229a2f6b54c3d54fc41f5d`。既存の未コミット変更を保全し、
今回もコミット・push・本番配備・認証変更は行っていない。

## 実行検証

実fixtureは自己期限なし・SIGTERM無視。テスト親にも独立した期限とcleanupを置いた。

| 試験 | 確認 |
|---|---|
| 96回の永久停止＋毎回正常probe | 64枠を超えても実際に新規execし、毎回kill/reap後の枠数0。正常probe継続 |
| 2候補がともに永久停止×96turn | 各turnが設定期限で有限Timeout。回収後、同sessionへ加えた正常候補が16turn成功 |
| 正常候補＋永久停止候補×96turn | 毎回正常候補が勝ち、停止側も回収。枠0へ復帰 |
| 判断prepare/assess停止 | 子を回収し、既存local gateへ退避。正常言語Providerの応答継続 |
| repair停止 | 有限Timeoutと回収。古い候補を配信しない |
| stdin未読・出力洪水・子孫のstdout保持・cancel | 有限失敗/応答とグループ終了、枠の復帰 |
| 通常CLI init→talk | prepare/language/assessすべて実execをaudit。Persona入力到達と非空mock応答 |

PID生存状態、thread数、RSSをOSから取得した。計測不能を黙ってPASSにはしていない。
macOS sandboxの`ps`・loopback制限があるため、同じローカル試験を制限外で実行した。

実測の詳細と公式CI最終集計は末尾に記す。

## 1000turn soak

最終ソースで既存の`dialogue_soak --ignored --nocapture`を別実行してPASS。
1000turn中929応答、71件の制御されたConversation失敗、929回配信、16回repair。
経過69700ms、RSS9248→14112KiB、turn latency p50=64ms/p95=100ms/p99=228ms。
これは協調fixtureの長期回帰であり、上記の非協調exec試験とは分けて解釈する。

## 検証範囲

ローカルmacOSで実プロセスのkill/wait・枠復帰・PID状態を確認した。
実API、本番ノードへの配備、Linux実機、release版macOS appの起動/署名検証は未実施。
appビルドスクリプトは`bash -n`成功、runtime/desktopの型検査・テストは公式CIの対象。

信頼されたfixture/埋込み向けの明示的なin-process注入APIは互換用に残す。
任意trait object自体を別プロセスへコピーできないため、独自の非協調Providerは
ProcessCommandとprocess adapterでexecする。通常の設定経路にin-process fallbackはない。
OSの資源sandbox、意図的なprocess group脱出、非割込みkernel停止は保証対象外。

## 公式CI・実測結果

`RUST_TEST_THREADS=1 RUST_TEST_NOCAPTURE=1 CARGO_NET_OFFLINE=true bash scripts/ci-local.sh`：
**991 passed / 0 failed / 6 ignored**。fmt、全workspace全target全featureのClippy、
workspaceテスト/doctest、core依存境界すべて成功。
ignoredのprocess helperは親5テストから独立子プロセスとして実行済み。

公式CIでのOS実測：

| 96回反復の条件 | thread数 | RSS（KiB） | 回収後の実行枠 |
|---|---:|---:|---:|
| 永久停止＋毎回正常probe | 2→2 | 4016→4528 | 0 |
| 正常候補＋永久停止候補 | 2→2 | 11040→13968 | 0 |

全停止試験は2候補を同時に永久停止させ、各回2秒以内のTimeoutを検査した（設定250ms）。
回収後の正常候補16turn成功も確認した。

最後に直接workerのゾンビ残留を明示的に拒否するようPID検査を強化した。
その過程で、Python起動前に期限で終了した試行をexec監査ログの欠損として扱う、
正常fixtureにも短い故障期限を流用する、という試験上の前提を修正した。
全停止の故障検出250ms・transport故障100msは維持し、正常継続確認は5秒へ分離した。
SIGTERM無視の停止処理へ実際に到達した回数を別途記録し、64枠を超えたことを要求する。
生産コードは公式CIから変更していない。強化後の最終結果は下記に追記する。

強化後の最終実行：`provider_hard_timeout --nocapture` は **5 passed / 0 failed / 1 helper ignored**。
runtime全target・全feature Clippyも成功。

| シナリオ | 停止本体への実到達（64超を要求） | thread数 | RSS（KiB） | 回収後の枠 |
|---|---:|---:|---:|---:|
| 全停止96turn＋正常追加16turn | 224 | — | — | 毎回0 |
| 正常候補との混合race（warmup4＋96turn） | 100 | 2→2 | 10784→13344 | 毎回0 |
| 単独停止＋正常probe（warmup4＋96試行） | 99 | 2→2 | 4272→4160 | 毎回0 |

単独試験の100試行中1試行は停止本体に入る前に期限終了した。
それを永久停止実行と数えず、実際の99回が64枠を超えることを確認した。
直接workerのPIDはゾンビも含めて消滅を要求し成功。同一グループの孤児子孫だけは、
initのreap待ちの非稼働状態を区別した。
