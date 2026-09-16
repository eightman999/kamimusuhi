# A0.1 データ品質レポート — banc_888 / materialization 888

- 生成: `experiments/mioba/scripts/a01_data_quality.py`
- store: `experiments/mioba/datasets/banc_888/store_v2` (schema_version 2)
- dataset identity: `{"kind": "banc_888", "materialization": 888, "synapse_version": "edgelist_split_v3", "morphology_version": "pcg-skel-2025-07"}`

## 1. Entity taxonomy（§2 — 「neuron」の内訳）

| entity_class | count |
|---|---|
| BIOLOGICAL_NEURON | 128153 |
| ROUGH_NEURON | 7324 |
| ORPHAN_FRAGMENT | 39699 |
| GLIA | 12755 |
| TRACHEA | 212 |
| OTHER_CELL | 197 |
| UNKNOWN_SEGMENT | 15917 |
| **total canonical entities** | **204257** |

表記ルール（§30）: 以後「204,257 neurons」とは呼ばず
「**204257 canonical anatomical entities、うち
biological neurons 128153**」と記述する。

## 2. Duplicate root_id 監査（§4）

- duplicate root_id groups: **165**（全 333 行を
  `duplicate_rows.jsonl` に保存、keep-first で canonical row を決定）
- annotation conflict のあるグループ: **66**
  （canonical row に `annotation_conflict=True` を付与）
- conflict が多い列: {'cell_type': 54, 'super_class': 13, 'cell_class': 11, 'neuropil': 8, 'flow_class': 6, 'soma_x': 3, 'soma_y': 3, 'soma_z': 3}

## 3. 欠損フィールド

- soma 座標あり: 169161 / 204257
- morphology AVAILABLE: 86482
- cell_type あり: 118652
- NT 予測あり: 153938（MODEL_INFERENCE）
- 実測膜パラメータ: 0
  （UNKNOWN のまま、推定値で埋めていない）
- gap junction: UNKNOWN

## 4. Synapse version とランタイムグラフ（§6/§8）

- synapse source: `edgelist_split_v3`（simple v3 との synapse
  総数一致を確認: 42309621）
- raw connection records（split rows、compartment 組合せ別）:
  15716248
- aggregated directed connection pairs（= runtime edges）:
  13620865
- 「13,620,865 pairs」は aggregated pair
  であり「13,620,865 synapses」ではない

## 5. 3層分離（§5）

- Layer A raw: entities.parquet / connections_split.parquet /
  morphology_nodes.parquet / duplicate_rows.jsonl
- Layer B curated: entities.parquet の annotation 列
- Layer C runtime: connectivity.parquet（+ A2 の reduction.parquet）

## 6. Provenance 内訳（§9/§10）

```json
{
 "entity_idx": "PROOFREAD_SEGMENTATION",
 "dataset_id": "PROOFREAD_SEGMENTATION",
 "entity_class": "CURATED_ANNOTATION",
 "annotation_conflict": "CURATED_ANNOTATION",
 "soma_x": "RAW_EM_DERIVED",
 "soma_y": "RAW_EM_DERIVED",
 "soma_z": "RAW_EM_DERIVED",
 "cell_type": "CURATED_ANNOTATION",
 "cell_class": "CURATED_ANNOTATION",
 "flow_class": "CURATED_ANNOTATION",
 "super_class": "CURATED_ANNOTATION",
 "neuropil": "CURATED_ANNOTATION",
 "side": "CURATED_ANNOTATION",
 "hemilineage": "CURATED_ANNOTATION",
 "proofread_status": "PROOFREAD_SEGMENTATION",
 "nt_top": "MODEL_INFERENCE",
 "nt_confidence": "MODEL_INFERENCE",
 "syn_pre_total": "RAW_EM_DERIVED",
 "syn_post_total": "RAW_EM_DERIVED",
 "morphology_status": "RAW_EM_DERIVED",
 "morphology_ref": "RAW_EM_DERIVED",
 "morphology_version": "RAW_EM_DERIVED",
 "morphology_validation": "MODEL_INFERENCE"
}
```

connectivity: `{"pre_idx": "PROOFREAD_SEGMENTATION", "post_idx": "PROOFREAD_SEGMENTATION", "anatomical_count": "RAW_EM_DERIVED", "weight": "MODEL_INFERENCE", "weight_provenance": "CURATED_ANNOTATION", "pre_compartment": "MODEL_INFERENCE", "post_compartment": "MODEL_INFERENCE"}`

## 7. 既知の制限

- split ラベルは per-connection の dominant label に集約
  （runtime edge 用）。全組合せは connections_split.parquet に保持
- per-synapse 座標は未取得（v3 enriched parquet は 18.8GB —
  必要になれば部分取得する）。compartment 情報は connection 粒度
- morphology_nodes は対象回路サブセットのみ取り込み（A2 参照）
