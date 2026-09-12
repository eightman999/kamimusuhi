# M1A — Recurrent Substrate Validation & Repair 報告

対象: `llm_master_now` / `~/kamimusuhi` HEAD 4c73e8d + 本稿の修正コミット
実施日: 2026-09-11 / 検証者: Devin
成果物: `experiments/mioba/.runs/m1a/`（生 JSON・図・`summary.json`）、`experiments/mioba/m1a/`（診断コード）、`experiments/mioba/tests/test_m1a_substrate.py`（回帰テスト 8 件）

## 判定: **PASS（基質力学）＋ 重大な留意事項（現行デフォルト regime は退化）**

substrate の実装は再起的・状態ful・因果応答を持つことが、micro / meso / production の3スケールで証明された。1 件の実装ミス（Poisson 刺激に `wScale` 係数が抜け 250 mV 直接蹴り）を発見し修正した（semantics v2→v3）。

ただし **現行 production デフォルト（wScale=0.275, 1% @50 Hz）では、再帰入力が発火閾値に約 2 桁届かず、観測活動は Poisson 入力のエコーに過ぎない**（W=0 と bit-identical）。これは配線バグではなく動作点の問題であり、**M1B は進化実験の前に regime を非退化領域へ移す必要がある**。

## §1 Forensic audit — Poisson エコーの根因

1 step を実測トレースした（`m1a/diagnostics.py trace`、state を毎 step 読み取り、Poisson 抽選は generator 状態を replay して正確に回収）。

1. **駆動は閾上の直接電圧蹴り**。`v[drive] += Bernoulli(rate·dt) × kick`。1 入力イベント → 同一 step に発火 1 回（refractory 損失 ~1% のみ）。meso 実測: 入力 489 イベント / 出力 485 スパイク = **echo ratio 0.9918**。出力の 100% が駆動ニューロン由来（non-driven = 0）。
2. **再帰経路は正しく配線されているが弱すぎる**。`EventGraph.propagate`（pre-major CSC、`index_add_` で post へ集積）は dense `W @ spikes` と数値一致（テスト済）。遅延環は spike 放出から D=18 step 後に正確に到着（テスト済）。しかし 1 スパイク当たり post の `g` へ ~0.14 mV（`wScale=0.275 × U(0,1)`）、定常的な膜電位寄与は ~0.03–0.3 mV で **閾値ギャップ 7 mV に約 2 桁不足**。実測 `delayed_norm` 平均 1.65 / g_max 1.29 mV。
3. **実装ミス（修正済）**: kick が生の `scalePoisson=250` mV だった。参照実装（Shiu et al.）の PoissonInput weight は `w_syn × f_poi = 0.275 × 250 = 68.75 mV`。どちらも閾上なのでデフォルトではエコーのままだが、vThr=+20 mV が活動を変えるかが変わる（変わるようになった）。
4. **既知の偏差（変更せず）**: 参照実装は駆動ニューロンの refractory を 0 にする。MIOBA では駆動ニューロンも再帰参加者なので tRefrac=2.2 ms を維持した（設計判断として記録）。結果として駆動ニューロンは ~454 Hz 上限でクリップされる。

結論: 観測された「Poisson echo」は wiring バグではなく、**閾上駆動（仕様）＋ 再帰入力が退化 regime（仕様の動作点）**の合成。

## §2–§5 証拠（数値は `summary.json` / 各 JSON に記録）

| Gate | 結果 | 証拠 |
|---|---|---|
| G1 再帰因果性 | **PASS**（操作下） | meso: baseline 485 vs no_recurrence 485（bit-identical=デフォルト退化）vs strong_recurrence 225,414 spikes。prod: 3,458 vs 3,458 vs **2,054,882**（non-driven 2.03M）。micro A–D 全 PASS |
| G2 パラメータ感度 | **PASS** | vThr（200→4 spikes, 400→0, −52→280k 飽和）、wScale（0→0、20→111k、60→175k、200→217k）、tRefrac（0.1→287 … 50→154）、scalePoisson（5→0、25→91）。**tauMem/tauSyn は echo regime で bit-identical**（膜時定数が蹴り支配の活動に効かない正当な物理結果として記録） |
| G3 impulse | **PASS** | pulse 終了後: デフォルトでは即死（残存 10 spike=refractory residue）。wScale≥20 で **自励残響**（meso post-pulse 220k–321k spikes；production でも 2,053,782）。loop raster で 0→1→2→0 の回帰的可視化 |
| G4 状態持続 | **PASS** | 毎 step 状態ワイプで軌道乖離（83/500 step 差、+4 spikes）。refractory 状態だけでも軌道が変わることを確認 |
| G5 production 規模 | **PASS** | 139k / 13,994,841 edges、cuda:0 で上記すべて再現 |
| 入力/出力分離 | **PASS** | no_input → 0 spike；ignition 下で non-driven 2.03M spikes；指標は駆動ではなくネットワーク発火を数える |

回帰テスト: `pytest experiments/mioba/tests/` 全 349 PASS（新規 8 件含む）。

## §6 修正内容（最小）

- `fba/torch_backend.py`: `v[drive] += Bernoulli × wScale × scalePoisson`（旧: `× scalePoisson`、250 mV）。semantics version 2→3（`fba/semantics.py`）、`test_m1_semantics` の期待値を更新。
- 副作用: `wScale` 変異が駆動強度にも効く（参照実装と同じ結合）。wScale→0 で入力が消え、wScale≲0.1 で蹴りが閾下になる。
- 行っていないこと: 駆動ニューロン refractory=0 化（設計変更として報告のみ）、rate 較正、topolog/駆動設計の変更。

## §7 M0 9.5 Hz の検証 — **計量・報告アーティファクトと判定**

- M0 lineage.sqlite の全 296 評価の記録値 `mean_rate_hz` = **0.494–0.499 Hz**（median 0.4968）。`spikes_total`/`per_replicate` とも整合（replicate あたり ~34.5k spikes = 6.9/step = Poisson 入力率）。
- 「9.5 Hz」は `fitness = −|rate − 5| ≈ −4.5` を `rate = 5 + 4.5` と上側分枝で逆算した**報告上の誤読**。下側分枝 0.5 Hz が真値で、DB・現行再現（0.4976 Hz）と一致。GUI は 0.50 Hz を正しく表示していた。
- GPU validation の 36.65 Hz は異なる駆動（20% @ 200 Hz）のエコー — 同じ機構の別入力。
- 0.5885（旧再現）と 0.4976（現行）の差は semantics v1→v3 の RNG/駆動変更による**正常な drift**。
- 影響: M0 の測定は内部的に正しい（echo の計測として）。`9.5 Hz` を引用した記述は誤り。M0 レポート末尾に provenance note を追記した（原文は変更せず）。

## §8 デバイス性能（正しさの根拠ではなく性能のみ）

m1_pilot 条件（139k/14M、500 ms×4 rep、slots {1,2}）:

| device | evals/min | p50 s | VRAM |
|---|---|---|---|
| cpu | 5.56 | 11.00 | — |
| cuda:0 (RTX 3060) | **8.92** | 13.38 | 230 MB |
| cuda:1 (Tesla P100) | 6.64 | 18.03 | 230 MB |

- 3060↔P100 は **bit-identical**（同一 CUDA RNG ストリーム）。CPU は別ストリーム/加算順で ~0.4% 差 — 同一 device・seed での再現性は bit-exact、device 横断は統計的一致。
- 参考: 同一条件の過去 bench（m1-bench/device_bench.json）では cpu 10.8 / 3060 17.8 / P100 13.2 evals/min。負荷差の範囲。

## M1B への提言

**現行デフォルトでは進化実験を再開してはいけない**（M0 と同じ退化 regime）。substrate は正しいので、M1B は「動作点の選定」を行う:

1. `mutation.parameter_scale` [0.9,1.1] は wScale を永久に echo regime に閉じ込める。wScale に ~×20–100 を含む変異範囲か、現実的な代替（実 connectome の synapse-count weight 分布、駆動密度の増加）を検討する。
2. `target_rate_hz` / `minimum_viable_task_score` は非退化 regime を選んだ**後**に、そこでの実測活動分布から設定する（M1A では設定しない）。
3. 「自発/誘発活動」「安定性」「energetic cost」「disturbance tolerance」の評価軸は、残響が観測される regime で初めて意味を持つ。

## 付録: 再現手順

```bash
cd ~/kamimusuhi && source ~/mioba-venv/bin/activate
OUT=experiments/mioba/.runs/m1a
python -m experiments.mioba.m1a.diagnostics --out $OUT micro
python -m experiments.mioba.m1a.diagnostics --out $OUT --neurons 20000 --steps 500 trace
python -m experiments.mioba.m1a.diagnostics --out $OUT --neurons 20000 --steps 500 ablation --strong-factor 60 --pulse-steps 50
python -m experiments.mioba.m1a.diagnostics --out $OUT --neurons 20000 --steps 500 impulse --pulse-steps 50 --wscales 0.275 20 60
python -m experiments.mioba.m1a.diagnostics --out $OUT --neurons 20000 --steps 500 persist
python -m experiments.mioba.m1a.diagnostics --out $OUT --neurons 20000 --steps 500 genomelink
python -m experiments.mioba.m1a.diagnostics --out $OUT --neurons 20000 --steps 300 sweep
python -m experiments.mioba.m1a.diagnostics --out $OUT --device cuda:0 --neurons 139000 --edges 14000000 --steps 500 prodscale
python -m experiments.mioba.cli --config experiments/mioba/configs/m1_pilot.yaml device-bench --devices cpu,cuda:0,cuda:1 --evaluations 4 --slots 1,2 --out $OUT/device_bench.json
python -m experiments.mioba.m1a.diagnostics --out $OUT plots
```
