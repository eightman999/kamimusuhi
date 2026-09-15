# G0-v5 — Predictive Invariant Grounding

独立したobs-only自己教師あり実験。名称衝突により当初予定G0-v4からG0-v5へ変更した。旧実験のコード・結果は変更せず、結果/checkpointを流用しない。[系譜](EXPERIMENT_LINEAGE.md)と[原仕様](EXPERIMENT_INSTRUCTIONS.md)を参照。

## 実行契約

データ生成→evaluator sanity→protocol/source/config/data固定→raw/PCA/exact twin controls→controls再評価→seed0 pilot→sanity gate。初回ユーザー指定範囲はpilotまで。Full seed0–4は未開始で、次段階は独立した判断となる。

Training subprocessに渡すパスは `data/frozen/training/` のみ。archiveキーは `obs` だけを許可し、oracleは `data/frozen/evaluation/` に隔離する。code auditはOSレベルsandboxではない。学習encoder/input/lossにactions、cause、context、対応ラベルを渡さない。

- train: fixed episode context、12-step cause segments、cause%4==contextを欠落させる。
- IID:既知context/composition/scheduleからのペア診断。
- match:異contextの独立trajectory同士。時間的positiveとしては学習に存在しない。
- dynseg:contextと許可composition集合を維持、段長/scheduleのみ変える。
- combo:欠落cause×contextをqueryに使用。
- midctx:cause固定、両contextはそのcauseについて既知。
- intervention:外生phaseとsensor noiseを共有し、cause/context/nuisanceの一つだけ変える。

共有cause成分とcontext依存成分を混合する生成器であり、任意の無関連sensor変換の下での同定可能性は主張しない。固定seedの生成機構1個に対する実験である。

## Controls / gate

raw、trainのみでfitした16-dim PCA、保存した初期重みそのもののuntrained twin。C3 old-G0は旧結果流用禁止・入力非互換によりSKIP。H1のreconstruction比較は直接には未検証。

S1 finite loss、S2 full-collapseなし、S3ラベル不要future-observation ridge readoutでexact twinからvalidation MSEが1%以上改善、S4 archive/code/twin同一性監査、S5同一checkpoint評価がatol1e-7/rtol1e-6で一致。dimensional-collapse/context-only shortcut/全epochの失敗も通過不可。

Full候補はS3のvalidation改善率順で上位3、最低2方式必要。OODラベルは方式/ハイパラ選択に使用しない。科学的成功は5 seedsで3/4 OOD指標、raw/PCA/twinと介入selectivityを比較し4/5方向一致を必要とする。pilot完走やloss低下は科学的成功ではない。

## 実行

リポジトリルートから、torch/numpy/PyYAML/matplotlib/pytestを含むPythonを使う。

```sh
python -m pytest experiments/g0_v5/tests -q
python -m experiments.g0_v5.src.dataset --output experiments/g0_v5/data/frozen
# source/config/evaluator/dataを最終確認後 protocol.freeze(...) で初回lock作成
PYTHON=/path/to/python bash experiments/g0_v5/scripts/run_pilot.sh
PYTHON=/path/to/python bash experiments/g0_v5/scripts/summarize.sh
```

RTX3060でCPC→JEPA、P100でGRU→VICReg。それぞれ独立process。AMP/DDP不使用。既存サービス停止なし。run/dataset/lockは既存出力を上書きしない。バグ修正時は新規出力先でseed0を初めから実行し、旧失敗を保存する。

各runの `manifest.json` のgit_commitはbase commit、未コミット追加実装の実体はsource hashとprotocol_lockで識別する。評価は `evaluation/` 内にも保存し、epoch telemetryと評価統計を混在させない。失敗分類はNONE/NAN/OOM/COLLAPSE_FULL/COLLAPSE_DIM/SHORTCUT_CONTEXT/DATA_LEAK/EVAL_ERROR/TRAIN_DIVERGENCE/SYSTEM_ERRORを予約する。
