# Providerの子プロセス隔離・強制回収（Issue #58）

## 通常経路

`DialogueSession::start` は判断Provider、設定から作るprimary/追加言語Providerを
`kamimusuhi-runtime provider-worker` のone-shot子プロセスで実行する。
`set_language_provider` で動的追加する設定も同じ経路を使う。
workerは個体DBを開かず、親が組み立てた型付き入力だけを受け取る。
結果の同一性・根拠・匿名性・canonical書込みの権限は親が引き続き検証する。

workerは呼出しごとに終了する。内部に非協調thread・メモリを持っていても、
期限切れ・キャンセル・異常応答ではプロセスグループへSIGKILLを送り、直接の子をwaitして
回収した後で実行枠を解放する。正常終了時も残った同一グループの子孫を終了する。
64回以上の永久停止でも隔離枠が残り続けることはない。

- 既定stage期限は120秒。`runtime.json` の `provider_timeout_ms` は1〜600000ms。
  キーのない既存configは120000ms。埋込みでは`set_provider_timeout(Duration)`でも設定可能。
- 期限は各stageに適用する。言語raceには共通期限があり、判断・repairも独立して有限。
- 匿名候補、最大2件batch、first usable、reject後の次batch、最大1回repairを維持する。
  判断Providerの停止は既存fail-soft、返ってきた不正な判定はfail-closed。
- 正常候補が勝った場合も、残りのworkerをキャンセルしてsupervisorをjoinする。
  エラーによる早期returnでも同じRAII cleanupが動く。
- 子プロセス数・host supervisor数はそれぞれ全session共通で最大64、待ちqueueなし。
  一時的な容量超過は有限失敗するが、終了・回収後は同じ枠を再利用できる。
- 親のstdin/stdout側は非blocking Unix socket。未読stdin、無応答stdout、
  子孫のdescriptor保持でもI/O補助threadや無期限joinを作らない。
  request/replyは各1MiB上限。出力洪水は拒否して回収する。
- request・資格情報をargvへ入れない。資格情報は従来のauth_envで参照する。
  stderrは親へ転送せず、workerエラーは閉じた固定enumに変換する。
  privateなPersona入力はローカルIPCにのみ追加し、既存の外部API用JSONへは追加しない。
- Decisionの設定は親の構築時に固定し、子で環境を再評価してprivacy区分を変えない。

## 導入と信頼境界

Unix（macOS/Linux）対応。`kamimusuhi-runtime`自身、またはhostの隣にある同バージョンの
`kamimusuhi-runtime`をworkerとして使う。test executableの`deps`配下からは親directoryも探索する。
別配置なら`KAMIMUSUHI_PROVIDER_WORKER`に実在する絶対実行パスを設定する。
workerがない場合に黙ってin-processへ戻ることはない。
resident導入は既存のruntime配置を利用し、macOS appビルドもworker sidecarを同梱する。

任意のRust trait objectを別プロセスへコピーすることはできない。
`start_with_providers`/`start_with_decision_provider`/`add_language_provider`の明示的な
in-process注入は、既存fixture・信頼された埋込みコードとの互換用である。
その場合だけ旧Boundaryの上限付きthread隔離が残り、非協調threadの強制回収保証はない。
独自の非協調Providerは`ProcessCommand`＋`ProcessLanguageProvider`/`ProcessDecisionProvider`で
exec可能なプログラムとして登録する。`is_process_isolated=true`はこの有限supervisor契約を表し、
任意コードの自己申告を検証するセキュリティ機構ではない。

これはプロセス寿命の隔離であり、OS権限sandbox・CPU/メモリ量の資源制限ではない。
同一グループで動く通常の非協調userspace処理を回収する。意図的なsetsid等のグループ脱出や
OSの非割込みカーネル停止は保証対象外。外部API・別サービスに渡した仕事のキャンセルも別契約。

## 故障試験

`tests/provider_hard_timeout.rs` は自己期限なし・SIGTERM無視のexec fixtureを使う。
親testプロセスにも独立期限とPID cleanupを設け、回帰時に試験全体をハングさせない。

- 96回（64枠超）の永久停止→SIGKILL/wait→正常呼出し成功。毎回枠が0へ戻ること。
- 全候補停止の有限失敗を96回繰り返し、その後も同じsessionで正常候補が成功すること。
- 正常候補と永久停止候補のraceを96回繰り返し、応答と回収が継続すること。
- 判断prepare/assess・repair停止、stdin未読、子孫によるstdout保持、出力洪水、cancel。
- PIDの消滅/非稼働、OS thread数・RSSの安定を実測。RSSは16MiBの許容幅で検査する。
  OS計測が拒否された環境で黙ってPASSにはしない。

```sh
cargo test -p kamimusuhi-runtime --test provider_hard_timeout -- --nocapture
cargo test -p kamimusuhi-runtime --test dialogue_arbitration --test llm_jev_loop
cargo test -p kamimusuhi-runtime --test dialogue_soak -- --ignored --nocapture
RUST_TEST_THREADS=1 CARGO_NET_OFFLINE=true bash scripts/ci-local.sh
```

fixtureによる実プロセス故障試験であり、実API・本番配備の検証ではない。
