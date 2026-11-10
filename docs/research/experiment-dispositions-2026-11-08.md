# 実験の本番化・凍結・棄却 (2026-11-08)

## 現在の結論

学習器官の本番承認は **0件**。H0/R0は本番評価候補、S0/T0は研究継続、O0は凍結。
全5件のruntime manifestをShadowへ変更し、設定からActiveへ上書きする経路を拒否する。
合成課題のPASS、原report・revision・checkpointの歴史は変更しない。

判定正本は [`knowledge/experiment-dispositions.json`](../../knowledge/experiment-dispositions.json)。
この文書の値は既存reportの監査結果であり、学習・統計の再計測結果ではない。
RustのFI2/FI3は既に本番配備済み。PythonのFI実験に結果がないことと混同しない。
Rust C0のproposal/replay/activation/rollback機構も、実モデルの自己改善能力が
実証されたという意味ではない。Python c0 smokeとは別に評価する。

## 現実装・実機との接続

- Mac設定にはK-COREがない。Pi設定のK-COREにはconfig/stdin指定がない。
- Pi実プロセスも `--config` なし、`--stdin` なし、stdin=`/dev/null` を読み取り確認。
  標準runnerは器官0件・perceptなしのidle tickであり、running表示は器官有効化を意味しない。
- residentのK-CORE出力はjournalへ行く。通常の `DialogueSession` は
  `run_organs_for_persona` を呼ばず、独立K-COREの信号を受け取らない。
- CX0のwrapは合成worldのPythonクラスであり、ProcessOrganのJSON実行プログラムではない。
  5/16次元観測や空間pointerから自然言語・実メモリへの意味対応は定義されていない。
- 元R0/CX0 checkpointはローカルにない。CX0のSlotMemoryは元の学習済みR0とは別機構。
  欠落時に自動学習するCX0 loaderを本番loaderとして使わない。

根拠: [resident launcher](../../crates/kamimusuhi-resident/src/kcore.rs)、
[K-CORE defaults](../../crates/kamimusuhi-runtime/src/kcore.rs)、
[dialogue path](../../crates/kamimusuhi-runtime/src/dialogue.rs)、
[CX0 modules](../../experiments/cx0/organs/modules.py)、
[CX0 loader](../../experiments/cx0/organs/pretrain.py)。

## 23研究ディレクトリの判断

直下の24ディレクトリのうち1個は `__pycache__`。研究対象は23個。
「候補」は評価を優先する意味であり、Active承認ではない。

| 系 | 判断 | 現在の根拠と次の関門 |
|---|---|---|
| C0-Python | 凍結 | smokeのみ。Rust C0機構と別系で、再現・実モデル評価未完。 結果を再現しRust継続性との差分を特定するまで追加投資を止める。 [evaluate.py](../../experiments/c0/evaluate.py) |
| CX0 | 棄却（方式限定） | 構造化皮質の優位性なし。generic GRUが最高。lesion手法は保持。 同じ構造化方式の本番延長を止め、lesion法だけを実益評価へ再利用。 [CX0_OVERNIGHT_REPORT.md](../../experiments/cx0/reports/CX0_OVERNIGHT_REPORT.md) |
| FI0 | 継続 | 実験コードは存在するが結果artifact未確認。 固定モデルの完全な評価結果を残す。 [evaluate.py](../../experiments/fi0/evaluate.py) |
| FI1 | 継続 | 障害・永続性実験の結果artifact未確認。Rust kernelとの重複がある。 既存Rust kernelとの差分を整理し未実証の障害だけ評価。 [repro.py](../../experiments/fi1/repro.py) |
| FI2-Python | 継続 | Python実験の結果artifactなし。Rust忠実性境界は別途配備済み。 Python実験結果とRust配備証拠を混同しない。 [integrity.py](../../experiments/fi2/analysis/integrity.py) |
| FI3-Python | 継続 | Python実験の結果artifactなし。Rust同一性ゲートは別途配備済み。 Python実験結果とRust配備証拠を混同しない。 [repro.py](../../experiments/fi3/repro.py) |
| G0 | 棄却（方式限定） | 既存方式はFAIL。untrained reservoir同程度。 負の結果と対照を保存し、同方式の延長を止める。 [G0_RESULTS.md](../../experiments/g0/reports/G0_RESULTS.md) |
| G0-v4 | 棄却（方式限定） | 既存predictive-invariant方式はFAIL。 負の結果を保存し本番対象から除外。 [G0_V4_RESULTS.md](../../experiments/g0_v4/reports/G0_V4_RESULTS.md) |
| G0-v5 | 棄却（方式限定） | seed0 pilotでFAIL・打切り。full5seed完走とは言わない。 同方式の延長を止め、pilotの限定結論を保存。 [G0_V5_REPORT.md](../../experiments/g0_v5/G0_V5_REPORT.md) |
| G0-v6-r2 | 継続 | 旧r1 STRONG_PASSはevaluator bugで無効。現r2結果なし。 r1を昇格根拠に使わず、修正済r2の凍結protocol結果だけ評価。 [README.md](../../experiments/g0_v6/README.md) / [EXPERIMENT_LINEAGE.md](../../experiments/g0_v6/EXPERIMENT_LINEAGE.md) |
| H0 | 本番評価候補（未承認） | 合成課題5/5 PASSだが一部OODはheuristicに劣り、2変数の寄与なし。 実入力対応・hash固定・実LLMとheuristic対照・因果的ON/OFFを満たす。 [H0_RESULTS.md](../../experiments/h0/reports/H0_RESULTS.md) |
| K0-E | 凍結 | 現存コードと歴史要約まで。集計成果未確認。 既存結果の再現・役割の特定まで追加学習を止める。 [README.md](../../experiments/k0_brainstem/README.md) / [history.md](../../docs/research/history.md) |
| K0-E2 | 凍結 | 全体FAIL。PPO退行・未知遅延転移FAIL。 現方式を本番へ接続しない。 [K0_E2_REPORT.md](../../experiments/k0_e2_active_info/artifacts/primary/K0_E2_REPORT.md) |
| K0-F2 | 凍結 | protocol/testsのみで結果なし。K0-Fの確証成功ではない。 確証結果が揃うまで採用しない。 [README.md](../../experiments/k0_f2_interoception_confirmatory/README.md) |
| K0-F | 凍結 | BLINDを上回る確証なし。probe失敗後の部分値は探索的診断。 現方式の本番接続を止める。 [K0_F_REPORT.md](../../experiments/k0_f_interoception/results/20260910/K0_F_REPORT.md) |
| MIOBA | 凍結 | 常駐実益なし。ただし後続A3系の因果graft効果・BODY0閉ループの成果はある。全体寄与0とはしない。 常駐ロードマップとは別研究として保持。graftの課題性能悪化を解消し用途を定義。 [A3_1_DROSOPHILA_CALIBRATION_REPORT.md](../../docs/experiments/A3_1_DROSOPHILA_CALIBRATION_REPORT.md) / [A3_2_PHYSIOLOGY_HARDENING_REPORT.md](../../docs/experiments/A3_2_PHYSIOLOGY_HARDENING_REPORT.md) / [BODY0_CLOSED_LOOP_REPORT.md](../../docs/experiments/BODY0_CLOSED_LOOP_REPORT.md) |
| O0 | 凍結 | strong heuristic corridordynが優位。 強い追跡baselineを事前規定条件で上回るまでShadow。 [O0_RESULTS.md](../../experiments/o0/reports/O0_RESULTS.md) |
| P0 | 棄却（方式限定） | 既存方式FAIL。vtriggerに劣りsurprise制御を獲得せず。 負の結果を保存し本番対象から除外。 [P0_RESULTS.md](../../experiments/p0/reports/P0_RESULTS.md) |
| R0 | 本番評価候補（未承認） | 合成memory gate strong PASS。実メモリ対応未定義・元checkpoint不在。CX0 SlotMemoryは別物。 元checkpointをhash固定し、実入力対応・強heuristic・ON/OFF/lesionを事前規定して測定。 [R0_RESULTS.md](../../experiments/r0/reports/R0_RESULTS.md) / [pretrain.py](../../experiments/cx0/organs/pretrain.py) |
| S0 | 継続 | 合成PASSだがaction delay=2で崩れ、常駐の遅延行動を支持しない。 遅延・counterfactual・別worldで再検証してから本番評価。 [S0_RESULTS.md](../../experiments/s0/reports/S0_RESULTS.md) |
| T0 | 継続 | 内挿PASS、外挿0.260・scaling FAIL。 時間外挿・scalingを解決してから本番評価。 [T0_RESULTS.md](../../experiments/t0/reports/T0_RESULTS.md) |
| U0 | 凍結 | learned-cleanでU-H1/U-H2未達。他gate全体を評価したとは言わない。 現形の本番接続を止め、先に2主閾値を再現して満たす。 [U0_COMPACT_RESULTS.md](../../experiments/u0/reports/U0_COMPACT_RESULTS.md) |
| X0 | 凍結 | smoke・環境検証のみ。timing heuristic優位が残る。 時間shortcutを除く対照と完全な結果が揃うまで凍結。 [env_validation.json](../../experiments/x0/artifacts/env_validation.json) |

## 監査による訂正

- G0-v6のSTRONG_PASSは **無効化された旧r1** の集計。evaluator bugで隔離され、
  現r2の結果はない。旧PASSを昇格根拠にしない。G0-v5もseed0 pilot打切りで、
  full5seed失敗という記述へ拡大しない。
- MIOBA全体の機能寄与を0とするのは誤り。M1.5の無寄与と、後続A3.1/A3.2の
  因果的graft効果、BODY0の閉ループ成立は別の結果。graftがorientingを悪化させた
  事実も保持する。常駐会話・記憶への実益はなく、常駐ロードマップから切り離して凍結する。
- CX0の構造化皮質は棄却するが、lesion・器官依存性の測定手法は再利用する。
  P0/G0の既存方式を棄却しても、予測やgroundingという研究概念全体の否定にはしない。
- [Persona](../persona/PERSONA_RESEARCH_REPORT.md) は23フォルダ外の継続研究。
  設計例文・採点者1名をモデル能力の実測と呼ばず、実モデル14シナリオと
  独立採点者2名以上の再評価を必要とする。

## 次の本番化関門

R0を先、H0を次の候補とするが、以下を満たすまでON/OFF実験を開始しない。

1. 元artifactの入手元・hash・loader版を固定し、欠落・改変時は停止する。
2. 実入力・実メモリと学習時入力の対応を定義する。任意のembeddingや乱数をtoy vectorへ
   押し込んで「接続した」としない。合成世界と違う意味の入力なら元モデルの移植とは言えない。
3. 通常のresident→DialogueSession→Personaまでの実経路を実装し、
   器官の状態が出所・同一性・訂正規則の権限を持たないことを確認する。
4. データ・モデル・サンプリング・閾値・呼出数/時間/token/RSS上限を事前固定する。
   OFF / ON / signal shuffle / lesion / 強heuristicを同一条件で比較する。
5. 主指標は正答と出所ID、訂正逆戻り、主体混入、根拠不在での拒否。
   token・p95 latency・RSS・残留子プロセスも測る。差のCIを示し、
   主体混入と訂正逆戻りは0。モデル採点だけで合格としない。
6. 隔離個体で評価し、本番個体の記憶やheadに実験用書込みをしない。

現在は(1)(2)(3)が未充足なので、実LLMを呼んでも元器官の寄与測定にならない。
このため今回は無意味なON/OFFや新規学習を行わず、昇格境界の是正を完了する。

既存Rustの宣言link/tag検索を評価する場合は、別の **runtime recall評価** として扱う。
同じ隔離runtimeでlexicalのみ/link-tag有効/shuffle/正解記憶lesionを比較できるが、
それはR0の成功とは呼ばない。現 `recall-eval` はread-only到達率だけで、
LLM応答・relevance判定の再生を含まないため、それだけで本番効果を主張しない。

## 検証・運用範囲

台帳と実験ディレクトリの完全一致、出典content hash、runtime manifestとの一致を
`cargo test -p kamimusuhi-runtime --test experiment_dispositions` で確認する。
出典が変わればhash照合が失敗し、判定の再監査を必要とする。
設定Active指定の拒否は子プロセス未起動・正典不変まで検証する。

研究データ・checkpoint・負の結果を削除せず、既存P0/P1/Slackを変更しない。
今回の昇格ポリシー変更はローカル実装まで。本番に新しい学習器官は追加・有効化しない。

実行結果（2026-11-08）: organs unit 7、kcore unit 9、kcore_loop 9、
台帳整合3の計 **28 PASS / 0 FAIL**。設定Active拒否時の子未起動・正典不変を含む。
Clippy（対象lib/2 integration tests、`-D warnings`）、fmt、diff checkもPASS。
実LLMの新規ON/OFF実験・本番設定変更・学習・commit/pushは行っていない。
