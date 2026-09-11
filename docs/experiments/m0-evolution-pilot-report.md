# M0 Evolution Pilot Report

実験 ID: `M0-20260911-040243-5e9bf7`
実施日: 2026-09-11 (JST)
対象機: `llm-machine` / hostname `master` — RTX 3060 12GB (cuda:0) + Tesla P100 16GB (cuda:1)
config: `experiments/mioba/configs/m0_pilot.yaml`

| | |
|---|---|
| git commit | `6f21d52bffc9e9a79821d5aa86522632c605e132` |
| config_hash | `cee07f4cd2314c386f78d3047cece8e744854183ef804fe699b59aea4a308b19` |
| scientific_config_hash | `6ef2dcf443a258137869f2a643211d521d6aa42d29e11b840a06f183c9bc4c44` |
| runtime_config_hash | `9dd7d108eefdec65f2f4634268bb6ccd928cc5b4e41aae6d0afe982f9358f8d0` |
| dataset identity | `v0-n139000-e14000000`（synthetic 139k neurons / 14M edges） |
| runs_dir | `~/mioba-runs/M0-20260911-040243-5e9bf7` |
| DB backup | `~/mioba-backups/{…-gen0-start,…-final}.sqlite` |
| 稼働時間 | 04:02–06:19 UTC（約 2 時間 17 分） |

## 判定

**PASS**（基盤要件は全項目充足）。ただし世代更新は **9 回**（gen 0–9）で、
指示の 10 回に 1 回足りない。これは障害ではなく、運用者の指示で gen 9 の
途中に graceful stop したため。残り 24 job は `QUEUED` のまま保全されており、
`--resume` で継続可能であることは実証済み（下記 10）。

現行 fitness は暫定のため、**M0 の fitness 向上は研究成果として扱わない**。
M0 が答えたのは「複数世代を実際に回しても基盤が壊れないか」のみ。

## 実行条件（開始前に確認）

- `llama-master.service` / `open-webui.service` ともに inactive
- 両 GPU アイドル（3060 43 MiB / P100 6 MiB、util 0%、compute process なし）
- runs_dir 空き 443 GB、RAM 空き 29 GB
- git commit / config_hash / scientific_config_hash を記録（上表）
- DB バックアップ先 `~/mioba-backups/` を確認し開始時点の DB を退避
- Observatory を `http://100.74.160.53:8880/` で起動して監視
- M0 実行中の人的介入なし（監視のみ。停止は運用者の明示指示による）

## 結果サマリ

| | |
|---|---|
| 世代 | gen 0–9（各 32 個体、計 320 genomes） |
| 世代更新 | 9 回（`generation_advanced` × 9） |
| 評価成功 | 296 |
| 評価失敗 | 0 |
| UNKNOWN | 0 |
| retry | 0（全 320 job が `attempt=0`） |
| checkpoint | 27（periodic、5 分間隔） |
| 1 評価あたり | 約 50 秒 / GPU（replicates 8, execution_batch 1） |
| 1 世代あたり | 約 13–14 分 |

## 判定項目

### 1. 世代更新が正常

gen 0–9 が各 32 個体ちょうど。`births` 行 320、`generation_advanced` イベント 9 件。
`population.target_size: 32` と完全に一致。

### 2. lineage が繋がっている

- gen≥1 で親エッジを持たない個体: **0**
- 参照先が存在しない親 ID: **0**
- 親エッジを辿って generation 0 の根に到達しない個体: **0**
- 親エッジ 319 本 = 全 320 個体 − 根 1 個体

### 3. mutation が発生している

mutation 行 319 件、変異を持つ個体 319（＝根以外の全個体）。
内訳は `parameter` 252 / `organ` 67。全世代で 32/32 個体が変異を持つ
（gen 0 のみ 31/31 = 初期個体 1 を除く全個体）。

### 4. fitness 分布が変化している

| gen | best | median | mean | worst | spread |
|---|---|---|---|---|---|
| 0 | -4.50067 | -4.50299 | -4.50291 | -4.50496 | 0.00429 |
| 1 | -4.50194 | -4.50340 | -4.50337 | -4.50510 | 0.00316 |
| 2 | -4.50159 | -4.50303 | -4.50316 | -4.50516 | 0.00358 |
| 3 | -4.50159 | -4.50314 | -4.50321 | -4.50506 | 0.00347 |
| 4 | -4.50124 | -4.50341 | -4.50331 | -4.50511 | 0.00386 |
| 5 | -4.50090 | -4.50315 | -4.50319 | -4.50544 | 0.00454 |
| 6 | -4.50129 | -4.50359 | -4.50344 | -4.50509 | 0.00379 |
| 7 | -4.50159 | -4.50310 | -4.50331 | -4.50554 | 0.00396 |
| 8 | -4.50111 | -4.50309 | -4.50299 | -4.50561 | 0.00449 |
| 9 | -4.50197 | -4.50287 | -4.50308 | -4.50394 | 0.00197 (n=8) |

分布は世代ごとに動いている。非有限値（`-inf` / NaN）は **0 件**。

**ただし改善方向には動いていない。** 詳細は「M1 への申し送り」参照。

### 5. elite / parent / child 関係が正常

全世代で 32 子個体、親は 7–8 個体（`elite_k: 8`、重複抽選により 7 になる世代あり）。
**前世代以外から選ばれた親は全世代で 0**。親は全て前世代の
fitness 上位 8 個体に含まれる（8/8 または 7/7）。

### 6. 3060 / P100 双方へ job が流れる

| | 評価数 |
|---|---|
| rtx3060 (cuda:0) | 147 |
| p100 (cuda:1) | 149 |

非同期に独立進行し、速い側が遅い側を待つ構造になっていない。
GPU identity は終始分離（rtx3060 → RTX 3060 / CC 8.6 / `1d6afaad-67b7`、
p100 → Tesla P100 / CC 6.0 / `d11b6f7f-002d`）。

**GPU をまたいでも科学条件は不変**:

- `requested_replicates != completed_replicates` の評価: **0**（全 296 件が 8→8）
- `execution_batch_size`: 全 296 件が 1
- `scientific_config_hash`: **1 種類のみ**（`6ef2dcf443a2`）
- `git_commit`: 1 種類のみ（`6f21d52bffc9`）
- dataset identity: 1 種類のみ（`v0-n139000-e14000000`）

### 7. FAILED / UNKNOWN / retry に異常増加なし

`evaluations_failed` 0、`unknown` 0、`last_error` を持つ job 0、
全 320 job が `attempt=0`（再試行ゼロ）。
1 つの job_id に複数 evaluation が紐づく重複は **0 件**。

### 8. checkpoint が継続生成される

`periodic` 27 件、`checkpoint.interval_s: 300` どおり 5 分間隔で
04:07 から 06:17 まで欠落なし。

### 9. DB integrity OK

`pragma integrity_check` → `ok`、`pragma foreign_key_check` → 0 行。
最終 DB サイズ 8.26 MB。

### 10. 再起動後も続行可能

graceful stop（`stop --timeout 180`）で coordinator が正常終了。
停止時点で `running` 0 / `queued` 24 / `unknown` 0 — 実行中 job を
孤児化せずに着地させている。

その後 `start --resume M0-20260911-040243-5e9bf7` で再起動:

```
experiment_id  M0-20260911-040243-5e9bf7（同一）
status         running
births 320 / evaluations_succeeded 296 / queued 24 / failed 0 / unknown 0
config_hash == config_hash_stored
scientific_config_hash 一致
```

lineage・評価・キューが完全に保持されて復帰した。確認後に再度 graceful stop。

### 11. メモリリーク / VRAM 増加なし

74 分間 60 秒間隔で 75 サンプル採取（`~/mioba-logs/leakwatch.log`）:

| | 開始 | 終了 | 変化 |
|---|---|---|---|
| GPU0 VRAM | 1,626 MiB | 1,626 MiB | **0**（全 75 サンプルで同値） |
| GPU1 VRAM | 1,750 MiB | 1,750 MiB | **0**（全 75 サンプルで同値） |
| worker cuda:0 RSS | 1,017,828 kB | 1,018,132 kB | +0.3 MB |
| worker cuda:1 RSS | 1,044,468 kB | 1,044,784 kB | +0.3 MB |
| coordinator RSS | 499,724 kB | 510,560 kB | **+10.6 MB** |

VRAM は完全に一定。worker RSS も実質一定（ジョブ開始時にグラフ構築で
一時的に 1.9–2.1 GB へ跳ねるが毎回ベースラインへ戻る）。

coordinator RSS のみ 74 分で +10.6 MB（約 8.6 MB/h）増加している。
個体数 320 の成長と対応しており、この時間規模ではリークと断定できないが、
**M1 のような長時間 run の前に再測定すべき項目**として残す。

### 終了後の状態

両 GPU はアイドル基準値へ復帰（3060 43 MiB / P100 6 MiB、util 0%）。
MIOBA プロセスは全て終了。`llama-master.service` / `open-webui.service` は
inactive のまま（起動前と同じ）。

## M1 への申し送り

1. **選択圧が実質ゼロ。** mean_rate_hz が全個体で約 9.5 に張り付き、
   target 5.0 から離れたまま動かない。fitness の世代内ばらつきは
   0.002–0.005 しかなく、9 世代を通じて best は -4.50067（gen 0）から
   改善していない。現在の変異は発火率をほとんど動かさないため、
   elite 選抜が機能していても進化圧として効いていない。
   M1 の fitness・environment・mutation pressure 設計の一次入力とする。
2. **coordinator RSS の漸増**（約 8.6 MB/h）を長時間 run 前に再測定する。
3. **execution_batch 機構が FlyWire スケールで利得を生んでいない。**
   GPU validation §9(e) のとおり batch ≥ 2 で throughput が 5–7 倍低下し、
   M0 は両 GPU とも batch 1 で実行した。M1 でスループットを上げるなら
   ここが最大の改善余地。
4. **残り 1 世代。** 10 回目の世代更新が必要なら
   `start --resume M0-20260911-040243-5e9bf7` で約 15 分で完了する。

## 生成物

```
~/mioba-runs/M0-20260911-040243-5e9bf7/   lineage.sqlite, checkpoints/(27), telemetry/, traces/
~/mioba-backups/M0-...-gen0-start.sqlite  開始時点 DB
~/mioba-backups/M0-...-final.sqlite       終了時点 DB（検証に使用）
~/mioba-logs/coordinator.log              coordinator
~/mioba-logs/worker_{rtx3060,p100}.log    worker
~/mioba-logs/leakwatch.log                RSS / VRAM サンプル 75 件
~/mioba-logs/m0_check.py                  本レポートの検証スクリプト
```
