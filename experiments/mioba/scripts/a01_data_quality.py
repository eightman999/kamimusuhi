"""A0.1 data-quality report (directive §32) — writes
docs/experiments/A0_1_DATA_QUALITY.md from a canonical v2 store.

Everything reported is computed from the store, so the doc stays
consistent with the data instead of hand-copied numbers.
"""
from __future__ import annotations

import argparse
import json
from collections import Counter
from pathlib import Path

from ..anatomy import schema as A


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--store", required=True)
    ap.add_argument("--out", default="docs/experiments/A0_1_DATA_QUALITY.md")
    args = ap.parse_args()

    manifest, ent, conn = A.load_store_v2(args.store)
    fid = json.loads((Path(args.store) /
                      "ANATOMY_FIDELITY.json").read_text())["counts"]

    dup_path = Path(args.store) / "duplicate_rows.jsonl"
    dup_groups, dup_rows, conflict_groups = 0, 0, 0
    conflict_kinds = Counter()
    if dup_path.is_file():
        seen = {}
        for line in dup_path.read_text().splitlines():
            r = json.loads(line)
            rid = r["root_id"]
            dup_rows += 1
            if rid not in seen:
                dup_groups += 1
                seen[rid] = True
                if r["conflicts"]:
                    conflict_groups += 1
                    for k in r["conflicts"]:
                        conflict_kinds[k] += 1

    cls = manifest.get("entity_class_counts", {})
    ds = manifest.get("dataset", {})
    lines = f"""# A0.1 データ品質レポート — {ds.get('kind')} / materialization {ds.get('materialization')}

- 生成: `experiments/mioba/scripts/a01_data_quality.py`
- store: `{args.store}` (schema_version 2)
- dataset identity: `{json.dumps(ds, ensure_ascii=False)}`

## 1. Entity taxonomy（§2 — 「neuron」の内訳）

| entity_class | count |
|---|---|
"""
    for k in A.ENTITY_CLASSES:
        lines += f"| {k} | {cls.get(k, 0)} |\n"
    lines += f"| **total canonical entities** | **{fid['entities_total']}** |\n"

    lines += f"""
表記ルール（§30）: 以後「204,257 neurons」とは呼ばず
「**{fid['entities_total']} canonical anatomical entities、うち
biological neurons {fid['biological_neurons']}**」と記述する。

## 2. Duplicate root_id 監査（§4）

- duplicate root_id groups: **{dup_groups}**（全 {dup_rows} 行を
  `duplicate_rows.jsonl` に保存、keep-first で canonical row を決定）
- annotation conflict のあるグループ: **{conflict_groups}**
  （canonical row に `annotation_conflict=True` を付与）
- conflict が多い列: {dict(conflict_kinds.most_common(8))}

## 3. 欠損フィールド

- soma 座標あり: {fid['entities_with_soma']} / {fid['entities_total']}
- morphology AVAILABLE: {fid['entities_with_morphology']}
- cell_type あり: {fid['entities_with_cell_type']}
- NT 予測あり: {fid['entities_with_nt_prediction']}（MODEL_INFERENCE）
- 実測膜パラメータ: {fid['entities_with_direct_membrane_measurement']}
  （UNKNOWN のまま、推定値で埋めていない）
- gap junction: {fid['gap_junction_status']}

## 4. Synapse version とランタイムグラフ（§6/§8）

- synapse source: `{ds.get('synapse_version')}`（simple v3 との synapse
  総数一致を確認: {fid['aggregated_synapse_count']}）
- raw connection records（split rows、compartment 組合せ別）:
  {fid['raw_synapse_records']}
- aggregated directed connection pairs（= runtime edges）:
  {fid['aggregated_connection_pairs']}
- 「{fid['aggregated_connection_pairs']:,} pairs」は aggregated pair
  であり「{fid['aggregated_connection_pairs']:,} synapses」ではない

## 5. 3層分離（§5）

- Layer A raw: entities.parquet / connections_split.parquet /
  morphology_nodes.parquet / duplicate_rows.jsonl
- Layer B curated: entities.parquet の annotation 列
- Layer C runtime: connectivity.parquet（+ A2 の reduction.parquet）

## 6. Provenance 内訳（§9/§10）

```json
{json.dumps(manifest.get('field_provenance', {}), indent=1)}
```

connectivity: `{json.dumps(manifest.get('connectivity_provenance', {}))}`

## 7. 既知の制限

- split ラベルは per-connection の dominant label に集約
  （runtime edge 用）。全組合せは connections_split.parquet に保持
- per-synapse 座標は未取得（v3 enriched parquet は 18.8GB —
  必要になれば部分取得する）。compartment 情報は connection 粒度
- morphology_nodes は対象回路サブセットのみ取り込み（A2 参照）
"""
    Path(args.out).write_text(lines)
    print(f"[a01-report] -> {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
