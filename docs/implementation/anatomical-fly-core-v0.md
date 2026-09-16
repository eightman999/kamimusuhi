# Anatomical Fly Core — 設計書 v0

> 2026-09-16。MIOBA の研究目的を「合成 FBA0 基質への人工器官進化」から
> 「実ハエ神経系を immutable anatomical core とし、その上へ人工神経
> モジュール（graft）を移植・進化させる」へ転換する指示書の実装設計。
> 元指示書は本ドキュメントの上位仕様であり、本書はその v0 縦切り
> （A0/G0）の実装方針を記述する。

## 1. 方針転換とシリーズ構成

| シリーズ | 内容 | 状態 |
|---|---|---|
| M-series | synthetic FBA0 上の器官進化 | 完了・凍結（simulator validation として再解釈） |
| M1.5 | paired ablation / guard / maintenance / コスト分析 | 完了（infrastructure validation として継承） |
| A-series | Anatomical reconstruction（A0 取込〜A4 生理層） | 本線 |
| G-series | Graft 実験（G0 10n → G1 100n → G2 1000n） | A-series 後 |
| E-series | Graft 進化（host 固定・graft のみ進化） | G-series 安定後 |

- synthetic FBA0 は **substrate family として分離**。既存コード・
  `mac_m1.yaml`・`mac_m2.yaml`・凍結 baseline を一切変更しない。
- synthetic FBA0 上の M2（`mac_m2.yaml`）は本線として開始しない
  （コードと config は残す）。

## 2. Canonical host — BANC

第一候補 BANC（Bates, Phelps, Kim, Yang et al., Nature 2026;
CAVE materialization 888）。理由: brain+VNC を同一 CNS として保持し、
ascending/descending・sensory→CNS→motor 階層をそのまま使える。

データ取得元（匿名 HTTPS 可、実測 200 OK）:
`https://storage.googleapis.com/lee-lab_brain-and-nerve-cord-fly-connectome/compiled_data/banc_888/`

| ファイル | サイズ | 内容 |
|---|---|---|
| `banc_888_meta.feather` | ~57MB | per-neuron metadata（~188k 行） |
| `banc_888_metrics.feather` | ~7MB | morphology + synapse 数スリム表 |
| `banc_888_neurotransmitter_prediction_v2.csv` | ~21MB | NT 予測 |
| `banc_888_edgelist_simple_v3.feather` | ~336MB | neuron→neuron v3 接続 |

規約: データ payload は `/data/cache/banc_888/`（gitignore 済み慣例）。
`experiments/mioba/datasets/` 以下はコード・manifest のみ。

**重要**: 「brainstem-like」「spinal/reflex-like」等は機能上の比喩であり、
脊椎動物との解剖学的相同は主張しない（指示書 §0）。

## 3. 責務分離（BANC 固有を深部へ埋め込まない）

```
datasets/banc_888/            # 取得元定義・manifest・ハッシュ
importers/banc.py           # BANC 生ファイル → canonical store
anatomy/
  provenance.py             # provenance 列挙 + Provenanced 値
  schema.py                 # CanonicalNeuron / CanonicalSynapse / store
  fidelity.py               # ANATOMY_FIDELITY.json 生成
graft/
  namespaces.py             # banc:* / graft:* ID 名前空間
  schema.py                 # graft 定義（固定 G0 用）
fba/torch_backend.py        # anatomy_dir で canonical store を読む
                            # （データセット名を知らない汎用ローダのみ）
```

canonical store の形式（dataset 非依存）:

```
<anatomy_dir>/
  manifest.json        # dataset kind, ソース hash, fidelity 集計, provenance 方針
  neurons.parquet      # canonical neuron 表（下記 schema）
  connectivity.parquet # pre_idx, post_idx, anatomical_count, weight, weight_provenance
```

`torch_backend` は `fba.anatomy_dir`（または `anatomy:` config）を受けて
`connectivity.parquet` を読む汎用 `_load_anatomy()` のみを持つ。
`data_dir`（FlyWire parquet）経路はそのまま温存。

## 4. Provenance-first

すべての生理・解剖パラメータは provenance 付きで保持する:

`EXACT_EM` / `DIRECT_MEASUREMENT` / `CELL_TYPE_MEASUREMENT` /
`TRANSCRIPTOMIC_INFERENCE` / `LITERATURE_PRIOR` / `MODEL_INFERENCE` /
`IMPUTED` / `UNKNOWN`

規則:
- **`UNKNOWN != 0`**。欠損は null + provenance=UNKNOWN で表現し、
  0 や generic 値で黙って埋めない。
- 推定値を導入する場合は `MODEL_INFERENCE`/`IMPUTED` を明記し、
  mapping 自体にも version/provenance を持たせる
  （例: synapse count → conductance prior の変換表）。
- schema レベルで `value=None + provenance!=UNKNOWN` の組合せを
  拒否する（実データの無い値をそれらしく見せない）。

## 5. Canonical schema（v0 最小セット）

Neuron（1 行 = 1 実ニューロン）:
- `neuron_id`（内部 int index）、`dataset_id`（`banc:<root_id>`）
- `soma_x/y/z`（実座標、nm）+ provenance=EXACT_EM
- `cell_type`、`super_class`/`flow`（sensory/intrinsic/motor 等）、
  `neuropil` membership、`side`（left/right/mid）
- `nt_prediction`（top NT + 信頼度、provenance=MODEL_INFERENCE）
- `morphology_ref`（skeleton/mesh への参照ポインタ; v0 では
  L2 skeleton 未取込なら UNKNOWN/未設定を正直に記す）
- `ascending`/`descending` フラグ（BANC メタにあれば）

Synapse/connection（v0 は edgelist 集約で開始）:
- `pre_idx`/`post_idx`（canonical index）、`anatomical_count`
  （EXACT_EM 由来の synapse 数）、`physiological_weight`
  （v0: `count→weight` mapping を MODEL_INFERENCE として記録）、
  `neuropil`（v3 enriched があれば）

**1 neuron = 1 scalar を canonical にしない**（指示書 §6.6）:
canonical 側は morphology 参照と座標を保持し、runtime 側の
point/reduced 簡略化は compiler の責務とする。v0 では runtime は
point のみだが、canonical には soma 座標と morphology 参照枠を残す。

## 6. Host/Graft 名前空間（§19）

- host: `banc:<dataset_root_id>`（例 `banc:7205759406...`）
- graft: `graft:<graft_id>:n<index>`（例 `graft:g001:n00042`）
- 人工ニューロンに BANC/FlyWire 風 ID を付けない。
- `graft/namespaces.py` に `is_host_id`/`is_graft_id`/`parse` を集約。

## 7. A0/G0 縦切りの実装分解

1. `anatomy/provenance.py` — enum + `Provenanced`（value/unit/provenance/
   source）+ バリデーション
2. `anatomy/schema.py` — canonical テーブルの I/O（parquet 書き込み、
   manifest 検証）
3. `datasets/banc_888/` — 取得 URL・期待サイズ・sha256 manifest
4. `importers/banc.py` — feather/csv → canonical store +
   `ANATOMY_FIDELITY.json` 出力
5. `fba/torch_backend.py` — `anatomy_dir` 対応（canonical
   connectivity を読む汎用ローダ + manifest を dataset_identity に反映）
6. `graft/schema.py` + `graft/build.py` — 固定 10-neuron graft を
   phenotype の `artificial_organs`/`attachments` 形式へ compile
   （endpoint 解決は canonical store の neuropil/ID index 経由）
7. `scripts/a0g0_demo.py` — host circuit ロード → graft 接続 →
   paired ablation（full vs graft disabled、同一 seed）
8. Observatory anatomical mode — 実 soma 座標の 2D 投影 +
   host/graft/方向別レイヤー（`anatomy_view.json` 生成器）
9. `configs/anatomical/a0_banc.yaml` / `configs/graft/g0_10n.yaml`

受入基準（指示書 §24）: real ID/morphology 参照/chemical 接続の保持、
provenance メタ、発火伝播、合成ランダム不使用、seed 再現性、
10n graft 双方向伝播、paired ablation 比較、Observatory での
host/graft 分離表示。

## 8. v0 でやらないこと（明示）

- full morphology（skeleton/mesh）の runtime 利用 — canonical に
  参照枠のみ保持、A2 compartment compiler で扱う
- gap junction — BANC whole-CNS データが無ければ status=UNKNOWN 保持
- neuromodulation 層 — schema のみ、runtime は A4 以降
- virtual body — §13 の段階実装の対象、G0 では不要
- E-series（graft 進化）— G-series 安定後

## 9. 既存コードへの影響

- `substrate/`・`fba/` の synthetic 経路: 無変更
- `torch_backend`: `anatomy_dir` 追加のみ（data_dir/synthetic 分岐を
  壊さない）
- `genome`/mutation: G0 は固定 graft のためゲノム拡張は後回し
  （graft schema は独立モジュール、E-series で genome へ統合）
- `m15_ablation.py` の paired 手法を graft ablation へ継承
