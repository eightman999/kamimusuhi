# Issue #58 Provider停止封じ込めの検証（2026-11-08）

> この記録は子プロセス化前のthread隔離段階。現行の強制回収は
> [子プロセス版の検証](issue58-provider-process-2026-11-08.md) を参照。

## 対象

HEAD `352a6e8a4c7feaea84229a2f6b54c3d54fc41f5d` と既存の未コミット変更に、
`provider_boundary.rs`、対話呼出しの隔離、config期限、回帰テストを追加したローカルソース。
既存の他変更は保全した。コミット・push・本番配備・認証変更は行っていない。

## 実装と保証範囲

[停止封じ込め仕様](../provider-fault-containment.md) を参照。
同期traitにcaller側期限、Provider単位single-flight、全session共通64枠を設けた。
非協調コードは安全に強制終了できないため、永久停止枠は実処理が戻るかホストが終了するまで保持する。
待ち行列やturnごとの無制限再spawnは作らない。
任意コード内の無限allocate・独自thread増殖を防ぐOS sandboxではない。

## 最終ソースでの検証

`RUST_TEST_THREADS=1 CARGO_NET_OFFLINE=true bash scripts/ci-local.sh`：
**982 passed / 0 failed / 6 ignored**。書式、全workspace・全target・全featureのClippy、
全workspaceテスト・doctest、core依存境界すべて成功。

永久停止テストの親5件はすべてPASS。ignored helperは親が独立子プロセスとして起動し、
永久停止時にも親の120秒期限で回収する。通常実行でhelper単独を起動しないためのignoreであり、
5シナリオのスキップではない。

| 条件 | 検証 |
|---|---|
| 非協調1つ＋正常候補 | 正常応答成功。warmup後1000turn、停止Provider呼出しは1回だけ |
| 全候補非協調 | 80msのhost期限を設定し、初回2秒以内のTimeout。1000turn反復でも停止呼出し2回だけ |
| session再作成 | 64枠飽和後も1000回のSTARTまたはTURNが有限失敗。実Provider呼出し64回のまま |
| 判断prepare/assess停止 | 各Providerを1回だけ実行し、以降はlocal gateへ退避して応答 |
| repair停止 | 修復呼出しも有限Timeout、古い候補を配信しない |

上の反復3シナリオでOSスレッド数・RSSを測定し、増加スレッド1以内・RSS増加16MiB以内を検査した。
スレッド内Providerには自己deadlineも協調キャンセルもない。
別途、panic本文の抑制、既存panic hook転送、busy共有、late completion破棄、正常復帰を単体検証した。
既存の匿名性、batch/race、bounded repair、fail-soft/fail-closed回帰も公式CIに含む。

`CARGO_NET_OFFLINE=true cargo test -p kamimusuhi-runtime --test dialogue_soak -- --ignored --nocapture`：
**1000turn PASS**。936応答、64件の制御されたConversation失敗、936回配信、23回repair。
経過70049ms。RSSは9152→13952KiB。turn latency p50=63ms、p95=90ms、p99=469ms。
これは既存の協調fixtureの長期回帰であり、上記の真の永久停止試験とは区別する。

## 環境と未検証

macOSのsandbox内ではloopback bindおよび`ps`のOS計測が拒否された。
テストを弱めず、同じローカル試験を制限外で実行して成功を確認した。

実API呼出し、本番residentへの配備・再起動、OS別の長期RSS挙動は未検証。
64永久隔離時のホスト自動ローテーションや、exec可能なProviderプロトコル＋OS資源制限は未実装。
したがって「任意Providerを必ずkill/reapし、任意内部allocationも制限する」保証ではない。
