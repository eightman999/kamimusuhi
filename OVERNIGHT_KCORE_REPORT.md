# Overnight K-CORE Report

## Result

PASS — 実際に会話でき、長時間動かしても破綻しない最小 conversation loop が、
Mac上で再現可能な deterministic test 付きで存在する。

- workspace build: green
- 全テスト: green（workspace全体、0 failed）
- 1,000-turn seeded soak: PASS（2回実行、両方PASS）

## Branch

`feat/kcore-dialogue-loop-hardening`（master から分岐。main/master へ直接commitなし）

## Commits

- `aba9f19` fix(dialogue): restore merged LLM/Jev loop to a compiling, passing state
- `b4309c4` feat(dialogue): add race cancellation, Jev fail-soft, and turn observability
- `d33860e` test(dialogue): add deterministic provider arbitration suite
- `235efbb` test(dialogue): add seeded 1000-turn soak test
- `05a8f4b` feat(runtime): make trace rotation limit configurable
- `8122306` docs(dialogue): document fail-soft boundary, cancellation, and new trace fields
- （本reportのcommit）

## Implemented

- **Baseline repair**: master がcompileすら通らない状態を修正し、落ちていた
  統合testを緑に戻した（詳細は Bugs fixed）。
- **Deterministic fakes** (`llm_jev::testing`): scripted / seeded language provider
  （正常・最速不良・低速・timeout・error・empty・malformed・hang）と
  scripted / seeded decision provider。外部API不使用で全race条件を再現。
- **Cooperative cancellation**: raceごとに共有 `CancellationToken` を
  各器官request cloneへ注入。採用・枯渇・早期errorのいずれでも drop guard が
  cancelを発火。協調器官は blocking 境界で中断し、cancel不能な器官は自身の
  timeoutまで走るだけで結果は破棄される。
- **Late candidates**: race終了後に完了した試行を10ms上限でdrainし、
  `late_candidates` として記録。評価・配信の対象にはしない。
- **Jev fail-soft**: availability障害（timeout/transport/TLS/HTTP status/
  credential/invalid config）のみ、そのターンを rule-based gate へ degrade。
  `fallback=true` + 障害codeを結果へ付与し、`decision_fallbacks` に
  `stage:CODE` を記録。一度degradeしたターン内は残りのJev呼出しも local gate
  で処理し、同一dead endpointへの繰り返しtimeoutを避ける。次ターンは
  Jevを再試行する（per-turn回復）。
- **Fail-closed維持**: 応答が届いたが契約違反（malformed 1回修復後も、無効
  choice、未知候補、確率分布破綻、不適格選択、provider申告 `fallback=true`、
  明示 `REJECT`、発話前 `WAIT`／観測先なし `OBSERVE_MORE`）は従来どおり
  エラー。自動ACCEPT・primary自動選択で補わない。
- **Observability**: `ConversationTrace` に `total_turn_latency_ms`、
  `late_candidates`、`decision_fallbacks` を追加し、turn trace eventにも
  同内容を出力。内部cancelはprovider失敗としてtelemetry成功率を下げない。
- **`add_language_provider`**: in-process器官をoperator configを経由せず
  登録するtest用経路。race・telemetry・evidence bindingは正規経路と同一。
- **Trace rotation可変化**: `RuntimeOptions::trace_max_bytes`。`None` で
  従来の8 MiB/1世代、`Some(0)` でrotation無効（長時間runの完備性assert用）。

## Tests

- unit: `kamimusuhi-runtime` lib tests — 全pass（workspace全体も全pass）。
- integration `tests/llm_jev_loop.rs`: **19 passed** — fixture HTTP経由の
  preparation→race→匿名assessment。fail-soft変更に伴い2件更新、
  `unavailable_jev_degrades_to_the_local_gate_and_still_delivers` を追加。
- integration `tests/dialogue_arbitration.rs`: **11 passed** — 必須A–J相当:
  単一器官ACCEPT、fast-bad reject→slow-good採用、12器官race（固定上限なし）、
  最速timeout→failover、全器官失敗→controlled error→session回復、
  Jev outage→local degrade→会話継続、malformed Jev fail-closed、
  cancel後のthread/call leakなし、late完了の観測、Jev payloadに
  model/provider名なし、連続turn状態整合。
- soak `tests/dialogue_soak.rs`: **1 passed** — 下記参照。

## Soak

seed固定（`KAMIMUSUHI_SOAK_TURNS`で変更可、10,000対応）。
3つのseeded器官（latency 1–30ms変動、error/empty/oversized/hang混入）+
seeded Jev（retry 8% / reject 7% / outage 5%）。

    turns:             1,000
    replies / emitted: 934 / 934（emitはreplyごとに1回のみ）
    controlled_errors: 66（WAIT/REJECT verdict + all-fail turn、全て制御内）
    degraded_turns:    42（Jev outage → local gate）
    retries:           19
    late_attempts:     869（cancel / straggler 観測）
    provider_calls:    [970, 962, 964]（starvationなし）
    crashes:           0
    deadlocks:         0
    thread leak:       0（全器官のin-flight gaugeが終了時0）
    timeouts:          0（制御外timeoutなし。hang器官は内部400ms deadlineで終了）
    turn_latency_ms:   p50 62 / p95 109 / p99 180
    elapsed_ms:        69,327（2回目runは62,880）
    peak RSS:          before 8,368 KiB → after 12,736 KiB（+4.4 MiB）
    trace:             turn.started が全attempt turn分存在（rotation無効で完備）

再現: `cargo test -p kamimusuhi-runtime --test dialogue_soak`、
長runは `KAMIMUSUHI_SOAK_TURNS=10000 cargo test -p kamimusuhi-runtime --test dialogue_soak -- --nocapture`。

## Architecture observations

- raceは「最初の有効完了 + 10ms grace内の同着1件」までを評価batchにし、
  Jevへの候補は常に `candidate-N` 匿名ID・最大2件。provider数に上限なし。
- RETRYは即再生成せず未完了器官の次batchを先に判定し、全器官枯渇後のみ
  最初のRETRY候補を1回修復。2回目のRETRYは上限error。
- 修復再評価は採用候補だけでなく生成済み全候補を対象にする（本件で修正した
  regressionの趣旨そのもの）。
- degradeはstage単位で記録される（`prepare_turn` / `assess_responses`）。
  ターン途中でdegradeしても残りのJev呼出しはlocal gateへ直行する。
- RSS +4.4 MiB / 1,000 turns は会話履歴・evidenceの蓄積とallocator arenaの
  温まりが主因と見られる。器官thread gaugeは0へ戻り、race経路自体の
  リークではない。履歴を無限に保持するsession設計の帰結であり、
  長期運用では履歴管理方針が別途必要。

## Bugs found

- master HEAD の `kamimusuhi-runtime` lib がcompile不能だった
  （merge `564b361` がrebase後に未compileのまま入ったと見られる）。
  1. `dialogue.rs`: provider mapが `Arc<dyn LanguageProvider>` と推論される
     箇所へ `Box<dyn LanguageProvider>` をinsertする型不一致。
  2. `llm_jev.rs` test: `""fixture""` / `""mock""` 形式の不正文字列literal。
  3. retry修復時に `language_results = vec![final_result]` と採用候補のみを
     残すため、修復re-assessmentが他候補IDを参照したfixture応答と不一致に
     なり、意図されたRETRY_AFTER_LIMIT経路が成立しなかった。
     （compile不能だったため、このregressionを検出するtest自体が
     master上で一度も走っていなかった。）
- soak test の trace完備assertは既定8 MiB/1世代rotationの下では
  10,000 turn構成で早期eventが正規に捨てられてspurious failし得た
  （`trace_max_bytes` 追加で解消）。

## Bugs fixed

- 上記 compile error 2件と candidate-pool regression 1件（`aba9f19`）。
- race終了後の器官完了を無観測でdropしていた問題 → `late_candidates` で記録。
- 内部cancelをprovider失敗としてtelemetryへ計上していた問題 → 除外。
- Jev障害が即turn全体のerrorとなり conversation が継続不能だった問題 →
  availability障害のみ per-turn degrade。

## Remaining risks

- 実API（HAI/TypeSafe Jev）でのsmokeはcredential非搭載環境のため未実施。
  mock/fixtureでの契約検証のみ。実モデルの応答品質は未評価。
- cancel不能なblocking HTTP器官は自身のtimeoutまで走り続ける
  （協調cancelはhintであり強制killではない）。drainは10msで打ち切るため
  その完了結果は観測されず破棄される。
- token usage / tok/s はtransportが返さないため未計測（spec上optional）。
- `j72`・学習系・tokenizerは今回のscope外で未変更。
- desktop crate の rustfmt drift はpre-existingとして未touch
  （無関係のformat churnをcommitへ混ぜないため）。
- soak 1,000 turn のRSS増分はboundedだが、10,000 turn規模での
  履歴・evidence蓄積プロファイルは未計測。

## Recommended next task

1. `KAMIMUSUHI_SOAK_TURNS=10000` の長時間runを1回実行し、RSS・latency・
   rotation設定の実測を取る（完備性assertは `trace_max_bytes=Some(0)` で
   既に対応済み）。
2. P1 replay: 記録済みtraceからconversation decisionを再現する簡易harness。
3. P2 benchmark: arbitration overhead の p50/p95/p99・accept/retry/fallback率を
   定型計測として残す（soakが既に同種の値を出すので形式化のみ）。
4. credentialが用意できた時点で実API smokeを数turnだけ実行し、
   wire contractの実endpoint互換を確認する（失敗してもmock結果は無効化しない）。
