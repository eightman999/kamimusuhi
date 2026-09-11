# MIOBA 実機CUDA検証 — RTX 3060 12GB + Tesla P100 16GB

対象 PR: #21 (`devin/1789059290-mioba-infra`)
検証 commit: `a9dabca9423cea83a4c9cd361798f99ae3b68949`
対象機: `llm_master_now` = `llm-machine` / hostname `master` (Ubuntu 26.04, x86_64)
実施日: 2026-09-11 (JST)
本番進化 run は開始していない。
PR #21 は本検証の判定を受けて merge 済み（merge commit `7dfea42`）。

**benchmark 数値の扱い**: 本文の bench / backend smoke はすべて llama-server 停止後の
clean 計測。初回計測は llama-server が両 GPU に常駐した状態（3060 に 3,502 MiB、
P100 に 4,412 MiB、util 100%）で、時間・VRAM・OOM が汚染されている。汚染版は
`*_dirty.json` として保存してあり、**llama-server 競合時の参考値**としてのみ扱う。
性能判定・selected batch の根拠には使わない。

## 判定

| | 判定 |
|---|---|
| RTX 3060 (CC 8.6) | **PASS** |
| Tesla P100 (CC 6.0) | **PASS** |
| **PR #21** | **MERGE OK**（下記の不具合はいずれも科学的正しさに影響しない） |

## 実行環境

```
GPU 0  NVIDIA GeForce RTX 3060     CC 8.6  12,288 MiB  UUID 1d6afaad-67b7-b965-46c7-efe6c9c395f3
GPU 1  Tesla P100-PCIE-16GB        CC 6.0  16,384 MiB  UUID d11b6f7f-002d-b252-1921-149ed2d84f15
driver 580.173.02 / CUDA runtime 12.6 / torch 2.9.1+cu126 / numpy 2.2.6 / Python 3.12.13
```

P100 (sm_60) が cu126 wheel で実際に動作することを確認。`torch.cuda.is_available()` は両 GPU で True。

**計測条件に関する重要事項**: 初回計測時、両 GPU に `llama-server` が常駐しており
(3060 に 3,502 MiB、P100 に 4,412 MiB、計測中 util 100%)、bench と backend smoke の
時間・VRAM・OOM が汚染されていた。llama-server 停止後に **bench と backend smoke を
全て再計測**。本書の数値は全て clean 環境のもの。汚染版は `*_dirty.json` として保存。

## 1. 20k / 139k-14M backend smoke 結果

両 GPU・両スケールで **全項目 PASS**。

| | 3060 (cuda:0) | P100 (cuda:1) |
|---|---|---|
| **20k** n / nnz | 20,000 / 3,979,726 | 同一 |
| mean_rate_hz | 36.981998443603516 | **36.981998443603516** |
| VRAM | 53,038,080 B | 同一 |
| wall_s | 1.433 | 1.956 |
| sparse layout | `torch.sparse_csr` | `torch.sparse_csr` |
| **139k/14M** n / nnz | 139,000 / 13,994,841 | 同一 |
| dataset id | `v0-n139000-e14000000` | 同一 |
| mean_rate_hz | 36.65082550048828 | **36.65082550048828** |
| VRAM | 201,134,080 B | 同一 |
| wall_s | 4.898 | 6.639 |

各スケールで `device_recognized` / `synthetic_init_run` / `artificial_organ_run`(sparse path) /
`checkpoint_restore_replay` / `same_seed_deterministic` / `execution_batch_invariance` の
6 項目すべて PASS。execution batch 1/2/4 は `identical_across_execution_batches: true`。

spike 数は **GPU をまたいで完全一致**:
20k `[74137, 73791, 73709, 74358]`、139k `[510159, 508734, 509089, 509615]`（3060 = P100）。

## 2. selected batch / throughput / VRAM / OOM

### 20k (candidates 1,2,4,8,16,32)

| batch | 3060 sim-s/wall-s | 3060 thr | P100 sim-s/wall-s | P100 thr | VRAM reserved |
|---|---|---|---|---|---|
| 1 | 0.3981 | 0.398 | 0.3410 | 0.341 | 70 MiB |
| 2 | 0.0736 | 0.147 | 0.0531 | 0.106 | 104 MiB |
| 4 | 0.0704 | 0.282 | 0.0517 | 0.207 | 106 MiB |
| 8 | 0.0604 | 0.483 | 0.0492 | 0.394 | 140 MiB |
| 16 | 0.0351 | **0.562** | 0.0419 | 0.670 | 140 MiB |
| 32 | 0.0172 | 0.551 | 0.0219 | **0.701** | 268 MiB |
| **selected** | **16** | | **32** | | |

### 139k / 14M edges

| batch | 3060 thr | P100 thr | VRAM reserved |
|---|---|---|---|
| 1 | **0.1113** | **0.1119** | 202 MiB |
| 2 | 0.0407 | 0.0300 | 364 MiB |
| 4 | 0.0653 | 0.0569 | 472 MiB |
| 8 | 0.0560 | 0.0888 | 472 MiB |
| 16 | 0.0368 | 0.0592 | 800 MiB |
| 32 | 0.0289 | 0.0453 | 1,458 MiB |
| **selected** | **1** | **1** | |

- **OOM: clean 環境では両 GPU・両スケールで発生せず**（全候補 `ok: true`）。
- VRAM 使用は最大 1,458 MiB（3060 の 12.2%）。`vram_headroom: 0.85` の制約は一度も binding しなかった。
- clean 環境では **P100 の方が高 throughput**（20k 最良 0.701 vs 3060 0.562）。
  sparse CSR は帯域律速で、P100 の HBM2 が 3060 の GDDR6 を上回るため。
  汚染下で見えていた「3060 が速い」は llama-server の巻き添えだった。
- worker 経由 (`gpu_smoke.yaml`) での selected batch は **両 GPU とも 8**。
  これは GPU 差ではなく `evaluation.replicates: 8` が上限になるため。

## 3. 2 worker 同時起動 (`gpu_smoke.yaml`)

実験 `GS-20260911-021809-5a259a`、16 evaluations。**PASS**

- 両 worker が正しい GPU 名 / CC / UUID / `nvidia_smi_index` を報告
  (rtx3060→cuda:0 / CC 8.6 / `1d6afaad…`、p100→cuda:1 / CC 6.0 / `d11b6f7f…`)。
  telemetry も別個（436MB/100%/64℃ と 560MB/93%/50℃）で混線なし。
- `requested_replicates == completed_replicates == 8` が **16 evaluation 全てで成立**
  (cuda:0 が 8 本、cuda:1 が 8 本)。`execution_batch_size` は両者 8。
- `scientific_config_hash` は 1 種類のみ (`1c0bca95…`)。
  同一 seed の `replicate_seeds_json` が device 間で相違 **0 件**。duplicate job_id **0 件**。
- fitness 全て約 -4.50、`-inf` および FAILED は **0 件**。
- **速い GPU が遅い GPU を待っていない**: 3060 は 1 ジョブ 8.74s、P100 は 10.59s と
  別ペースで進行、cross-worker の重なり 14 ペア、wall span 85.79s に対し busy 合計
  154.62s（並列度 1.80）。3060 は 78.3s で 8 本完了、P100 は 85.8s まで独立に継続。

## 4. worker loss → UNKNOWN → requeue

実験 `GL-20260911-023317-9b390d`（`gpu_smoke.yaml` 派生、`duration_ms` を 6000 に、
`max_generations` を 4 に引き上げたもの。`gpu_smoke.yaml` は 85 秒で枯れて
mid-job kill ができないため。replicates・backend・environment・fba・env は不変）。**PASS**

```
kill 前   p100 current_job=job_b885e78348cb / rtx3060 current_job=job_aefaf529804b
SIGKILL p100 (pid 101166)
35秒後   worker_lost → job_marked_unknown → job_requeued  各1件
         evaluations_failed: 0   unknown: 0（requeue 済み）
         p100 status=lost failed_jobs=0
         rtx3060 status=online、自身のジョブを完了しつつ requeue された job を再取得
p100 再起動 → 新しい worker_run として online 復帰、正しい GPU identity を再報告
```

**CUDA OOM や GPU 障害が個体の FAILED / -inf fitness として扱われた事例は 0 件。**
実装側も `worker.py` の `_is_oom()` で OOM を検出して execution batch を半減して再試行し、
batch 1 でも不可なら `_is_retryable_infra()` により status `RETRY`（`evaluation = None`）
として返す構造になっている。

## 5. coordinator `kill -9` → `--resume`

**PASS**

```
kill 前   genomes 8 / evals 2 (eval_79f1405fc3c3, eval_f9d89b16466e)、running 2 / queued 4
SIGKILL coordinator (pid 101081)
--resume  同一 experiment ID で復帰、status running
          genomes 8 / evals 2 と eval_id が保持、config_hash == config_hash_stored
          t+80  evals 4  ← kill 時に飛んでいた 2 ジョブが再配送で着地
          t+240 evals 6  queued 0  failed 0  unknown 0
最終      dup job_ids 0 件、dup result_ids 0 件
          per device cuda:0=4本 / cuda:1=2本（いずれも requested=completed=8、batch=8）
```

lineage / evaluations 保持、duplicate evaluation なし、job 再投入正常。

## 6. GPU evaluation の replay

対象 `eval_cacd35caff67`（3060 で記録）。**4 パターン全て PASS**

| replay | 結果 |
|---|---|
| 同一 GPU (cuda:0) `--strict` | **EXIT=0**、`identical_spike_counts: true`、`identical_replicate_seeds: true` |
| 別 GPU (cuda:1) `--strict` | **EXIT=5**「GPU model differs … compute capability differs '8.6' → '6.0'」 |
| `--strict --allow-device-drift` | **EXIT=0**、cross-GPU parity 実行。`recorded_device` / `current_device` を記録し警告 5 件（明示ドリフト / model / CC / UUID / index）。**spikes 39769 = 39769 で完全一致** |
| 同一 GPU `--execution-batch 1` | **EXIT=0**、seed・spike とも一致 |

## 7. Observatory GUI

`http://100.74.160.53:8871/` を MacBook のブラウザから確認。**PASS**

- 日本語表示（ダッシュボード / 個体群・系譜 / 個体インスペクター / MIE・テレメトリ / イベント）。
- 時刻は全て JST 表示（`12:05:28 JST` 等）、DB は UTC 保存。
- RTX 3060 と P100 が別カードで正しく表示され、
  `cuda:0 / CC 8.6 / 1d6afaad…` と `cuda:1 / CC 6.0 / d11b6f7f…`。
- VRAM (0.4/12.0 GB vs 0.5/16.0 GB)、温度 (76℃ vs 56℃)、GPU使用率、実行バッチ、
  実行中ジョブ、完了/失敗ジョブ、ベンチマーク throughput が **カードごとに別値で混線なし**。
- read-only を実証: 制御 endpoint は token 無しで全て拒否。
  `POST /api/control/{pause,resume,checkpoint,stop}` → いずれも **HTTP 401 `bad control token`**、
  status は `running` のまま不変。GUI の HTML 内の操作要素はクライアント側フィルタ
  `<input id="q">` の 1 個のみで、制御ボタン・フォームは存在しない。

## 8. CUDA nondeterminism の有無

**観測されなかった。** README は `torch.sparse.mm` の atomics により CUDA では
bit 再現性が保証されない可能性を「GPU ホストで実測すべき事項」としていたが、
今回の全ケースで bit-identical だった。

| 確認項目 | 結果 |
|---|---|
| checkpoint → 100 steps → restore → 同 100 steps | 両 GPU・両スケールで完全一致 |
| same-seed 2 回実行 | 両 GPU・両スケールで完全一致 |
| execution batch 1 / 2 / 4 の replicate 不変性 | `identical: true`（CUDA では報告のみで assert されない項目だが一致） |
| 同一 GPU での strict replay | `identical_spike_counts: true` |
| **cross-GPU replay (3060 記録 → P100 再生)** | **spikes 39769 = 39769、mean_rate_hz も完全一致** |

3060 と P100 は CC 8.6 と 6.0 で世代が異なるが、それでも spike 数が一致した。

## 9. 発見した不具合

### (a) `numpy<2.3` pin と Python 3.14 の非互換（環境・移植性）

`requirements-gpu-cu126.txt` の `numpy<2.3` は Python 3.14 に wheel が無く、
ソースビルドに落ちて `Python dependency not found` で失敗する
(torch 2.9.1+cu126 自体は cp314 wheel あり)。README は `python3.10 -m venv` を
例示するのみで**対応 Python 上限を明記していない**。Ubuntu 26.04 のように
system Python が 3.14 のみのホストでは README どおりに進めると詰まる。
今回は `uv` で CPython 3.12.13 を用意して回避した（pin は変更していない）。
→ README に対応 Python 範囲（3.10–3.13）を明記するのが望ましい。

### (b) offline worker_run の `current_job_id` が残り、GUI が「実行中ジョブ」として表示

kill された p100 の `worker_run` (`wrun_51c6b3f91caf`) は `status='offline'` になった後も
`current_job_id='job_b885e78348cb'` を保持する。当該 job は既に `SUCCEEDED`
(`claimed_by_worker='rtx3060'`, `finished_at` 記録済み) であり、
Observatory はこれを死んだカード上に「実行中ジョブ」として表示する。
スケジューリングには影響しない（requeue と完了は正しく動作）が、
運用者が「死んだ worker がまだ job を保持している」と誤読しうる。

### (c) 異種GPU比較テーブルが worker_run 単位で、同一 GPU を重複表示

worker を再起動するたび `worker_runs` に行が増え、GUI は全 run をカード化・
列化するため、物理 2 GPU に対し今回 **5 カード / 5 列**（うち 3 つは死んだ登録）
になった。offline カードは最終 telemetry（56℃ / 100% 等）を保持したまま表示される
（`オフライン` と最終ハートビート時刻は併記されるので虚偽ではない）。
異種 GPU の比較という目的に対しては、live な worker で束ねる方が読みやすい。

### (d) replay レポートが evaluation 単位で上書きされる

`<run_dir>/replays/<evaluation_id>.json` は replay のたびに上書きされる。
同一 evaluation に対し strict → drift → parity → batch1 と順に実行すると、
cross-GPU parity の `device_warnings` 記録が最後の実行で消える。
今回は stdout を別ファイルに退避して保全した。

### (e) 所見（不具合ではない）: batch ≥ 2 で throughput が落ちる

`sim_seconds_per_wall_second` が batch 1 → 2 で約 5〜7 倍低下する
（3060 20k: 0.398 → 0.074）。clean 環境でも再現するため競合ではなく、
batch 1 が spmv、batch ≥ 2 が spmm と別カーネル経路に落ちていると考えられる。
結果として 139k では **両 GPU とも selected batch = 1** となり、
execution batch 機構が FlyWire スケールで利得を生んでいない。
正しさの問題ではないが、長時間 run の前に検討する価値がある。

## 10. 最重要判定の照合

| GPU によって変化してよい | 実測 |
|---|---|
| wall time | 変化した（20k: 1.433s vs 1.956s、139k: 4.898s vs 6.639s） |
| throughput | 変化した（20k 最良 0.562 vs 0.701） |
| execution batch | 単独 bench で変化した（20k: 16 vs 32）。worker 経由では replicates 上限で両者 8 |
| VRAM | 総容量差は反映、確保量は同一ワークロードで同値 |

| GPU によって変化してはいけない | 実測 |
|---|---|
| replicate 数 | 全 evaluation で `requested == completed == 8`、device 間差なし |
| replicate seeds | 同一 seed の `replicate_seeds_json` 相違 **0 件** |
| scientific config | `scientific_config_hash` 1 種類のみ |
| fitness 計算 | spike 数・mean_rate_hz が GPU 間で bit-identical、fitness も同水準。`-inf` 0 件 |
| selection 条件 | elite 選抜は同一 config hash 下で実行、世代進行に device 依存なし |

**CUDA OOM や GPU 障害を個体の FAILED / -inf fitness として扱った事例は 0 件。**

## 11. 生成物

`~/kamimusuhi/gpu-validation/` (llm-machine 上):

```
smoke_ab_cuda{0,1}_{20k,139k}.json         clean backend smoke
smoke_ab_cuda{0,1}_{20k,139k}_dirty.json   llama-server 同居下の初回計測（参考）
bench_cuda{0,1}_{20k,139k}.json            clean benchmark
bench_cuda{0,1}_{20k,139k}_dirty.json      同上の汚染版（3060 batch32 の外因 OOM を含む）
replay_same_strict.json                    同一GPU strict replay
replay_drift_strict.{json,err}             cross-GPU strict（EXIT=5）
replay_parity.json / replay_parity_report.json   allow-device-drift parity
replay_batch1.json                         execution-batch 1 replay
runs/GS-20260911-021809-5a259a/            2 worker 実験 (gpu_smoke.yaml)
runs2/GL-20260911-023317-9b390d/           worker loss / resume 実験
gpu_loss.yaml                              項目4-5 用の派生 config
logs/                                      coordinator / worker ログ
```

検証終了後、MIOBA のプロセスは全て停止済み。GPU は 3060 が 43 MiB / 0%、
P100 が 6 MiB / 0% のアイドル状態。**本番進化は開始していない。**
`llama-master.service` と `open-webui.service` は停止したまま（再起動不要との指示による）。
